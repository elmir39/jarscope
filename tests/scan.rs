mod common;

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use common::{
    RawEntry, RawZip, class_bytes, make_jar, make_jar_with_manifest, manifest_bytes, raw_zip,
    stored_jar_bytes,
};
use jarscope::cdir::CentralDirectory;
use jarscope::jar::{JarReport, read_info, scan_jar};
use jarscope::manifest::parse_manifest;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MR: &[(&str, &str)] = &[("Multi-Release", "true")];

fn scan(path: &Path) -> JarReport {
    scan_jar(path).expect("scan")
}

fn scan_err(path: &Path) -> String {
    format!("{:#}", scan_jar(path).expect_err("scan should fail"))
}

fn counts(pairs: &[(u16, usize)]) -> BTreeMap<u16, usize> {
    pairs.iter().copied().collect()
}

#[test]
fn java8_only() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/B.class", class_bytes(0, 52)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.required_major(), Some(52));
    assert_eq!(r.base, counts(&[(52, 2)]));
    assert!(r.invalid.is_empty());
    assert!(r.preview.is_empty());
    assert!(!r.multi_release);
}

#[test]
fn mixed_52_and_61() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/B.class", class_bytes(0, 61)),
            ("com/C.class", class_bytes(0, 52)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.required_major(), Some(61));
    assert_eq!(r.base, counts(&[(52, 2), (61, 1)]));
}

#[test]
fn multi_release_versioned_counted_separately() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        MR,
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ],
    );
    let r = scan(&jar);
    assert!(r.multi_release);
    assert_eq!(r.required_major(), Some(52));
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.versioned[&17][&61], 1);
    assert_eq!(r.versioned.len(), 1);
    assert_eq!(r.ignored_versioned, 0);
}

#[test]
fn versioned_ignored_without_multi_release_flag() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[("Main-Class", "com.A")],
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ],
    );
    let r = scan(&jar);
    assert!(!r.multi_release);
    assert_eq!(r.ignored_versioned, 1);
    assert!(r.versioned.is_empty());
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.required_major(), Some(52));
}

#[test]
fn versioned_without_any_manifest_not_counted_in_base() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/21/com/A.class", class_bytes(0, 65)),
            ("META-INF/versions/21/com/B.class", class_bytes(0, 65)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.ignored_versioned, 2);
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.required_major(), Some(52));
}

#[test]
fn root_module_info_reported_separately() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("module-info.class", class_bytes(0, 53)),
            ("com/A.class", class_bytes(0, 52)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.required_major(), Some(52));
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.module_info, [("module-info.class".to_string(), 53)]);
}

#[test]
fn versioned_module_info_listed_with_or_without_flag() {
    let dir = TempDir::new().expect("tempdir");
    let entries = [
        ("META-INF/versions/9/module-info.class", class_bytes(0, 53)),
        ("com/A.class", class_bytes(0, 52)),
    ];
    for (name, attrs) in [("mr.jar", MR), ("plain.jar", &[][..])] {
        let jar = make_jar_with_manifest(dir.path(), name, attrs, &entries);
        let r = scan(&jar);
        assert_eq!(
            r.module_info,
            [("META-INF/versions/9/module-info.class".to_string(), 53)],
            "{name}"
        );
        assert_eq!(r.required_major(), Some(52), "{name}");
        assert!(r.versioned.is_empty(), "{name}");
        assert_eq!(r.ignored_versioned, 0, "{name}");
    }
}

#[test]
fn preview_base_class() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/P.class", class_bytes(0xFFFF, 61)),
            ("com/A.class", class_bytes(0, 52)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.preview, [("com/P.class".to_string(), 61)]);
    assert_eq!(r.base, counts(&[(52, 1), (61, 1)]));
    assert_eq!(r.required_major(), Some(61));
}

#[test]
fn preview_versioned_class_in_multi_release_jar() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        MR,
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("META-INF/versions/21/com/P.class", class_bytes(0xFFFF, 65)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(
        r.preview,
        [("META-INF/versions/21/com/P.class".to_string(), 65)]
    );
    assert_eq!(r.versioned[&21][&65], 1);
    assert_eq!(r.required_major(), Some(52));
}

#[test]
fn invalid_classes_recorded_and_scan_continues() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/Bad.class", vec![0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 52]),
            ("com/Short.class", vec![0xCA, 0xFE, 0xBA]),
            ("com/B.class", class_bytes(0, 55)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(
        r.invalid,
        [
            ("com/Bad.class".to_string(), "bad magic".to_string()),
            (
                "com/Short.class".to_string(),
                "truncated class header".to_string()
            ),
        ]
    );
    assert_eq!(r.base, counts(&[(52, 1), (55, 1)]));
    assert_eq!(r.required_major(), Some(55));
}

