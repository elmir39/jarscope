mod common;

use std::path::{Path, PathBuf};

use common::{
    RawEntry, RawZip, break_local_header_bytes, break_local_headers, class_bytes, cut_jar_bytes,
    jar_bytes, make_jar, make_jar_with_duplicate, make_jar_with_manifest, manifest_bytes, raw_zip,
    stored_jar_bytes,
};
use jarscope::find::{
    ClassLocation, JarMatch, JarSearch, Location, Query, find, find_jars, search_jar,
};
use jarscope::render::render_find;
use tempfile::TempDir;

fn class() -> Vec<u8> {
    class_bytes(0, 52)
}

fn search(path: &Path, query: &str, nested: bool) -> JarSearch {
    search_jar(path, &Query::parse(query).expect("query"), nested).expect("search_jar")
}

fn search_err(path: &Path) -> String {
    let query = Query::parse("a.B").expect("query");
    format!(
        "{:#}",
        search_jar(path, &query, false).expect_err("should fail")
    )
}

fn paths(s: &JarSearch) -> Vec<&str> {
    s.matches.iter().map(|m| m.path.as_str()).collect()
}

fn found(path: &str, classes: &[(&str, Location)]) -> JarMatch {
    JarMatch {
        path: path.to_string(),
        classes: classes
            .iter()
            .map(|&(name, location)| ClassLocation {
                name: name.to_string(),
                location,
            })
            .collect(),
    }
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

fn write(dir: &Path, name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write jar");
    path
}

#[test]
fn exact_match_and_no_match() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[("com/foo/", vec![]), ("com/foo/Bar.class", class())],
    );
    let s = search(&jar, "com.foo.Bar", false);
    assert_eq!(
        s.matches,
        [found(&shown(&jar), &[("com.foo.Bar", Location::Base)])]
    );
    assert!(s.warnings.is_empty());
    assert_eq!(search(&jar, "com.foo.Baz", false), JarSearch::default());
}

#[test]
fn non_class_entries_never_match() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[("com/foo/Bar.txt", vec![]), ("com/foo/Bar.class/", vec![])],
    );
    for q in ["com.foo.Bar", "**"] {
        assert_eq!(search(&jar, q, false), JarSearch::default(), "{q}");
    }
}

#[test]
fn all_locations_match_with_or_without_multi_release() {
    let dir = TempDir::new().expect("tempdir");
    let entries = [
        ("WEB-INF/classes/com/foo/Bar.class", class()),
        ("META-INF/versions/17/com/foo/Bar.class", class()),
        ("BOOT-INF/classes/com/foo/Bar.class", class()),
        ("META-INF/versions/9/com/foo/Bar.class", class()),
        ("com/foo/Bar.class", class()),
    ];
    let with = make_jar_with_manifest(dir.path(), "mr.jar", &[("Multi-Release", "true")], &entries);
    let without = make_jar(dir.path(), "plain.jar", &entries);
    for jar in [with, without] {
        let s = search(&jar, "com/foo/Bar.class", false);
        assert_eq!(
            s.matches,
            [found(
                &shown(&jar),
                &[
                    ("com.foo.Bar", Location::Base),
                    ("com.foo.Bar", Location::Versioned(9)),
                    ("com.foo.Bar", Location::Versioned(17)),
                    ("com.foo.Bar", Location::BootInf),
                    ("com.foo.Bar", Location::WebInf),
                ]
            )]
        );
    }
}

#[test]
fn pattern_returns_sorted_classes() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("BOOT-INF/classes/com/foo/WebService.class", class()),
            ("com/foo/UserService.class", class()),
            ("META-INF/versions/17/com/foo/UserService.class", class()),
            ("META-INF/versions/9/com/foo/UserService.class", class()),
            ("com/foo/bar/DeepService.class", class()),
            ("com/foo/OrderService.class", class()),
            ("com/foo/Outer$InnerService.class", class()),
            ("com/foo/UserServiceImpl.class", class()),
            ("com/foo/Other.class", class()),
        ],
    );
    let s = search(&jar, "com.foo.*Service", false);
    assert_eq!(
        s.matches,
        [found(
            &shown(&jar),
            &[
                ("com.foo.OrderService", Location::Base),
                ("com.foo.Outer$InnerService", Location::Base),
                ("com.foo.UserService", Location::Base),
                ("com.foo.UserService", Location::Versioned(9)),
                ("com.foo.UserService", Location::Versioned(17)),
                ("com.foo.WebService", Location::BootInf),
            ]
        )]
    );
    let deep = search(&jar, "com.foo.**Service", false);
    let names: Vec<_> = deep.matches[0]
        .classes
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "com.foo.OrderService",
            "com.foo.Outer$InnerService",
            "com.foo.UserService",
            "com.foo.bar.DeepService",
            "com.foo.UserService",
            "com.foo.UserService",
            "com.foo.WebService",
        ]
    );
}

