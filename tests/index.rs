mod common;

use std::fs::File;
use std::path::Path;

use common::{class_bytes, make_jar, make_jar_with_duplicate, make_jar_with_manifest};
use jarscope::jar::{JarReport, read_index, scan_jar};
use tempfile::TempDir;
use zip::ZipArchive;

fn index(path: &Path) -> JarReport {
    read_index(path).expect("read_index")
}

fn index_err(path: &Path) -> String {
    format!(
        "{:#}",
        read_index(path).expect_err("read_index should fail")
    )
}

fn scan_err(path: &Path) -> String {
    format!("{:#}", scan_jar(path).expect_err("scan should fail"))
}

#[test]
fn entries_in_archive_order_with_sizes() {
    let dir = TempDir::new().expect("tempdir");
    let text = vec![b'a'; 10_000];
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/", vec![]),
            ("com/A.class", class_bytes(0, 52)),
            ("com/data.txt", text.clone()),
            ("README.md", b"hi".to_vec()),
        ],
    );
    let r = index(&jar);
    let got: Vec<_> = r
        .entries
        .iter()
        .map(|e| (e.index, e.name.as_str(), e.is_dir(), e.size))
        .collect();
    assert_eq!(
        got,
        [
            (0, "com/", true, 0),
            (1, "com/A.class", false, class_bytes(0, 52).len() as u64),
            (2, "com/data.txt", false, text.len() as u64),
            (3, "README.md", false, 2),
        ]
    );
    assert!(r.entries[2].compressed_size < r.entries[2].size);
    assert!(r.invalid.is_empty());
    assert!(!r.duplicate_names);
}

#[test]
fn class_count_and_totals() {
    let dir = TempDir::new().expect("tempdir");
    let entries = [
        ("module-info.class", class_bytes(0, 53)),
        ("com/A.class", class_bytes(0, 52)),
        ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ("BOOT-INF/classes/com/B.class", class_bytes(0, 52)),
        ("x.class/", vec![]),
        ("com/notes.txt", vec![b'x'; 5_000]),
    ];
    let jar = make_jar_with_manifest(dir.path(), "a.jar", &[], &entries);
    let r = index(&jar);
    assert_eq!(r.entries.len(), entries.len() + 1);
    assert_eq!(r.class_count(), 4);

    let manifest_len = common::manifest_bytes(&[]).len() as u64;
    let data_len: u64 = entries.iter().map(|(_, d)| d.len() as u64).sum();
    assert_eq!(r.total_size(), manifest_len + data_len);

    let mut archive = ZipArchive::new(File::open(&jar).expect("open")).expect("zip");
    let compressed: u64 = (0..archive.len())
        .map(|i| archive.by_index(i).expect("entry").compressed_size())
        .sum();
    assert_eq!(r.total_compressed_size(), compressed);
    assert!(r.total_compressed_size() < r.total_size());
}

#[test]
fn manifest_present_or_absent() {
    let dir = TempDir::new().expect("tempdir");
    let entries = [("com/A.class", class_bytes(0, 52))];

    let without = make_jar(dir.path(), "plain.jar", &entries);
    assert_eq!(index(&without).manifest, None);

    let with = make_jar_with_manifest(dir.path(), "mr.jar", &[("Multi-Release", "true")], &entries);
    let r = index(&with);
    assert!(r.manifest.is_some());
    assert!(r.multi_release);
}

#[test]
fn duplicate_names_are_collapsed() {
    let dir = TempDir::new().expect("tempdir");
    let first = class_bytes(0, 52);
    let mut second = class_bytes(0, 61);
    second.extend_from_slice(&[0u8; 100]);
    let jar = make_jar_with_duplicate(dir.path(), "dup.jar", first, second.clone());

    let r = index(&jar);
    assert!(r.duplicate_names);
    let dups: Vec<_> = r.entries.iter().filter(|e| e.name == "a/A.class").collect();
    assert_eq!(dups.len(), 1);
    // `zip` keeps the last entry's data for a duplicated name.
    assert_eq!(dups[0].size, second.len() as u64);

    let s = scan_jar(&jar).expect("scan");
    assert!(s.duplicate_names);
    assert_eq!(s.base.get(&61), Some(&1));
    assert_eq!(s.base.get(&52), None);
}

#[test]
fn scan_jar_shares_the_index() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[("Multi-Release", "true")],
        &[
            ("com/", vec![]),
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ],
    );
    let i = index(&jar);
    let s = scan_jar(&jar).expect("scan");
    assert_eq!(i.entries, s.entries);
    assert_eq!(i.manifest, s.manifest);
    assert!(i.base.is_empty());
    assert_eq!(s.required_major(), Some(52));
}

#[test]
fn errors_match_scan_jar() {
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("does-not-exist.jar");
    let empty = dir.path().join("empty.jar");
    std::fs::write(&empty, b"").expect("write");
    let not_zip = dir.path().join("x.jar");
    std::fs::write(&not_zip, "this is not a zip file\n").expect("write");

    let msg = index_err(&missing);
    assert!(msg.contains("cannot open"), "{msg}");
    assert!(msg.contains("does-not-exist.jar"), "{msg}");
    for path in [&empty, &not_zip] {
        let msg = index_err(path);
        assert!(msg.contains("is not a valid JAR/ZIP file"), "{msg}");
    }
    for path in [&missing, &empty, &not_zip] {
        assert_eq!(index_err(path), scan_err(path));
    }
}
