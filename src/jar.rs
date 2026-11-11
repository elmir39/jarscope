//! JAR entry classification by name.

/// What a ZIP entry is, decided from its name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A normal class file; counts toward the base requirement.
    Class,
    /// A class under `META-INF/versions/N/` with a valid release `N`.
    Versioned {
        /// The release number `N` (at least 9).
        release: u32,
    },
    /// A `module-info.class` at the root (`None`) or under `META-INF/versions/N/`.
    ModuleInfo {
        /// The release number, or `None` for the root descriptor.
        release: Option<u32>,
    },
    /// Not a loadable class: resources, directories, manifest, invalid versioned paths.
    Other,
}

const VERSIONS_PREFIX: &str = "META-INF/versions/";
const MODULE_INFO: &str = "module-info.class";

/// Parses a versioned-directory number: ASCII digits only, fitting `u32`, at least 9.
fn parse_release(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|&n| n >= 9)
}

/// Classifies a ZIP entry from its name; case-sensitive, like the JVM.
pub fn classify(name: &str, is_dir: bool) -> EntryKind {
    if is_dir || name.ends_with('/') || !name.ends_with(".class") {
        return EntryKind::Other;
    }
    if name == MODULE_INFO {
        return EntryKind::ModuleInfo { release: None };
    }
    if let Some(rest) = name.strip_prefix(VERSIONS_PREFIX) {
        let Some((n, rest)) = rest.split_once('/') else {
            return EntryKind::Other;
        };
        let Some(release) = parse_release(n) else {
            return EntryKind::Other;
        };
        return if rest == MODULE_INFO {
            EntryKind::ModuleInfo {
                release: Some(release),
            }
        } else {
            EntryKind::Versioned { release }
        };
    }
    EntryKind::Class
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> EntryKind {
        classify(name, false)
    }

    #[test]
    fn classify_plain_classes() {
        assert_eq!(c("com/foo/Bar.class"), EntryKind::Class);
        assert_eq!(c("com/foo/Outer$Inner.class"), EntryKind::Class);
        assert_eq!(c("BOOT-INF/classes/A.class"), EntryKind::Class);
        assert_eq!(c("META-INF/foo.class"), EntryKind::Class);
    }

    #[test]
    fn classify_non_class_and_dirs() {
        assert_eq!(c("META-INF/MANIFEST.MF"), EntryKind::Other);
        assert_eq!(c("a/B.txt"), EntryKind::Other);
        assert_eq!(classify("com/foo/Bar.class", true), EntryKind::Other);
        assert_eq!(c("com/foo/"), EntryKind::Other);
        assert_eq!(c("META-INF/versions/9/"), EntryKind::Other);
        assert_eq!(c("META-INF/versions/9"), EntryKind::Other);
    }

    #[test]
    fn classify_module_info() {
        assert_eq!(
            c("module-info.class"),
            EntryKind::ModuleInfo { release: None }
        );
        assert_eq!(
            c("META-INF/versions/9/module-info.class"),
            EntryKind::ModuleInfo { release: Some(9) }
        );
        assert_eq!(c("META-INF/versions/8/module-info.class"), EntryKind::Other);
        assert_eq!(c("com/foo/module-info.class"), EntryKind::Class);
    }

    #[test]
    fn classify_versioned() {
        assert_eq!(
            c("META-INF/versions/11/com/foo/Bar.class"),
            EntryKind::Versioned { release: 11 }
        );
        assert_eq!(
            c("META-INF/versions/11/com/foo/Outer$Inner.class"),
            EntryKind::Versioned { release: 11 }
        );
        assert_eq!(
            c("META-INF/versions/9/X.class"),
            EntryKind::Versioned { release: 9 }
        );
    }

    #[test]
    fn classify_invalid_release() {
        for n in ["abc", "8", "+9", "", " 9", "9a", "99999999999", "-9"] {
            assert_eq!(
                c(&format!("META-INF/versions/{n}/X.class")),
                EntryKind::Other,
                "{n:?}"
            );
        }
        assert_eq!(
            c("META-INF/versions/abc/module-info.class"),
            EntryKind::Other
        );
    }

    #[test]
    fn classify_case_sensitive_prefix() {
        assert_eq!(c("meta-inf/versions/11/X.class"), EntryKind::Class);
    }
}