#[test]
fn corrupt_compressed_data_is_invalid_not_fatal() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/Corrupt.class", class_bytes(0, 52)),
        ],
    );
    // 0xFF at the start of a deflate stream means block type 11 (reserved) -> corrupt data.
    let mut bytes = std::fs::read(&jar).expect("read jar");
    let name = b"com/Corrupt.class";
    let pos = bytes
        .windows(name.len())
        .position(|w| w == name)
        .expect("local header");
    let extra_len = u16::from_le_bytes([bytes[pos - 2], bytes[pos - 1]]) as usize;
    let data = pos + name.len() + extra_len;
    bytes[data..data + 4].fill(0xFF);
    std::fs::write(&jar, &bytes).expect("write jar");

    let r = scan(&jar);
    assert_eq!(r.invalid.len(), 1, "{:?}", r.invalid);
    assert_eq!(r.invalid[0].0, "com/Corrupt.class");
    assert!(!r.invalid[0].1.is_empty());
    assert_eq!(r.base, counts(&[(52, 1)]));
}

#[test]
fn no_classes_means_no_requirement() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(dir.path(), "a.jar", &[], &[("readme.txt", b"hi".to_vec())]);
    let r = scan(&jar);
    assert_eq!(r.required_major(), None);
    assert!(r.base.is_empty());
    assert!(r.invalid.is_empty());
}

#[test]
fn directories_and_resources_ignored() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/", vec![]),
            ("com/foo/", vec![]),
            ("x.class/", vec![]),
            ("config.properties", b"a=b".to_vec()),
            ("data.bin", b"garbage".to_vec()),
            ("com/foo/A.class", class_bytes(0, 52)),
        ],
    );
    let r = scan(&jar);
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert!(r.invalid.is_empty());
    assert!(r.module_info.is_empty());
    assert_eq!(r.ignored_versioned, 0);
}

#[test]
fn manifest_is_accessible() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[("Main-Class", "com.example.Main")],
        &[("com/A.class", class_bytes(0, 52))],
    );
    let r = scan(&jar);
    assert_eq!(
        r.manifest.as_ref().and_then(|m| m.get("Main-Class")),
        Some("com.example.Main")
    );
    assert_eq!(
        r.manifest.as_ref().and_then(|m| m.get("Manifest-Version")),
        Some("1.0")
    );
}

#[test]
fn missing_manifest_is_none() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(dir.path(), "a.jar", &[("com/A.class", class_bytes(0, 52))]);
    let r = scan(&jar);
    assert_eq!(r.manifest, None);
    assert!(!r.multi_release);
}

#[test]
fn large_class_entry_scans_correctly() {
    let dir = TempDir::new().expect("tempdir");
    let mut big = class_bytes(0, 65);
    big.resize(5 * 1024 * 1024, 0x42);
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[("com/Big.class", big), ("com/A.class", class_bytes(0, 52))],
    );
    let r = scan(&jar);
    assert_eq!(r.base, counts(&[(52, 1), (65, 1)]));
    assert!(r.invalid.is_empty());
}

#[test]
fn missing_file_is_cannot_open() {
    let dir = TempDir::new().expect("tempdir");
    let msg = scan_err(&dir.path().join("does-not-exist.jar"));
    assert!(msg.contains("cannot open"), "{msg}");
    assert!(msg.contains("does-not-exist.jar"), "{msg}");
}

#[test]
fn non_zip_file_is_not_valid() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("x.jar");
    std::fs::write(&path, "this is not a zip file\n").expect("write");
    let msg = scan_err(&path);
    assert!(msg.contains("is not a valid JAR/ZIP file"), "{msg}");
    assert!(msg.contains("x.jar"), "{msg}");
}

#[test]
fn empty_file_is_error() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("empty.jar");
    std::fs::write(&path, b"").expect("write");
    let msg = scan_err(&path);
    assert!(msg.contains("is not a valid JAR/ZIP file"), "{msg}");
}

#[test]
fn truncated_jar_is_error() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        MR,
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/B.class", class_bytes(0, 61)),
            ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ],
    );
    let bytes = std::fs::read(&jar).expect("read");
    std::fs::write(&jar, &bytes[..bytes.len() / 2]).expect("write");
    assert!(scan_jar(&jar).is_err());
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write jar");
    path
}

fn versioned(pairs: &[(u32, u16, usize)]) -> BTreeMap<u32, BTreeMap<u16, usize>> {
    let mut out: BTreeMap<u32, BTreeMap<u16, usize>> = BTreeMap::new();
    for &(release, major, n) in pairs {
        out.entry(release).or_default().insert(major, n);
    }
    out
}

