mod common;

use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use common::{
    RawEntry, RawZip, break_local_headers, class_bytes, cut_jar_bytes, make_jar,
    make_jar_with_duplicate, make_jar_with_manifest, manifest_bytes, raw_zip, stored_jar_bytes,
};
use jarscope::cdir::CentralDirectory;
use jarscope::jar::{JarInfo, read_info, read_names, scan_jar};
use jarscope::manifest::Manifest;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn info(path: &Path) -> JarInfo {
    read_info(path).expect("read_info")
}

fn names(path: &Path) -> Vec<String> {
    let names = read_names(path).expect("read_names");
    let out: Vec<String> = names.iter().map(str::to_string).collect();
    assert_eq!(names.len(), out.len());
    assert_eq!(names.is_empty(), out.is_empty());
    out
}

fn info_err(path: &Path) -> String {
    format!("{:#}", read_info(path).expect_err("read_info should fail"))
}

fn names_err(path: &Path) -> String {
    format!(
        "{:#}",
        read_names(path).expect_err("read_names should fail")
    )
}

fn scan_err(path: &Path) -> String {
    format!("{:#}", scan_jar(path).expect_err("scan should fail"))
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write jar");
    path
}

fn manifest_with(attrs: &[(&str, &str)]) -> Manifest {
    jarscope::manifest::parse_manifest(&manifest_bytes(attrs))
}

#[test]
fn names_in_archive_order() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/", vec![]),
            ("com/A.class", class_bytes(0, 52)),
            ("com/data.txt", vec![b'a'; 10_000]),
            ("README.md", b"hi".to_vec()),
        ],
    );
    assert_eq!(
        names(&jar),
        ["com/", "com/A.class", "com/data.txt", "README.md"]
    );

    let empty = make_jar(dir.path(), "e.jar", &[]);
    assert!(names(&empty).is_empty());
    assert_eq!(info(&empty), JarInfo::default());
}

#[test]
fn counts_and_totals() {
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
    let r = info(&jar);
    assert_eq!(r.entries, entries.len() as u64 + 1);
    assert_eq!(r.classes, 4);

    let manifest_len = manifest_bytes(&[]).len() as u64;
    let data_len: u64 = entries.iter().map(|(_, d)| d.len() as u64).sum();
    assert_eq!(r.size, manifest_len + data_len);

    let mut archive = ZipArchive::new(File::open(&jar).expect("open")).expect("zip");
    let compressed: u64 = (0..archive.len())
        .map(|i| archive.by_index(i).expect("entry").compressed_size())
        .sum();
    assert_eq!(r.compressed_size, compressed);
    assert!(r.compressed_size < r.size);
}

#[test]
fn manifest_present_or_absent() {
    let dir = TempDir::new().expect("tempdir");
    let entries = [("com/A.class", class_bytes(0, 52))];

    let without = make_jar(dir.path(), "plain.jar", &entries);
    assert_eq!(info(&without).manifest, None);
    assert!(!info(&without).multi_release);

    let with = make_jar_with_manifest(dir.path(), "mr.jar", &[("Multi-Release", "true")], &entries);
    let r = info(&with);
    assert_eq!(
        r.manifest,
        Some(manifest_with(&[("Multi-Release", "true")]))
    );
    assert!(r.multi_release);
}

#[test]
fn duplicate_names_are_all_counted() {
    let dir = TempDir::new().expect("tempdir");
    let first = class_bytes(0, 52);
    let mut second = class_bytes(0, 61);
    second.extend_from_slice(&[0u8; 100]);
    let jar = make_jar_with_duplicate(dir.path(), "dup.jar", first.clone(), second.clone());

    assert_eq!(names(&jar), ["a/A.class", "a/A.class"]);
    let r = info(&jar);
    assert_eq!((r.entries, r.classes), (2, 2));
    assert_eq!(r.size, (first.len() + second.len()) as u64);

    let s = scan_jar(&jar).expect("scan");
    assert_eq!(s.base, [(52, 1), (61, 1)].into_iter().collect());
}

#[test]
fn scan_jar_agrees_with_read_info() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[("Multi-Release", "true")],
        &[
            ("com/", vec![]),
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
            ("module-info.class", class_bytes(0, 53)),
            ("com/Bad.class", vec![0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 52]),
        ],
    );
    let i = info(&jar);
    let s = scan_jar(&jar).expect("scan");
    assert_eq!(i.manifest, s.manifest);
    assert_eq!(i.multi_release, s.multi_release);
    assert!(s.multi_release);
    let base: usize = s.base.values().sum();
    let versioned: usize = s.versioned.values().flat_map(|m| m.values()).sum();
    let counted = base + versioned + s.ignored_versioned + s.module_info.len() + s.invalid.len();
    assert_eq!(
        (base, versioned, s.module_info.len(), s.invalid.len()),
        (1, 1, 1, 1)
    );
    assert_eq!(counted as u64, i.classes);
    assert_eq!(s.required_major(), Some(52));
}

