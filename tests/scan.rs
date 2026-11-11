mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{class_bytes, make_jar, make_jar_with_manifest};
use jarscope::jar::{JarReport, scan_jar};
use tempfile::TempDir;

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
