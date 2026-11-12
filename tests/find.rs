mod common;

use std::path::{Path, PathBuf};

use common::{
    break_local_header_bytes, break_local_headers, class_bytes, jar_bytes, make_jar,
    make_jar_with_manifest, manifest_bytes, stored_jar_bytes,
};
use jarscope::find::{ClassLocation, JarMatch, JarSearch, Location, Query, search_jar};
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