#[test]
fn errors_carry_the_same_context_as_scan_jar() {
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("does-not-exist.jar");
    let empty = write(dir.path(), "empty.jar", b"");
    let not_zip = write(dir.path(), "x.jar", b"this is not a zip file\n");

    let context = format!("cannot open {}: ", missing.display());
    for msg in [info_err(&missing), names_err(&missing), scan_err(&missing)] {
        assert!(msg.starts_with(&context), "{msg}");
    }
    for path in [&empty, &not_zip] {
        let context = format!("{} is not a valid JAR/ZIP file: ", path.display());
        for msg in [info_err(path), names_err(path), scan_err(path)] {
            assert!(msg.starts_with(&context), "{msg}");
        }
    }
}

#[test]
fn central_directory_cut_in_the_middle_fails_the_jar() {
    let dir = TempDir::new().expect("tempdir");
    let jar = write(dir.path(), "cut.jar", &cut_jar_bytes());
    let want = format!(
        "{} is not a valid JAR/ZIP file: truncated or corrupt central directory",
        jar.display()
    );
    assert_eq!(info_err(&jar), want);
    assert_eq!(names_err(&jar), want);
    assert_eq!(scan_err(&jar), want);
}

// Two Zip64 records with every size at `u64::MAX`; nothing past the central directory is read.
#[test]
fn totals_saturate() {
    let dir = TempDir::new().expect("tempdir");
    let entry = |name: &str| RawEntry {
        zip64: true,
        zip64_values: Some(vec![u64::MAX, u64::MAX - 1, 0]),
        ..RawEntry::new(name, "")
    };
    let bytes = raw_zip(&[entry("a.class"), entry("b.class")], &RawZip::default());
    let jar = write(dir.path(), "huge.jar", &bytes);
    let r = info(&jar);
    assert_eq!((r.entries, r.classes), (2, 2));
    assert_eq!((r.size, r.compressed_size), (u64::MAX, u64::MAX));
}

fn manifest_flags(bytes: &[u8]) -> u16 {
    let mut cd = CentralDirectory::new(Cursor::new(bytes)).expect("cdir");
    while let Some(e) = cd.next_entry().expect("entry") {
        if e.name == "META-INF/MANIFEST.MF" {
            return e.flags;
        }
    }
    panic!("no manifest");
}

const ATTRS: &[(&str, &str)] = &[("Main-Class", "a.Main"), ("Multi-Release", "true")];

fn assert_manifest_read(path: &Path) {
    let r = info(path);
    assert_eq!(r.manifest, Some(manifest_with(ATTRS)));
    assert!(r.multi_release);
}

#[test]
fn manifest_deflated_stored_or_with_data_descriptor() {
    let dir = TempDir::new().expect("tempdir");
    let class = class_bytes(0, 52);
    let entries = [
        ("META-INF/MANIFEST.MF", manifest_bytes(ATTRS)),
        ("a/Main.class", class.clone()),
    ];

    let deflated = make_jar(dir.path(), "deflated.jar", &entries);
    assert_manifest_read(&deflated);

    let stored = write(dir.path(), "stored.jar", &stored_jar_bytes(&entries));
    assert_manifest_read(&stored);

    let mut buf = Vec::new();
    let mut zip = ZipWriter::new_stream(&mut buf);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in &entries {
        zip.start_file(*name, opts).expect("file");
        zip.write_all(data).expect("write");
    }
    zip.finish().expect("finish");
    assert_ne!(manifest_flags(&buf) & 0x8, 0, "expected a data descriptor");
    let streamed = write(dir.path(), "streamed.jar", &buf);
    assert_manifest_read(&streamed);
}

#[test]
fn manifest_over_the_cap_parses_the_first_mib() {
    let dir = TempDir::new().expect("tempdir");
    let mut text = b"Manifest-Version: 1.0\r\nMain-Class: a.Main\r\nX-Pad: ".to_vec();
    text.resize(1 << 20, b'x');
    text.extend_from_slice(b"\r\nMulti-Release: true\r\n\r\n");
    let jar = make_jar(dir.path(), "big.jar", &[("META-INF/MANIFEST.MF", text)]);
    let r = info(&jar);
    let m = r.manifest.expect("manifest");
    assert_eq!(m.get("Main-Class"), Some("a.Main"));
    assert_eq!(m.get("X-Pad").map(str::len), Some((1 << 20) - 50));
    assert_eq!(m.get("Multi-Release"), None);
    assert!(!r.multi_release);
}

#[test]
fn manifest_with_broken_local_header_is_empty() {
    let dir = TempDir::new().expect("tempdir");
    let bytes = stored_jar_bytes(&[
        ("META-INF/MANIFEST.MF", manifest_bytes(ATTRS)),
        ("a/Main.class", class_bytes(0, 52)),
    ]);
    let jar = write(dir.path(), "broken.jar", &bytes);
    break_local_headers(&jar);
    let r = info(&jar);
    assert_eq!(r.manifest, Some(Manifest::default()));
    assert!(!r.multi_release);
    assert_eq!((r.entries, r.classes), (2, 1));
}