// Classes on both sides of the manifest record: only a look-ahead knows `Multi-Release`.
fn manifest_last(attrs: &[(&str, &str)]) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("META-INF/versions/17/com/A.class", class_bytes(0, 61)),
        ("com/A.class", class_bytes(0, 52)),
        ("META-INF/MANIFEST.MF", manifest_bytes(attrs)),
        ("META-INF/versions/21/com/B.class", class_bytes(0, 65)),
    ]
}

#[test]
fn manifest_after_versioned_classes_with_multi_release() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(dir.path(), "a.jar", &manifest_last(MR));
    let r = scan(&jar);
    assert!(r.multi_release);
    assert_eq!(r.versioned, versioned(&[(17, 61, 1), (21, 65, 1)]));
    assert_eq!(r.ignored_versioned, 0);
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.manifest, read_info(&jar).expect("info").manifest);
    assert_eq!(r.manifest, Some(parse_manifest(&manifest_bytes(MR))));
}

#[test]
fn manifest_after_versioned_classes_without_multi_release() {
    let dir = TempDir::new().expect("tempdir");
    let attrs = &[("Main-Class", "com.A")];
    let jar = make_jar(dir.path(), "a.jar", &manifest_last(attrs));
    let r = scan(&jar);
    assert!(!r.multi_release);
    assert!(r.versioned.is_empty());
    assert_eq!(r.ignored_versioned, 2);
    assert_eq!(r.base, counts(&[(52, 1)]));
    assert_eq!(r.manifest, Some(parse_manifest(&manifest_bytes(attrs))));

    let mut entries = manifest_last(MR);
    entries.retain(|(name, _)| *name != "META-INF/MANIFEST.MF");
    let jar = make_jar(dir.path(), "none.jar", &entries);
    let r = scan(&jar);
    assert!(!r.multi_release);
    assert_eq!(r.manifest, None);
    assert_eq!(r.ignored_versioned, 2);
    assert_eq!(r.base, counts(&[(52, 1)]));
}

#[test]
fn duplicated_manifest_last_record_wins() {
    let dir = TempDir::new().expect("tempdir");
    let plain = manifest_bytes(&[("Main-Class", "a.Main")]);
    let mr = manifest_bytes(MR);
    let class = RawEntry::new("META-INF/versions/17/a/A.class", class_bytes(0, 61));
    for (name, first, last, counted) in [
        ("mr-first.jar", &mr, &plain, false),
        ("mr-last.jar", &plain, &mr, true),
    ] {
        let bytes = raw_zip(
            &[
                RawEntry::new("META-INF/MANIFEST.MF", first),
                class.clone(),
                RawEntry::new("META-INF/MANIFEST.MF", last),
            ],
            &RawZip::default(),
        );
        let jar = write(dir.path(), name, &bytes);
        let r = scan(&jar);
        assert_eq!(r.multi_release, counted, "{name}");
        assert_eq!(r.manifest, Some(parse_manifest(last)), "{name}");
        assert_eq!(
            r.manifest,
            read_info(&jar).expect("info").manifest,
            "{name}"
        );
        if counted {
            assert_eq!(r.versioned, versioned(&[(17, 61, 1)]), "{name}");
            assert_eq!(r.ignored_versioned, 0, "{name}");
        } else {
            assert!(r.versioned.is_empty(), "{name}");
            assert_eq!(r.ignored_versioned, 1, "{name}");
        }

        // Without versioned classes the main pass alone picks the last record.
        let bytes = raw_zip(
            &[
                RawEntry::new("META-INF/MANIFEST.MF", first),
                RawEntry::new("a/A.class", class_bytes(0, 52)),
                RawEntry::new("META-INF/MANIFEST.MF", last),
            ],
            &RawZip::default(),
        );
        let jar = write(dir.path(), name, &bytes);
        let r = scan(&jar);
        assert_eq!(r.multi_release, counted, "{name}");
        assert_eq!(r.manifest, Some(parse_manifest(last)), "{name}");
    }
}

fn find_from(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    from + bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("needle")
}

// The first occurrence of a name is in its local header, which starts 30 bytes earlier
// (`raw_zip` writes the local name as is).
fn break_local_header_of(bytes: &mut [u8], name: &str) {
    let start = find_from(bytes, 0, name.as_bytes()) - 30;
    assert_eq!(&bytes[start..start + 4], b"PK\x03\x04");
    bytes[start + 2] = 0;
}