fn outer_with(dir: &Path, prefix: &str, stored: bool) -> PathBuf {
    let inner_entries = [("com/foo/Bar.class", class())];
    let inner = if stored {
        stored_jar_bytes(&inner_entries)
    } else {
        jar_bytes(&inner_entries)
    };
    make_jar(
        dir,
        "outer.jar",
        &[
            ("com/foo/Other.class", class()),
            (&format!("{prefix}inner.jar"), inner),
        ],
    )
}

fn assert_nested_found(prefix: &str, stored: bool) {
    let dir = TempDir::new().expect("tempdir");
    let jar = outer_with(dir.path(), prefix, stored);
    let s = search(&jar, "com.foo.Bar", true);
    assert_eq!(
        s.matches,
        [found(
            &format!("{}!/{prefix}inner.jar", shown(&jar)),
            &[("com.foo.Bar", Location::Base)]
        )],
        "{prefix}"
    );
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert_eq!(search(&jar, "com.foo.Bar", false), JarSearch::default());
}

#[test]
fn nested_meta_inf_jars() {
    assert_nested_found("META-INF/jars/", false);
}

#[test]
fn nested_meta_inf_jarjar() {
    assert_nested_found("META-INF/jarjar/", false);
}

#[test]
fn nested_boot_inf_lib_stored() {
    assert_nested_found("BOOT-INF/lib/", true);
}

#[test]
fn nested_web_inf_lib() {
    assert_nested_found("WEB-INF/lib/", false);
}

#[test]
fn nested_subdirectory_and_uppercase_extension() {
    let dir = TempDir::new().expect("tempdir");
    let inner = jar_bytes(&[("com/foo/Bar.class", class())]);
    let jar = make_jar(dir.path(), "outer.jar", &[("BOOT-INF/lib/x/IN.JAR", inner)]);
    let s = search(&jar, "com.foo.Bar", true);
    assert_eq!(
        paths(&s),
        [format!("{}!/BOOT-INF/lib/x/IN.JAR", shown(&jar))]
    );
}

#[test]
fn other_folders_are_never_opened() {
    let dir = TempDir::new().expect("tempdir");
    let jar = outer_with(dir.path(), "META-INF/other/", false);
    assert_eq!(search(&jar, "com.foo.Bar", true), JarSearch::default());
}

#[test]
fn nested_depth_limit() {
    let dir = TempDir::new().expect("tempdir");
    let mut inner: Option<(&str, Vec<u8>)> = None;
    for name in ["e", "d", "c", "b"] {
        let mut entries = vec![(format!("p{name}/X.class"), class())];
        if let Some((child, bytes)) = inner.take() {
            entries.push((format!("META-INF/jars/{child}.jar"), bytes));
        }
        let refs: Vec<_> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), d.clone()))
            .collect();
        inner = Some((name, jar_bytes(&refs)));
    }
    let jar = make_jar(
        dir.path(),
        "a.jar",
        &[
            ("pa/X.class", class()),
            ("META-INF/jars/b.jar", inner.expect("b.jar").1),
        ],
    );
    let s = search(&jar, "**.X", true);
    let a = shown(&jar);
    let b = format!("{a}!/META-INF/jars/b.jar");
    let c = format!("{b}!/META-INF/jars/c.jar");
    let d = format!("{c}!/META-INF/jars/d.jar");
    assert_eq!(paths(&s), [&a, &b, &c, &d]);
    let classes: Vec<_> = s
        .matches
        .iter()
        .map(|m| m.classes[0].name.as_str())
        .collect();
    assert_eq!(classes, ["pa.X", "pb.X", "pc.X", "pd.X"]);
    assert_eq!(
        s.warnings,
        [format!(
            "{d}!/META-INF/jars/e.jar: nested deeper than 3 levels, not searched"
        )]
    );
}

