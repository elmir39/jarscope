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
    assert_error_for("info", path, needle);
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
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("jarscope {}\n", env!("CARGO_PKG_VERSION"))
    );
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
fn list_into_closed_stdout_exits_quietly() {
    use std::process::{Command as Std, Stdio};
    let dir = TempDir::new().expect("tempdir");
    let names: Vec<String> = (0..5000)
        .map(|i| format!("com/foo/Entry{i:05}.txt"))
        .collect();
    let entries: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), vec![])).collect();
    let jar = make_jar(dir.path(), "a.jar", &entries);
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
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
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
fn duplicate_names_are_listed_and_counted() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_duplicate(dir.path(), "d.jar", class_bytes(0, 52), class_bytes(0, 55));
    let run = |args: &[&str]| jarscope().args(args).arg(&jar).output().expect("run");

    for (args, want) in [
        (&["list"][..], "a/A.class\na/A.class\n"),
        (&["list", "--classes-only"][..], "a.A\na.A\n"),
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let out = run(&["info"]);
    assert_eq!(out.status.code(), Some(0));
    let size = 2 * class_bytes(0, 52).len();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.starts_with("Entries:") || l.starts_with("Uncompressed:"))
            .collect::<Vec<_>>(),
        [
            "Entries:       2 (2 classes)".to_string(),
            format!("Uncompressed:  {size} bytes"),
        ]
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run(&["version"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Requires: Java 11 (major 55)\n\nClasses by Java version:\n  Java 8   1\n  Java 11  1\n"
    );
}

// Each entry fails in a different place; `raw_zip` writes every entry stored, whatever its
// method field says, so `Corrupt.class` is 0xFF bytes read as deflate.
#[test]
fn version_lists_each_invalid_class_and_goes_on() {
    use common::{RawEntry, RawZip, raw_zip};
    let dir = TempDir::new().expect("tempdir");
    let class = class_bytes(0, 52);
    let entries = [
        RawEntry::new("a/A.class", &class),
        RawEntry::new("a/BadMagic.class", [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 52]),
        RawEntry::new("a/Short.class", [0xCA, 0xFE, 0xBA]),
        RawEntry {
            method: 8,
            ..RawEntry::new("a/Corrupt.class", [0xFF; 16])
        },
        RawEntry::new("a/BadLocal.class", &class),
        RawEntry {
            flags: 1,
            ..RawEntry::new("a/Encrypted.class", &class)
        },
        RawEntry {
            method: 12,
            ..RawEntry::new("a/Bzip2.class", &class)
        },
        RawEntry::new("a/BadCrc.class", [0xCA, 0xFE, 0xBA, 0xBE, 0]),
        RawEntry::new("a/B.class", class_bytes(0, 55)),
    ];
    let mut bytes = raw_zip(&entries, &RawZip::default());
    let find = |bytes: &[u8], from: usize, name: &[u8]| {
        from + bytes[from..]
            .windows(name.len())
            .position(|w| w == name)
            .expect("name")
    };
    let local = find(&bytes, 0, b"a/BadLocal.class") - 30;
    assert_eq!(&bytes[local..local + 4], b"PK\x03\x04");
    bytes[local + 2] = 0;
    // The second occurrence is the central header, 46 bytes after its start; CRC at +16.
    let first = find(&bytes, 0, b"a/BadCrc.class");
    let central = find(&bytes, first + 1, b"a/BadCrc.class") - 46;
    assert_eq!(&bytes[central..central + 4], b"PK\x01\x02");
    bytes[central + 16] ^= 0xFF;
    let jar = dir.path().join("bad.jar");
    std::fs::write(&jar, bytes).expect("write");

    let out = jarscope().arg("version").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        concat!(
            "Requires: Java 11 (major 55)\n\n",
            "Classes by Java version:\n  Java 8   1\n  Java 11  1\n\n",
            "Invalid class files: 7\n",
            "  a/BadMagic.class: bad magic\n",
            "  a/Short.class: truncated class header\n",
            "  a/Corrupt.class: deflate decompression error: invalid block type\n",
            "  a/BadLocal.class: invalid local header signature\n",
            "  a/Encrypted.class: encrypted entry\n",
            "  a/Bzip2.class: unsupported compression method 12\n",
            "  a/BadCrc.class: CRC-32 mismatch\n",
        )
    );
}

