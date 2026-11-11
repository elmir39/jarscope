use crate::jar::{VERSIONS_PREFIX, parse_release};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassName {
    pub name: String,
    pub release: Option<u32>,
}

pub fn class_name(entry: &str) -> Option<ClassName> {
    if entry.ends_with('/') {
        return None;
    }
    let path = entry.strip_suffix(".class")?;
    let versioned = path
        .strip_prefix(VERSIONS_PREFIX)
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(n, rest)| Some((parse_release(n)?, rest)));
    let (release, path) = match versioned {
        Some((n, rest)) => (Some(n), rest),
        None => (None, path),
    };
    Some(ClassName {
        name: path.replace('/', "."),
        release,
    })
}

fn list_key(name: &str) -> (&str, bool, &str) {
    if name.ends_with('/') {
        return (name, false, name);
    }
    let group = name.rfind('/').map_or("", |i| &name[..=i]);
    (group, true, name)
}

pub fn sort_for_list(names: &mut [&str]) {
    names.sort_by(|a, b| list_key(a).cmp(&list_key(b)));
}

fn class_key(c: &ClassName) -> (Option<u32>, &str, &str) {
    let (package, simple) = c.name.rsplit_once('.').unwrap_or(("", &c.name));
    (c.release, package, simple)
}

// `None < Some(_)` puts base classes first; releases compare numerically.
pub fn sort_classes(names: &mut [ClassName]) {
    names.sort_by(|a, b| class_key(a).cmp(&class_key(b)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cn(entry: &str) -> Option<(String, Option<u32>)> {
        class_name(entry).map(|c| (c.name, c.release))
    }

    fn some(name: &str, release: Option<u32>) -> Option<(String, Option<u32>)> {
        Some((name.to_string(), release))
    }

    #[test]
    fn plain_names() {
        assert_eq!(cn("com/foo/Bar.class"), some("com.foo.Bar", None));
        assert_eq!(
            cn("com/foo/Outer$Inner.class"),
            some("com.foo.Outer$Inner", None)
        );
        assert_eq!(cn("Bar.class"), some("Bar", None));
        assert_eq!(cn("module-info.class"), some("module-info", None));
        assert_eq!(
            cn("com/foo/package-info.class"),
            some("com.foo.package-info", None)
        );
    }

    #[test]
    fn versioned_names() {
        assert_eq!(
            cn("META-INF/versions/17/com/foo/Bar.class"),
            some("com.foo.Bar", Some(17))
        );
        assert_eq!(
            cn("META-INF/versions/9/module-info.class"),
            some("module-info", Some(9))
        );
    }

    #[test]
    fn invalid_release_and_boot_inf_are_plain() {
        assert_eq!(
            cn("META-INF/versions/8/X.class"),
            some("META-INF.versions.8.X", None)
        );
        assert_eq!(
            cn("BOOT-INF/classes/com/foo/Bar.class"),
            some("BOOT-INF.classes.com.foo.Bar", None)
        );
    }

    #[test]
    fn non_classes() {
        for n in ["com/foo/", "x.class/", "README.md", "META-INF/MANIFEST.MF"] {
            assert_eq!(class_name(n), None, "{n}");
        }
    }

    #[test]
    fn list_order_groups_packages() {
        let mut v = [
            "com/foo/bar/X.class",
            "com/foo/baz.txt",
            "com/foo/",
            "com/foo/Bar.class",
            "A.txt",
        ];
        sort_for_list(&mut v);
        assert_eq!(
            v,
            [
                "A.txt",
                "com/foo/",
                "com/foo/Bar.class",
                "com/foo/baz.txt",
                "com/foo/bar/X.class"
            ]
        );
    }

    #[test]
    fn list_order_multi_release_fixture() {
        let mut v = [
            "META-INF/",
            "META-INF/MANIFEST.MF",
            "META-INF/versions/9/module-info.class",
            "fixture/",
            "fixture/M.class",
            "META-INF/versions/17/",
            "META-INF/versions/17/fixture/",
            "META-INF/versions/17/fixture/M$Named.class",
            "META-INF/versions/17/fixture/M.class",
        ];
        sort_for_list(&mut v);
        assert_eq!(
            v,
            [
                "META-INF/",
                "META-INF/MANIFEST.MF",
                "META-INF/versions/17/",
                "META-INF/versions/17/fixture/",
                "META-INF/versions/17/fixture/M$Named.class",
                "META-INF/versions/17/fixture/M.class",
                "META-INF/versions/9/module-info.class",
                "fixture/",
                "fixture/M.class",
            ]
        );
    }

    #[test]
    fn class_order() {
        let c = |n: &str, r| ClassName {
            name: n.to_string(),
            release: r,
        };
        let mut v = vec![
            c("a.B", Some(17)),
            c("com.foo.OuterB", None),
            c("a.B", Some(9)),
            c("com.foo.Outer$Inner", None),
            c("com.foo.Outer", None),
        ];
        sort_classes(&mut v);
        assert_eq!(
            v,
            vec![
                c("com.foo.Outer", None),
                c("com.foo.Outer$Inner", None),
                c("com.foo.OuterB", None),
                c("a.B", Some(9)),
                c("a.B", Some(17)),
            ]
        );
    }
}