#[test]
fn bad_nested_jar_is_a_warning() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "outer.jar",
        &[
            ("com/foo/Bar.class", class()),
            ("META-INF/jars/bad.jar", b"not a zip".to_vec()),
        ],
    );
    let s = search(&jar, "com.foo.Bar", true);
    assert_eq!(paths(&s), [shown(&jar)]);
    assert_eq!(s.warnings.len(), 1);
    let expected = format!(
        "{}!/META-INF/jars/bad.jar is not a valid JAR/ZIP file",
        shown(&jar)
    );
    assert!(s.warnings[0].starts_with(&expected), "{:?}", s.warnings);
}

#[test]
fn reads_only_the_central_directory() {
    let dir = TempDir::new().expect("tempdir");
    // The nested JAR's own headers are broken too, so the outer JAR has no stray signatures.
    let mut inner = stored_jar_bytes(&[("com/foo/Inner.class", class())]);
    assert_eq!(break_local_header_bytes(&mut inner), 1);
    let jar = write(
        dir.path(),
        "a.jar",
        stored_jar_bytes(&[
            (
                "META-INF/MANIFEST.MF",
                manifest_bytes(&[("Multi-Release", "true")]),
            ),
            ("com/foo/Bar.class", class()),
            ("com/foo/BazService.class", class()),
            ("META-INF/versions/11/com/foo/Bar.class", class()),
            ("META-INF/jars/inner.jar", inner),
        ]),
    );
    break_local_headers(&jar);

    let s = search(&jar, "com.foo.Bar", false);
    assert_eq!(
        s.matches,
        [found(
            &shown(&jar),
            &[
                ("com.foo.Bar", Location::Base),
                ("com.foo.Bar", Location::Versioned(11)),
            ]
        )]
    );
    assert!(s.warnings.is_empty());
    let s = search(&jar, "com.foo.*", false);
    assert_eq!(s.matches[0].classes.len(), 3);
    assert!(s.warnings.is_empty());

    let s = search(&jar, "com.foo.Bar", true);
    assert_eq!(paths(&s), [shown(&jar)]);
    assert_eq!(s.warnings.len(), 1);
    let expected = format!("cannot read {}!/META-INF/jars/inner.jar", shown(&jar));
    assert!(s.warnings[0].starts_with(&expected), "{:?}", s.warnings);
}

#[test]
fn missing_file_is_cannot_open() {
    let dir = TempDir::new().expect("tempdir");
    let msg = search_err(&dir.path().join("missing.jar"));
    assert!(msg.contains("cannot open"), "{msg}");
}

#[test]
fn empty_and_non_zip_files_are_not_valid() {
    let dir = TempDir::new().expect("tempdir");
    let empty = write(dir.path(), "empty.jar", vec![]);
    let text = write(dir.path(), "text.jar", b"hello, not a zip".to_vec());
    for path in [empty, text] {
        let msg = search_err(&path);
        assert!(msg.contains("is not a valid JAR/ZIP file"), "{msg}");
    }
}

fn jar(dir: &Path, rel: &str, class_entry: &str) -> PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, jar_bytes(&[(class_entry, class())])).expect("write jar");
    path
}

fn rel(root: &Path, jars: &[PathBuf]) -> Vec<String> {
    jars.iter()
        .map(|p| {
            p.strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn walk_finds_only_jar_files() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    for r in [
        "a.jar",
        "sub/b.JAR",
        "sub/deep/c.jar",
        "x.jar/e.jar",
        ".hidden/d.jar",
    ] {
        jar(root, r, "a/B.class");
    }
    std::fs::write(root.join("notes.txt"), b"hi").expect("write");
    let (jars, warnings) = find_jars(root).expect("find_jars");
    assert!(warnings.is_empty());
    assert_eq!(
        rel(root, &jars),
        [
            ".hidden/d.jar",
            "a.jar",
            "sub/b.JAR",
            "sub/deep/c.jar",
            "x.jar/e.jar"
        ]
    );
}

#[test]
fn walk_order_is_bytewise() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    jar(root, "p/a.jar", "a/B.class");
    jar(root, "p-bin/a.jar", "a/B.class");
    let (jars, _) = find_jars(root).expect("find_jars");
    assert_eq!(rel(root, &jars), ["p-bin/a.jar", "p/a.jar"]);
}

#[cfg(unix)]
#[test]
fn symlinks_are_handled() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("root");
    let real = jar(&root, "a.jar", "a/B.class");
    symlink(&root, root.join("loop")).expect("symlink dir");
    symlink(&real, root.join("link.jar")).expect("symlink file");
    let (jars, _) = find_jars(&root).expect("find_jars");
    assert_eq!(rel(&root, &jars), ["a.jar", "link.jar"]);
    let report = find(&root, &Query::parse("a.B").expect("query"), false).expect("find");
    assert_eq!(report.matches.len(), 2);

    let via = dir.path().join("via");
    symlink(&root, &via).expect("symlink root");
    let (jars, _) = find_jars(&via).expect("find_jars");
    assert_eq!(jars.len(), 2);
}

