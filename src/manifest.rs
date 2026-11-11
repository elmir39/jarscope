#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    attributes: Vec<(String, String)>,
}

impl Manifest {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    // Not trimmed on purpose: the JVM treats "true " as false.
    pub fn is_multi_release(&self) -> bool {
        self.get("Multi-Release")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }
}

pub fn parse_manifest(bytes: &[u8]) -> Manifest {
    let text = String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n");

    let mut logical: Vec<String> = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            if let Some(prev) = logical.last_mut() {
                prev.push_str(rest);
            }
        } else {
            logical.push(line.to_string());
        }
    }

    let mut manifest = Manifest::default();
    for line in logical {
        let Some((name, value)) = line.split_once(": ") else {
            continue;
        };
        match manifest
            .attributes
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            Some(entry) => *entry = (name.to_string(), value.to_string()),
            None => manifest
                .attributes
                .push((name.to_string(), value.to_string())),
        }
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(s: &str) -> bool {
        parse_manifest(s.as_bytes()).is_multi_release()
    }

    #[test]
    fn multi_release_true() {
        assert!(mr("Manifest-Version: 1.0\nMulti-Release: true\n"));
    }

    #[test]
    fn multi_release_false_or_missing() {
        assert!(!mr("Multi-Release: false\n"));
        assert!(!mr("Manifest-Version: 1.0\n"));
    }

    #[test]
    fn multi_release_case_insensitive() {
        assert!(mr("multi-release: TRUE\n"));
    }

    #[test]
    fn multi_release_trailing_space_is_false() {
        assert!(!mr("Multi-Release: true \n"));
    }

    #[test]
    fn three_line_wrap_is_joined() {
        let m = parse_manifest(
            b"Bundle-Description: Guava is a suite of core and expanded libraries th\n at include    utility classes, Google's collections, I/O classes, and\n     much more.\nOther: x\n",
        );
        assert_eq!(
            m.get("Bundle-Description"),
            Some(
                "Guava is a suite of core and expanded libraries that include    utility classes, Google's collections, I/O classes, and    much more."
            )
        );
        assert_eq!(m.get("Other"), Some("x"));
    }

    #[test]
    fn crlf_and_lf_agree() {
        let lf = parse_manifest(b"A: 1\nB: long\n er\nMulti-Release: true\n");
        let crlf = parse_manifest(b"A: 1\r\nB: long\r\n er\r\nMulti-Release: true\r\n");
        assert_eq!(lf, crlf);
        assert_eq!(crlf.get("B"), Some("longer"));
    }

    #[test]
    fn lone_cr_line_endings() {
        let m = parse_manifest(b"A: 1\rB: long\r er\rC: 3");
        assert_eq!(m.get("A"), Some("1"));
        assert_eq!(m.get("B"), Some("longer"));
        assert_eq!(m.get("C"), Some("3"));
    }

    #[test]
    fn no_trailing_newline() {
        assert!(mr("Manifest-Version: 1.0\nMulti-Release: true"));
    }

    #[test]
    fn per_entry_sections_ignored() {
        let m = parse_manifest(b"A: 1\n\nName: foo/Bar.class\nMulti-Release: true\n");
        assert_eq!(m.get("A"), Some("1"));
        assert_eq!(m.get("Name"), None);
        assert!(!m.is_multi_release());
    }

    #[test]
    fn empty_and_random_bytes() {
        assert_eq!(parse_manifest(b""), Manifest::default());
        let junk: Vec<u8> = (0..=255u8).cycle().take(2000).collect();
        let _ = parse_manifest(&junk);
        assert_eq!(
            parse_manifest(&[0xFF, 0xFE, 0x00, 0x80]),
            Manifest::default()
        );
    }

    #[test]
    fn leading_continuation_ignored() {
        let m = parse_manifest(b" orphan\nA: 1\n");
        assert_eq!(m.get("A"), Some("1"));
        assert_eq!(m.get("orphan"), None);
    }

    #[test]
    fn value_keeps_later_colon_space() {
        let m = parse_manifest(b"Created-By: Maven: 3.9\n");
        assert_eq!(m.get("created-by"), Some("Maven: 3.9"));
    }

    #[test]
    fn lines_without_separator_skipped_and_last_duplicate_wins() {
        let m = parse_manifest(b"garbage\nA:nospace\nA: 1\na: 2\n");
        assert_eq!(m.get("A"), Some("2"));
    }
}