#[test]
fn ignored_versioned_classes_are_never_read() {
    let dir = TempDir::new().expect("tempdir");
    let name = "META-INF/versions/17/a/A.class";
    for (jar, attrs) in [("plain.jar", &[][..]), ("mr.jar", MR)] {
        let mut bytes = raw_zip(
            &[
                RawEntry::new("META-INF/MANIFEST.MF", manifest_bytes(attrs)),
                RawEntry::new("a/A.class", class_bytes(0, 52)),
                RawEntry::new(name, class_bytes(0, 61)),
            ],
            &RawZip::default(),
        );
        break_local_header_of(&mut bytes, name);
        let r = scan(&write(dir.path(), jar, &bytes));
        assert_eq!(r.base, counts(&[(52, 1)]), "{jar}");
        assert!(r.versioned.is_empty(), "{jar}");
        if attrs.is_empty() {
            assert_eq!(r.ignored_versioned, 1);
            assert!(r.invalid.is_empty(), "{:?}", r.invalid);
        } else {
            assert_eq!(r.ignored_versioned, 0);
            assert_eq!(
                r.invalid,
                [(
                    name.to_string(),
                    "invalid local header signature".to_string()
                )]
            );
        }
    }
}

fn report_entries(zip64: bool) -> Vec<RawEntry> {
    [
        ("META-INF/MANIFEST.MF", manifest_bytes(MR)),
        ("a/", Vec::new()),
        ("a/A.class", class_bytes(0, 52)),
        ("a/P.class", class_bytes(0xFFFF, 61)),
        ("module-info.class", class_bytes(0, 53)),
        ("META-INF/versions/17/a/A.class", class_bytes(0, 61)),
        ("a/Bad.class", vec![0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 52]),
    ]
    .into_iter()
    .map(|(name, data)| RawEntry {
        zip64,
        ..RawEntry::new(name, data)
    })
    .collect()
}

#[test]
fn prepended_data_stored_and_zip64_give_the_same_report() {
    let dir = TempDir::new().expect("tempdir");
    let plain = scan(&write(
        dir.path(),
        "plain.jar",
        &raw_zip(&report_entries(false), &RawZip::default()),
    ));
    assert_eq!(plain.base, counts(&[(52, 1), (61, 1)]));
    assert_eq!(plain.versioned, versioned(&[(17, 61, 1)]));
    assert_eq!(plain.preview, [("a/P.class".to_string(), 61)]);
    assert_eq!(plain.module_info, [("module-info.class".to_string(), 53)]);
    assert_eq!(
        plain.invalid,
        [("a/Bad.class".to_string(), "bad magic".to_string())]
    );

    let raw = report_entries(false);
    let named: Vec<(&str, Vec<u8>)> = raw
        .iter()
        .map(|e| (std::str::from_utf8(&e.name).expect("name"), e.data.clone()))
        .collect();
    let deflated = scan(&make_jar(dir.path(), "deflated.jar", &named));
    assert_eq!(deflated, plain, "deflated");
    let stored = scan(&write(dir.path(), "stored.jar", &stored_jar_bytes(&named)));
    assert_eq!(stored, plain, "stored");

    let script = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n".repeat(3);
    for (name, zip64, prefix) in [
        ("prefixed.jar", false, script.clone()),
        ("zip64.jar", true, Vec::new()),
        ("prefixed-zip64.jar", true, script),
    ] {
        let opts = RawZip {
            zip64,
            prefix,
            ..RawZip::default()
        };
        let r = scan(&write(
            dir.path(),
            name,
            &raw_zip(&report_entries(zip64), &opts),
        ));
        assert_eq!(r, plain, "{name}");
    }
}

#[test]
fn data_descriptors_give_the_right_versions() {
    let dir = TempDir::new().expect("tempdir");
    let mut big = class_bytes(0, 65);
    big.extend((0..20_000u32).flat_map(|i| i.to_le_bytes()));
    let entries = [
        ("META-INF/MANIFEST.MF", manifest_bytes(MR)),
        ("a/A.class", class_bytes(0, 52)),
        ("a/Big.class", big),
        ("META-INF/versions/17/a/A.class", class_bytes(0, 61)),
        ("a/C.class", class_bytes(0, 55)),
    ];
    let mut bytes = Vec::new();
    let mut zip = ZipWriter::new_stream(&mut bytes);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in &entries {
        zip.start_file(*name, opts).expect("file");
        zip.write_all(data).expect("write");
    }
    zip.finish().expect("finish");
    let mut cd = CentralDirectory::new(Cursor::new(&bytes)).expect("cdir");
    while let Some(e) = cd.next_entry().expect("entry") {
        assert_ne!(e.flags & 0x8, 0, "{} has no data descriptor", e.name);
    }

    let r = scan(&write(dir.path(), "streamed.jar", &bytes));
    assert!(r.multi_release);
    assert_eq!(r.base, counts(&[(52, 1), (55, 1), (65, 1)]));
    assert_eq!(r.versioned, versioned(&[(17, 61, 1)]));
    assert!(r.invalid.is_empty(), "{:?}", r.invalid);
}
