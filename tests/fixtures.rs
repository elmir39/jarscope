use std::path::{Path, PathBuf};

use assert_cmd::Command;
use jarscope::jar::scan_jar;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn plain_releases() {
    for (name, major) in [
        ("release8.jar", 52),
        ("release17.jar", 61),
        ("release21.jar", 65),
        ("release25.jar", 69),
    ] {
        let r = scan_jar(&fixture(name)).expect(name);
        assert_eq!(r.required_major(), Some(major), "{name}");
        assert!(r.preview.is_empty(), "{name}");
        assert!(r.invalid.is_empty(), "{name}");
        assert!(!r.multi_release, "{name}");
    }
}

#[test]
fn preview_class() {
    let r = scan_jar(&fixture("preview25.jar")).expect("scan");
    assert_eq!(r.required_major(), Some(69));
    assert_eq!(r.preview, vec![("fixture/P.class".to_string(), 69)]);
    assert!(r.invalid.is_empty());
}

#[test]
fn multi_release_jar() {
    let r = scan_jar(&fixture("multi-release.jar")).expect("scan");
    assert!(r.multi_release);
    assert_eq!(r.required_major(), Some(52));
    assert_eq!(r.versioned.keys().copied().collect::<Vec<_>>(), vec![17]);
    assert!(r.versioned[&17].keys().all(|&m| m == 61));
    assert_eq!(
        r.module_info,
        vec![("META-INF/versions/9/module-info.class".to_string(), 53)]
    );
    assert!(r.invalid.is_empty());
}

#[test]
fn cli_multi_release() {
    let out = Command::cargo_bin("jarscope")
        .expect("binary")
        .arg("version")
        .arg(fixture("multi-release.jar"))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("Requires: Java 8 (major 52)\n"), "{text}");
    assert!(
        text.contains("Multi-release (Multi-Release: true):\n"),
        "{text}"
    );
    assert!(
        text.contains("META-INF/versions/17/   2 classes (Java 17)"),
        "{text}"
    );
    assert!(
        text.contains("module-info.class: Java 9 (META-INF/versions/9/module-info.class)"),
        "{text}"
    );
}

fn run(args: &[&str], jar: &str) -> String {
    let out = Command::cargo_bin("jarscope")
        .expect("binary")
        .args(args)
        .arg(fixture(jar))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

#[test]
fn list_multi_release() {
    assert_eq!(
        run(&["list"], "multi-release.jar"),
        "\
META-INF/
META-INF/MANIFEST.MF
META-INF/versions/17/
META-INF/versions/17/fixture/
META-INF/versions/17/fixture/M$Named.class
META-INF/versions/17/fixture/M.class
META-INF/versions/9/module-info.class
fixture/
fixture/M.class
"
    );
}

#[test]
fn list_classes_only_multi_release() {
    assert_eq!(
        run(&["list", "--classes-only"], "multi-release.jar"),
        "\
fixture.M
module-info (META-INF/versions/9)
fixture.M (META-INF/versions/17)
fixture.M$Named (META-INF/versions/17)
"
    );
}

#[test]
fn info_release8() {
    assert_eq!(
        run(&["info"], "release8.jar"),
        "\
Created-By:     25 (Oracle Corporation)
Multi-Release:  false
Entries:        4 (1 class)
Compressed:     251 bytes
Uncompressed:   302 bytes
"
    );
}

#[test]
fn info_multi_release() {
    assert_eq!(
        run(&["info"], "multi-release.jar"),
        "\
Created-By:     25 (Oracle Corporation)
Multi-Release:  true
Entries:        9 (4 classes)
Compressed:     1.3 KB (1,293 bytes)
Uncompressed:   2.1 KB (2,116 bytes)
"
    );
}
