use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use jarscope::cdir::{CentralDirectory, EntryInfo, read_entry};
use jarscope::find::find_jars;
use rayon::prelude::*;
use zip::ZipArchive;

const MANIFEST: &str = "META-INF/MANIFEST.MF";
const CAP: u64 = 1 << 20;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let mode = args.remove(0);
    let use_zip = args.iter().any(|a| a == "--zip");
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| *a != "--zip")
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        usage();
    }
    let mut jars = Vec::new();
    for p in &paths {
        match find_jars(p) {
            Ok((j, _)) => jars.extend(j),
            Err(e) => eprintln!("{}: {e:#}", p.display()),
        }
    }
    match mode.as_str() {
        "compare" => compare(&jars),
        "walk" => walk(&jars, use_zip),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: cdir compare <dir>... | cdir walk <dir|jar>... [--zip]");
    std::process::exit(2);
}

fn walk(jars: &[PathBuf], use_zip: bool) {
    let names = AtomicU64::new(0);
    let errors = AtomicU64::new(0);
    jars.par_iter().for_each(|p| {
        let r = if use_zip { walk_zip(p) } else { walk_cdir(p) };
        match r {
            Ok(n) => {
                names.fetch_add(n, Ordering::Relaxed);
            }
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
    println!(
        "jars {} names {} errors {}",
        jars.len(),
        names.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed)
    );
}

fn walk_cdir(p: &Path) -> anyhow::Result<u64> {
    let mut cd = CentralDirectory::new(BufReader::new(File::open(p)?))?;
    let mut n = 0u64;
    while cd.next_entry()?.is_some() {
        n += 1;
    }
    Ok(n)
}

fn walk_zip(p: &Path) -> anyhow::Result<u64> {
    let z = ZipArchive::new(BufReader::new(File::open(p)?))?;
    Ok(z.file_names().count() as u64)
}

type Rec = (String, u64, u64);

fn cdir_records(p: &Path) -> anyhow::Result<(Vec<Rec>, Vec<EntryInfo>)> {
    let mut cd = CentralDirectory::new(BufReader::new(File::open(p)?))?;
    let mut recs = Vec::new();
    let mut mf = None;
    while let Some(e) = cd.next_entry()? {
        if e.name == MANIFEST {
            mf = Some(e.to_info());
        }
        recs.push((e.name.to_string(), e.size, e.compressed_size));
    }
    Ok((recs, mf.into_iter().collect()))
}

fn zip_records(p: &Path) -> anyhow::Result<Vec<Rec>> {
    let z = ZipArchive::new(BufReader::new(File::open(p)?))?;
    let mut recs = Vec::new();
    for i in 0..z.len() {
        let d = z.by_index_data(i)?;
        recs.push((d.name()?.to_string(), d.size(), d.compressed_size()));
    }
    Ok(recs)
}

fn zip_manifest(p: &Path) -> anyhow::Result<Vec<u8>> {
    let mut z = ZipArchive::new(BufReader::new(File::open(p)?))?;
    let mut f = z.by_name(MANIFEST)?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

fn cdir_manifest(p: &Path, info: &EntryInfo) -> anyhow::Result<Vec<u8>> {
    let mut r = BufReader::new(File::open(p)?);
    let mut v = Vec::new();
    read_entry(&mut r, info, CAP, &mut v)?;
    Ok(v)
}

fn compare(jars: &[PathBuf]) {
    let (mut empty, mut matching, mut dups, mut mism, mut onesided) = (0, 0, 0, 0, 0);
    let mut mf_capped = 0;
    let (mut both_err, mut mf_ok, mut mf_bad, mut mf_n) = (0, 0, 0, 0);
    let (mut dup_raw, mut dup_uniq) = (0u64, 0u64);
    for p in jars {
        let a = cdir_records(p);
        let b = zip_records(p);
        let (recs, mf) = match (a, b) {
            (Ok((a, mf)), Ok(b)) => {
                if a.is_empty() && b.is_empty() {
                    empty += 1;
                }
                let uniq: HashSet<&str> = a.iter().map(|r| r.0.as_str()).collect();
                if uniq.len() != a.len() {
                    dups += 1;
                    dup_raw += a.len() as u64;
                    dup_uniq += uniq.len() as u64;
                    println!(
                        "duplicates: {} ({} vs {})",
                        p.display(),
                        a.len(),
                        uniq.len()
                    );
                    if b.len() != uniq.len() {
                        mism += 1;
                        println!(
                            "MISMATCH zip count {} vs unique {}: {}",
                            b.len(),
                            uniq.len(),
                            p.display()
                        );
                    } else {
                        matching += 1;
                    }
                } else if a == b {
                    matching += 1;
                } else {
                    mism += 1;
                    if mism <= 5 {
                        let i = a.iter().zip(&b).position(|(x, y)| x != y);
                        println!(
                            "MISMATCH {}: {} vs {} records, first diff {:?}",
                            p.display(),
                            a.len(),
                            b.len(),
                            i.map(|i| (&a[i], &b[i]))
                        );
                    }
                }
                (a, mf)
            }
            (Err(a), Err(b)) => {
                both_err += 1;
                println!("both error {}: cdir: {a:#} | zip: {b:#}", p.display());
                continue;
            }
            (a, b) => {
                onesided += 1;
                println!(
                    "ONE-SIDED {}: cdir: {:?} | zip: {:?}",
                    p.display(),
                    a.err().map(|e| format!("{e:#}")),
                    b.err().map(|e| format!("{e:#}"))
                );
                continue;
            }
        };
        let _ = recs;
        if let Some(info) = mf.first() {
            mf_n += 1;
            match (cdir_manifest(p, info), zip_manifest(p)) {
                (Ok(x), Ok(y)) if x == y => mf_ok += 1,
                (Ok(x), Ok(y)) if x.len() as u64 == CAP + 1 && y.starts_with(&x) => mf_capped += 1,
                (x, y) => {
                    mf_bad += 1;
                    if mf_bad <= 5 {
                        println!(
                            "MANIFEST {}: cdir {:?} | zip {:?}",
                            p.display(),
                            x.map(|v| v.len()).map_err(|e| format!("{e:#}")),
                            y.map(|v| v.len()).map_err(|e| format!("{e:#}"))
                        );
                    }
                }
            }
        }
    }
    println!(
        "jars {} empty {empty} matching {matching} duplicate-name {dups} (raw {dup_raw}, unique {dup_uniq}) mismatches {mism} one-sided {onesided} both-error {both_err}",
        jars.len()
    );
    println!(
        "manifests {mf_n} equal {mf_ok} over-cap-prefix-equal {mf_capped} different-or-error {mf_bad}"
    );
}