// Signed JARs list every file after the main section; only the main section is parsed.
fn signed_manifest(files: usize) -> Vec<u8> {
    let mut text = manifest_bytes(ATTRS);
    text.extend_from_slice(b"\r\n");
    for i in 0..files {
        let section = format!("Name: org/x/C{i}.class\r\nSHA-256-Digest: {i:044}=\r\n\r\n");
        text.extend_from_slice(section.as_bytes());
    }
    text
}

#[test]
fn signed_manifest_reads_the_main_section() {
    let dir = TempDir::new().expect("tempdir");
    // ~80 bytes per section: about 0.8 MiB, and 2.4 MiB to also pass the 1 MiB cap.
    for files in [10_000, 30_000] {
        let entries = [
            ("META-INF/MANIFEST.MF", signed_manifest(files)),
            ("META-INF/versions/11/a/V.class", class_bytes(0, 55)),
            ("a/Main.class", class_bytes(0, 52)),
        ];
        let deflated = make_jar(dir.path(), &format!("d{files}.jar"), &entries);
        let stored = write(
            dir.path(),
            &format!("s{files}.jar"),
            &stored_jar_bytes(&entries),
        );
        for jar in [deflated, stored] {
            assert_manifest_read(&jar);
            let s = scan_jar(&jar).expect("scan");
            assert_eq!(s.manifest, Some(manifest_with(ATTRS)));
            assert_eq!(s.versioned.len(), 1, "{}", jar.display());
        }
    }
}

// The CRC covers the whole manifest, so a damage after the main section must still be noticed.
#[test]
fn manifest_with_a_wrong_crc_is_empty() {
    let dir = TempDir::new().expect("tempdir");
    let mut bytes = stored_jar_bytes(&[
        ("META-INF/MANIFEST.MF", signed_manifest(1000)),
        ("a/Main.class", class_bytes(0, 52)),
    ]);
    let central = bytes
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("central header");
    bytes[central + 16] ^= 1;
    let jar = write(dir.path(), "bad-crc.jar", &bytes);
    let r = info(&jar);
    assert_eq!(r.manifest, Some(Manifest::default()));
    assert!(!r.multi_release);
    assert_eq!(
        scan_jar(&jar).expect("scan").manifest,
        Some(Manifest::default())
    );
}

#[test]
fn duplicated_manifest_last_record_wins() {
    let dir = TempDir::new().expect("tempdir");
    let first = manifest_bytes(&[("Main-Class", "a.First")]);
    let bytes = raw_zip(
        &[
            RawEntry::new("META-INF/MANIFEST.MF", &first),
            RawEntry::new("META-INF/MANIFEST.MF", manifest_bytes(ATTRS)),
        ],
        &RawZip::default(),
    );
    let jar = write(dir.path(), "two.jar", &bytes);
    assert_manifest_read(&jar);
    assert_eq!(info(&jar).entries, 2);

    let bytes = raw_zip(
        &[
            RawEntry::new("META-INF/MANIFEST.MF", manifest_bytes(ATTRS)),
            RawEntry::new("META-INF/MANIFEST.MF", &first),
        ],
        &RawZip::default(),
    );
    let jar = write(dir.path(), "two-reversed.jar", &bytes);
    let r = info(&jar);
    assert_eq!(
        r.manifest,
        Some(manifest_with(&[("Main-Class", "a.First")]))
    );
    assert!(!r.multi_release);
}

fn raw_jar(zip64: bool) -> Vec<RawEntry> {
    [
        ("META-INF/MANIFEST.MF", manifest_bytes(ATTRS)),
        ("a/", Vec::new()),
        ("a/Main.class", class_bytes(0, 52)),
    ]
    .into_iter()
    .map(|(name, data)| RawEntry {
        zip64,
        ..RawEntry::new(name, data)
    })
    .collect()
}

#[test]
fn prepended_data_and_zip64() {
    let dir = TempDir::new().expect("tempdir");
    let script = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n".repeat(3);
    let size = (manifest_bytes(ATTRS).len() + class_bytes(0, 52).len()) as u64;
    for (name, zip64, prefix) in [
        ("prefixed.jar", false, script.clone()),
        ("zip64.jar", true, Vec::new()),
        ("prefixed-zip64.jar", true, script.clone()),
    ] {
        let opts = RawZip {
            zip64,
            prefix,
            ..RawZip::default()
        };
        let jar = write(dir.path(), name, &raw_zip(&raw_jar(zip64), &opts));
        assert_eq!(
            names(&jar),
            ["META-INF/MANIFEST.MF", "a/", "a/Main.class"],
            "{name}"
        );
        let r = info(&jar);
        assert_eq!(r.manifest, Some(manifest_with(ATTRS)), "{name}");
        assert!(r.multi_release, "{name}");
        assert_eq!((r.entries, r.classes), (3, 1), "{name}");
        assert_eq!((r.size, r.compressed_size), (size, size), "{name}");
    }
}
