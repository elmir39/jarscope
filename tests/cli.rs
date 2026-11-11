mod common;

use std::path::Path;

use assert_cmd::Command;
use common::{class_bytes, make_jar, make_jar_with_duplicate, make_jar_with_manifest};
use tempfile::TempDir;

fn jarscope() -> Command {
    Command::cargo_bin("jarscope").expect("binary")
}

fn assert_error(path: &Path, needle: Option<&str>) {
    assert_error_for("version", path, needle);
    assert_error_for("list", path, needle);
}

fn assert_error_for(cmd: &str, path: &Path, needle: Option<&str>) {
    let out = jarscope().arg(cmd).arg(path).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error: "), "{err}");
    if let Some(n) = needle {
        assert!(err.contains(n), "{err}");
    }
}

#[test]
fn version_full_report() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[("Multi-Release", "true")],
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/P.class", class_bytes(0xFFFF, 61)),
            ("com/Broken.class", vec![0u8; 16]),
            ("META-INF/versions/21/com/B.class", class_bytes(0, 65)),
            ("META-INF/versions/9/module-info.class", class_bytes(0, 53)),
        ],
    );
    let out = jarscope().arg("version").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("Requires: Java 17 (major 61)\n"), "{text}");
    assert!(text.contains("Multi-release (Multi-Release: true):\n"));
    assert!(text.contains("META-INF/versions/21/   1 class (Java 21)"));
    assert!(text.contains("module-info.class: Java 9 (META-INF/versions/9/module-info.class)"));
    assert!(text.contains("Preview classes: 1 — need exactly Java 17 with --enable-preview"));
    assert!(text.contains("Invalid class files: 1\n  com/Broken.class: "));
}

#[test]
fn missing_file() {
    assert_error(Path::new("does-not-exist.jar"), Some("cannot open"));
}

#[test]
fn non_zip_file() {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path().join("x.jar");
    std::fs::write(&p, b"this is not a zip file at all").expect("write");
    assert_error(&p, None);
}

#[test]
fn empty_file() {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path().join("e.jar");
    std::fs::write(&p, b"").expect("write");
    assert_error(&p, None);
}

#[test]
fn truncated_jar() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "t.jar",
        &[
            ("com/A.class", class_bytes(0, 52)),
            ("com/B.class", class_bytes(0, 52)),
        ],
    );
    let bytes = std::fs::read(&jar).expect("read");
    std::fs::write(&jar, &bytes[..bytes.len() / 2]).expect("write");
    assert_error(&jar, None);
}

#[test]
fn version_flag() {
    let out = jarscope().arg("--version").output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "jarscope 0.1.0\n");
}

#[test]
fn help_lists_version_subcommand() {
    let out = jarscope().arg("--help").output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("version"));
}

#[test]
fn closed_stdout_does_not_panic() {
    use std::process::{Command as Std, Stdio};
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(dir.path(), "a.jar", &[("com/A.class", class_bytes(0, 52))]);
    let bin = assert_cmd::cargo::cargo_bin("jarscope");
    let mut child = Std::new(bin)
        .arg("version")
        .arg(&jar)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stdout.take());
    let status = child.wait().expect("wait");
    assert_ne!(status.code(), Some(101));
}

#[test]
fn list_prints_entries_in_order() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/foo/bar/X.class", class_bytes(0, 52)),
            ("com/foo/baz.txt", b"t".to_vec()),
            ("com/foo/", vec![]),
            ("com/foo/Bar.class", class_bytes(0, 52)),
        ],
    );
    let out = jarscope().arg("list").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "com/foo/\ncom/foo/Bar.class\ncom/foo/baz.txt\ncom/foo/bar/X.class\n"
    );
    assert!(out.stderr.is_empty());
}

#[test]
fn list_classes_only() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("com/foo/Bar.class", class_bytes(0, 52)),
            ("README.md", b"x".to_vec()),
            ("META-INF/versions/17/com/foo/Bar.class", class_bytes(0, 61)),
        ],
    );
    let out = jarscope()
        .args(["list", "--classes-only"])
        .arg(&jar)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "com.foo.Bar\ncom.foo.Bar (META-INF/versions/17)\n"
    );
}

#[test]
fn list_duplicate_names_warns_on_stderr() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_duplicate(dir.path(), "d.jar", class_bytes(0, 52), class_bytes(0, 55));
    let out = jarscope().arg("list").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a/A.class\n");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning: ")
            && err.contains("has duplicate entry names; each name is listed once"),
        "{err}"
    );
}

#[test]
fn list_empty_jar() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(dir.path(), "e.jar", &[]);
    let out = jarscope().arg("list").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn list_help() {
    let out = jarscope().args(["list", "--help"]).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--classes-only"));
}

#[test]
fn list_closed_stdout_exits_quietly() {
    use std::process::{Command as Std, Stdio};
    let dir = TempDir::new().expect("tempdir");
    let names: Vec<String> = (0..20000).map(|i| format!("p/C{i}.class")).collect();
    let entries: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), Vec::new())).collect();
    let jar = make_jar(dir.path(), "big.jar", &entries);
    let bin = assert_cmd::cargo::cargo_bin("jarscope");
    let mut child = Std::new(bin)
        .arg("list")
        .arg(&jar)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stdout.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