#[test]
fn central_directory_cut_in_the_middle_is_an_error() {
    let dir = TempDir::new().expect("tempdir");
    let jar = dir.path().join("cut.jar");
    std::fs::write(&jar, common::cut_jar_bytes()).expect("write");
    for args in [
        &["list"][..],
        &["list", "--classes-only"],
        &["info"],
        &["version"],
    ] {
        let out = jarscope().args(args).arg(&jar).output().expect("run");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!(
                "error: {} is not a valid JAR/ZIP file: truncated or corrupt central directory\n",
                jar.display()
            )
        );
    }
}

#[test]
fn list_and_info_on_prepended_and_zip64_jars() {
    use common::{RawEntry, RawZip, raw_zip};
    let dir = TempDir::new().expect("tempdir");
    let manifest = common::manifest_bytes(&[("Main-Class", "a.Main")]);
    let class = class_bytes(0, 52);
    let script = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n".to_vec();
    for (zip64, prefix) in [(false, script.clone()), (true, Vec::new()), (true, script)] {
        let entries: Vec<RawEntry> = [
            ("a/Main.class", class.clone()),
            ("META-INF/MANIFEST.MF", manifest.clone()),
            ("a/", Vec::new()),
        ]
        .into_iter()
        .map(|(n, d)| RawEntry {
            zip64,
            ..RawEntry::new(n, d)
        })
        .collect();
        let opts = RawZip {
            zip64,
            prefix,
            ..RawZip::default()
        };
        let jar = dir.path().join("x.jar");
        std::fs::write(&jar, raw_zip(&entries, &opts)).expect("write");

        let out = jarscope().arg("list").arg(&jar).output().expect("run");
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "META-INF/MANIFEST.MF\na/\na/Main.class\n"
        );
        let out = jarscope().arg("info").arg(&jar).output().expect("run");
        assert_eq!(out.status.code(), Some(0));
        let size = manifest.len() + class.len();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!(
                "Main-Class:     a.Main\nMulti-Release:  false\nEntries:        3 (1 class)\nCompressed:     {size} bytes\nUncompressed:   {size} bytes\n"
            )
        );
    }
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

#[test]
fn info_reports_manifest_counts_and_sizes() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = common::manifest_bytes(&[
        ("Main-Class", "com.example.Main"),
        ("Created-By", "test"),
        ("Multi-Release", "true"),
    ]);
    let class = class_bytes(0, 52);
    let readme = vec![b'x'; 1500];
    let jar = make_jar_with_manifest(
        dir.path(),
        "a.jar",
        &[
            ("Main-Class", "com.example.Main"),
            ("Created-By", "test"),
            ("Multi-Release", "true"),
        ],
        &[
            ("com/", Vec::new()),
            ("com/A.class", class.clone()),
            ("README", readme.clone()),
        ],
    );
    let total = manifest.len() + class.len() + readme.len();
    let out = jarscope().arg("info").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with(
        "Main-Class:     com.example.Main\nCreated-By:     test\nMulti-Release:  true\nEntries:        4 (1 class)\nCompressed:     "
    ), "{text}");
    assert!(
        text.ends_with(&format!(
            "Uncompressed:   1.7 KB (1,{:03} bytes)\n",
            total - 1000
        )),
        "{text}"
    );
}

#[test]
fn info_without_manifest() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(dir.path(), "a.jar", &[("A.class", class_bytes(0, 52))]);
    let out = jarscope().arg("info").arg(&jar).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("Manifest:      none\nEntries:       1 (1 class)\n"),
        "{text}"
    );
}