#[cfg(unix)]
#[test]
fn unreadable_subdirectory_is_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    jar(root, "a.jar", "a/B.class");
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("mkdir");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let readable = std::fs::read_dir(&locked).is_ok();
    let result = find_jars(root);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    if readable {
        return;
    }
    let (jars, warnings) = result.expect("find_jars");
    assert_eq!(rel(root, &jars), ["a.jar"]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("locked"), "{warnings:?}");
}

#[test]
fn single_file_root() {
    let dir = TempDir::new().expect("tempdir");
    let good = jar(dir.path(), "a.jar", "a/B.class");
    let query = Query::parse("a.B").expect("query");
    let report = find(&good, &query, false).expect("find");
    assert_eq!((report.matches.len(), report.jars), (1, 1));

    let bad = write(dir.path(), "bad.txt", b"nope".to_vec());
    assert!(find(&bad, &query, false).is_err());
    assert!(find(&dir.path().join("missing"), &query, false).is_err());
}

#[test]
fn corrupt_jar_is_skipped_with_warning() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    jar(root, "good.jar", "a/B.class");
    write(root, "bad.jar", b"not a zip".to_vec());
    let query = Query::parse("a.B").expect("query");
    let report = find(root, &query, false).expect("find");
    assert_eq!((report.jars, report.skipped), (2, 1));
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("bad.jar"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn parallel_results_are_deterministic() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    for i in 0..40 {
        jar(root, &format!("d{}/j{i:02}.jar", i % 5), "a/B.class");
    }
    let query = Query::parse("a.B").expect("query");
    let first = find(root, &query, false).expect("find");
    let sorted: Vec<_> = first.matches.iter().map(|m| m.path.clone()).collect();
    let mut expect = sorted.clone();
    expect.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(sorted, expect);
    for _ in 0..5 {
        assert_eq!(find(root, &query, false).expect("find"), first);
    }
}

fn printed(root: &Path, query: &str, nested: bool) -> String {
    let query = Query::parse(query).expect("query");
    render_find(&find(root, &query, nested).expect("find"), &query)
}

#[test]
fn duplicate_class_is_printed_once() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar_with_duplicate(dir.path(), "dup.jar", class(), class());
    let path = shown(&jar);
    assert_eq!(printed(&jar, "a.A", false), format!("{path}\n"));
    assert_eq!(printed(&jar, "a.*", false), format!("{path}\n  a.A\n"));
    assert_eq!(
        search(&jar, "a.A", false).matches,
        [found(&path, &[("a.A", Location::Base)])]
    );
}

// Renames `META-INF/jars/b.jar` to `META-INF/jars/a.jar` in place, as `make_jar_with_duplicate`.
#[test]
fn duplicate_nested_jar_is_searched_once_using_the_last_record() {
    let dir = TempDir::new().expect("tempdir");
    let mut bytes = stored_jar_bytes(&[
        (
            "META-INF/jars/a.jar",
            stored_jar_bytes(&[("com/First.class", class())]),
        ),
        (
            "META-INF/jars/b.jar",
            stored_jar_bytes(&[("com/Last.class", class())]),
        ),
    ]);
    let (from, to) = (b"META-INF/jars/b.jar", b"META-INF/jars/a.jar");
    let at: Vec<_> = (0..=bytes.len() - from.len())
        .filter(|&i| &bytes[i..i + from.len()] == from)
        .collect();
    assert_eq!(at.len(), 2, "one local and one central header");
    for i in at {
        bytes[i..i + to.len()].copy_from_slice(to);
    }
    let jar = write(dir.path(), "outer.jar", bytes);
    assert_eq!(
        printed(&jar, "**", true),
        format!("{}!/META-INF/jars/a.jar\n  com.Last\n", shown(&jar))
    );
}

const SCRIPT: &[u8] = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n";

fn raw_class(name: &str) -> RawEntry {
    RawEntry::new(name, class())
}

#[test]
fn prepended_data_jar_is_found_at_top_level_and_nested() {
    let dir = TempDir::new().expect("tempdir");
    let prefixed = RawZip {
        prefix: SCRIPT.to_vec(),
        ..RawZip::default()
    };
    let inner = raw_zip(&[raw_class("com/In.class")], &prefixed);
    let jar = write(
        dir.path(),
        "app.jar",
        raw_zip(
            &[
                raw_class("BOOT-INF/classes/com/App.class"),
                RawEntry::new("BOOT-INF/lib/in.jar", inner),
            ],
            &prefixed,
        ),
    );
    let path = shown(&jar);
    assert_eq!(
        search(&jar, "com.App", false).matches,
        [found(&path, &[("com.App", Location::BootInf)])]
    );
    let s = search(&jar, "com.*", true);
    assert_eq!(
        s.matches,
        [
            found(&path, &[("com.App", Location::BootInf)]),
            found(
                &format!("{path}!/BOOT-INF/lib/in.jar"),
                &[("com.In", Location::Base)]
            ),
        ]
    );
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
}

#[test]
fn zip64_jar_is_found_at_top_level_and_nested() {
    let dir = TempDir::new().expect("tempdir");
    let zip64 = RawZip {
        zip64: true,
        ..RawZip::default()
    };
    let entry = |e: RawEntry| RawEntry { zip64: true, ..e };
    let inner = raw_zip(&[entry(raw_class("com/In.class"))], &zip64);
    let jar = write(
        dir.path(),
        "big.jar",
        raw_zip(
            &[
                entry(raw_class("com/Out.class")),
                entry(RawEntry::new("META-INF/jars/in.jar", inner)),
            ],
            &zip64,
        ),
    );
    let path = shown(&jar);
    let s = search(&jar, "com.*", true);
    assert_eq!(
        s.matches,
        [
            found(&path, &[("com.Out", Location::Base)]),
            found(
                &format!("{path}!/META-INF/jars/in.jar"),
                &[("com.In", Location::Base)]
            ),
        ]
    );
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
}

#[test]
fn central_directory_cut_in_the_middle_skips_the_jar() {
    let dir = TempDir::new().expect("tempdir");
    let good = jar(dir.path(), "good.jar", "a/B.class");
    let bad = write(dir.path(), "bad.jar", cut_jar_bytes());
    let msg = search_err(&bad);
    assert_eq!(
        msg,
        format!(
            "{} is not a valid JAR/ZIP file: truncated or corrupt central directory",
            shown(&bad)
        )
    );

    let report = find(dir.path(), &Query::parse("**").expect("query"), true).expect("find");
    assert_eq!((report.jars, report.skipped), (2, 1));
    assert_eq!(
        report.matches,
        [found(&shown(&good), &[("a.B", Location::Base)])]
    );
    assert_eq!(report.warnings, [msg]);
}

#[test]
fn nested_central_directory_cut_in_the_middle_is_a_warning() {
    let dir = TempDir::new().expect("tempdir");
    let jar = make_jar(
        dir.path(),
        "outer.jar",
        &[
            ("a/Outer.class", class()),
            ("META-INF/jars/cut.jar", cut_jar_bytes()),
        ],
    );
    let s = search(&jar, "**", true);
    assert_eq!(
        s.matches,
        [found(&shown(&jar), &[("a.Outer", Location::Base)])]
    );
    assert_eq!(
        s.warnings,
        [format!(
            "{}!/META-INF/jars/cut.jar is not a valid JAR/ZIP file: truncated or corrupt central directory",
            shown(&jar)
        )]
    );
}

#[test]
fn nested_encrypted_or_unsupported_method_is_a_warning() {
    let dir = TempDir::new().expect("tempdir");
    let inner = || stored_jar_bytes(&[("com/In.class", class())]);
    let jar = write(
        dir.path(),
        "outer.jar",
        raw_zip(
            &[
                raw_class("com/Out.class"),
                RawEntry {
                    flags: 1,
                    ..RawEntry::new("META-INF/jars/enc.jar", inner())
                },
                RawEntry {
                    method: 12,
                    ..RawEntry::new("META-INF/jars/m12.jar", inner())
                },
            ],
            &RawZip::default(),
        ),
    );
    let path = shown(&jar);
    let s = search(&jar, "com.*", true);
    assert_eq!(s.matches, [found(&path, &[("com.Out", Location::Base)])]);
    assert_eq!(
        s.warnings,
        [
            format!("cannot read {path}!/META-INF/jars/enc.jar: encrypted entry"),
            format!("cannot read {path}!/META-INF/jars/m12.jar: unsupported compression method 12"),
        ]
    );
}