fn find_tree() -> (TempDir, String, String) {
    let dir = TempDir::new().expect("tempdir");
    let a = make_jar(
        dir.path(),
        "a.jar",
        &[("com/foo/Bar.class", class_bytes(0, 52))],
    );
    let b = make_jar(
        dir.path(),
        "b.jar",
        &[
            ("com/foo/Bar.class", class_bytes(0, 52)),
            ("com/foo/BarService.class", class_bytes(0, 52)),
        ],
    );
    make_jar(
        dir.path(),
        "c.jar",
        &[("com/Other.class", class_bytes(0, 52))],
    );
    (dir, a.display().to_string(), b.display().to_string())
}

#[test]
fn find_found_prints_paths() {
    let (dir, a, b) = find_tree();
    let out = jarscope()
        .args(["find", "com.foo.Bar"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{a}\n{b}\n"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("2 matches in 3 jars ("), "{err}");
}

#[test]
fn find_pattern_prints_classes() {
    let (dir, _, b) = find_tree();
    let out = jarscope()
        .args(["find", "com.foo.*Service"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{b}\n  com.foo.BarService\n")
    );
}

#[test]
fn find_not_found_exits_1() {
    let (dir, _, _) = find_tree();
    let out = jarscope()
        .args(["find", "com.Nope"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("0 matches in 3 jars ("), "{err}");
}

#[test]
fn find_fatal_errors_exit_2() {
    let dir = TempDir::new().expect("tempdir");
    let txt = dir.path().join("x.txt");
    std::fs::write(&txt, b"not a zip").expect("write");
    let cases: Vec<(&str, std::path::PathBuf)> = vec![
        ("", dir.path().to_path_buf()),
        (".class", dir.path().to_path_buf()),
        ("com.A", dir.path().join("missing")),
        ("com.A", txt),
    ];
    for (q, path) in cases {
        let out = jarscope()
            .args(["find", q])
            .arg(&path)
            .output()
            .expect("run");
        assert_eq!(out.status.code(), Some(2), "{q:?} {path:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.starts_with("error: "), "{err}");
    }
}

#[test]
fn find_corrupt_jar_warns_and_skips() {
    let (dir, a, b) = find_tree();
    std::fs::write(dir.path().join("bad.jar"), b"garbage garbage").expect("write");
    let out = jarscope()
        .args(["find", "com.foo.Bar"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{a}\n{b}\n"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("warning: "), "{err}");
    assert!(err.contains(", 1 skipped ("), "{err}");

    let out = jarscope()
        .args(["find", "com.Nope"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn find_nested_end_to_end() {
    let dir = TempDir::new().expect("tempdir");
    let inner = common::jar_bytes(&[("com/In.class", class_bytes(0, 52))]);
    let outer = make_jar(dir.path(), "outer.jar", &[("META-INF/jars/in.jar", inner)]);
    let out = jarscope()
        .args(["find", "--nested", "com.In"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}!/META-INF/jars/in.jar\n", outer.display())
    );
    let out = jarscope()
        .args(["find", "com.In"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn find_help_works() {
    let out = jarscope().args(["find", "--help"]).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--nested"));
}

#[test]
fn find_closed_stdout_and_stderr_do_not_panic() {
    use std::process::{Command as Std, Stdio};
    let dir = TempDir::new().expect("tempdir");
    for i in 0..200 {
        make_jar(
            dir.path(),
            &format!("j{i}.jar"),
            &[("com/A.class", class_bytes(0, 52))],
        );
        std::fs::write(dir.path().join(format!("bad{i}.jar")), b"garbage").expect("write");
    }
    let bin = assert_cmd::cargo::cargo_bin("jarscope");
    let mut child = Std::new(bin)
        .args(["find", "com.A"])
        .arg(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stdout.take());
    drop(child.stderr.take());
    let status = child.wait().expect("wait");
    assert_ne!(status.code(), Some(101));
    assert!(status.code().is_some());
}
