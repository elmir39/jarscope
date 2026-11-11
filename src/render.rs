//! Text rendering of a [`JarReport`] for the `version` command.

use crate::classfile::java_version;
use crate::jar::JarReport;

/// Most names listed in the preview and invalid sections.
const MAX_LISTED: usize = 10;

/// `Java 8`, or `unknown (major N)` for majors below 45.
fn label(major: u16) -> String {
    match java_version(major) {
        Some(v) => format!("Java {v}"),
        None => format!("unknown (major {major})"),
    }
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}es")
    }
}

/// Appends at most [`MAX_LISTED`] indented lines, then `… and N more`.
fn push_list(out: &mut String, lines: &[String]) {
    for line in lines.iter().take(MAX_LISTED) {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if lines.len() > MAX_LISTED {
        out.push_str(&format!("  … and {} more\n", lines.len() - MAX_LISTED));
    }
}

/// Renders the `version` command output; ends with a single newline.
pub fn render_version(report: &JarReport) -> String {
    let mut sections: Vec<String> = Vec::new();

    sections.push(match report.required_major() {
        Some(m) => match java_version(m) {
            Some(v) => format!("Requires: Java {v} (major {m})\n"),
            None => format!("Requires: unknown (major {m})\n"),
        },
        None => "Requires: unknown (no class files found)\n".to_string(),
    });

    if !report.base.is_empty() {
        let rows: Vec<(String, String)> = report
            .base
            .iter()
            .map(|(&m, &n)| (label(m), n.to_string()))
            .collect();
        let lw = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
        let cw = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        let mut s = String::from("Classes by Java version:\n");
        for (l, c) in &rows {
            let pad = lw - l.chars().count();
            s.push_str(&format!("  {l}{} {c:>cw$}\n", " ".repeat(pad + 1)));
        }
        sections.push(s);
    }

    if !report.versioned.is_empty() {
        let mut s = String::from("Multi-release (Multi-Release: true):\n");
        for (release, majors) in &report.versioned {
            let total: usize = majors.values().sum();
            let top = majors.keys().next_back().copied().unwrap_or(0);
            s.push_str(&format!(
                "  META-INF/versions/{release}/   {} ({})\n",
                plural(total, "class"),
                label(top)
            ));
        }
        sections.push(s);
    }

    if !report.module_info.is_empty() {
        let mut s = String::new();
        for (name, major) in &report.module_info {
            if name == "module-info.class" {
                s.push_str(&format!("module-info.class: {}\n", label(*major)));
            } else {
                s.push_str(&format!("module-info.class: {} ({name})\n", label(*major)));
            }
        }
        sections.push(s);
    }

    if let Some(&(_, first)) = report.preview.first() {
        let n = report.preview.len();
        let uniform = report.preview.iter().all(|p| p.1 == first);
        let mut s = if uniform {
            format!(
                "Preview classes: {n} — need exactly {} with --enable-preview\n",
                label(first)
            )
        } else {
            format!(
                "Preview classes: {n} — need exactly the Java version shown, with --enable-preview\n"
            )
        };
        let lines: Vec<String> = report
            .preview
            .iter()
            .map(|(name, m)| {
                if uniform {
                    name.clone()
                } else {
                    format!("{name} ({})", label(*m))
                }
            })
            .collect();
        push_list(&mut s, &lines);
        sections.push(s);
    }

    if report.ignored_versioned > 0 {
        sections.push(format!(
            "Ignored: {} under META-INF/versions/ (Multi-Release not set in manifest)\n",
            plural(report.ignored_versioned, "class")
        ));
    }

    if !report.invalid.is_empty() {
        let mut s = format!("Invalid class files: {}\n", report.invalid.len());
        let lines: Vec<String> = report
            .invalid
            .iter()
            .map(|(n, r)| format!("{n}: {r}"))
            .collect();
        push_list(&mut s, &lines);
        sections.push(s);
    }

    sections.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn base(pairs: &[(u16, usize)]) -> BTreeMap<u16, usize> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn simple_report() {
        let r = JarReport {
            base: base(&[(52, 1961)]),
            module_info: vec![("META-INF/versions/9/module-info.class".into(), 53)],
            ..JarReport::default()
        };
        assert_eq!(
            render_version(&r),
            "Requires: Java 8 (major 52)\n\nClasses by Java version:\n  Java 8  1961\n\nmodule-info.class: Java 9 (META-INF/versions/9/module-info.class)\n"
        );
    }

    #[test]
    fn full_report() {
        let r = JarReport {
            base: base(&[(52, 120), (61, 3)]),
            multi_release: true,
            versioned: BTreeMap::from([(21, base(&[(65, 4)]))]),
            ignored_versioned: 2,
            preview: vec![("com/example/P.class".into(), 61)],
            module_info: vec![
                ("module-info.class".into(), 53),
                ("META-INF/versions/9/module-info.class".into(), 53),
            ],
            invalid: vec![("com/example/Broken.class".into(), "bad magic".into())],
            ..JarReport::default()
        };
        let expected = "\
Requires: Java 17 (major 61)

Classes by Java version:
  Java 8   120
  Java 17    3

Multi-release (Multi-Release: true):
  META-INF/versions/21/   4 classes (Java 21)

module-info.class: Java 9
module-info.class: Java 9 (META-INF/versions/9/module-info.class)

Preview classes: 1 — need exactly Java 17 with --enable-preview
  com/example/P.class

Ignored: 2 classes under META-INF/versions/ (Multi-Release not set in manifest)

Invalid class files: 1
  com/example/Broken.class: bad magic
";
        assert_eq!(render_version(&r), expected);
    }

    #[test]
    fn no_classes() {
        assert_eq!(
            render_version(&JarReport::default()),
            "Requires: unknown (no class files found)\n"
        );
    }

    #[test]
    fn unknown_major() {
        let r = JarReport {
            base: base(&[(3, 2)]),
            ..JarReport::default()
        };
        assert_eq!(
            render_version(&r),
            "Requires: unknown (major 3)\n\nClasses by Java version:\n  unknown (major 3)  2\n"
        );
    }

    #[test]
    fn singular_and_plural() {
        let mut r = JarReport {
            base: base(&[(52, 1)]),
            versioned: BTreeMap::from([(11, base(&[(55, 1)]))]),
            ignored_versioned: 1,
            ..JarReport::default()
        };
        let out = render_version(&r);
        assert!(out.contains("META-INF/versions/11/   1 class (Java 11)\n"));
        assert!(out.contains("Ignored: 1 class under"));
        r.ignored_versioned = 3;
        assert!(render_version(&r).contains("Ignored: 3 classes under"));
    }

    #[test]
    fn invalid_truncated_after_ten() {
        let r = JarReport {
            invalid: (0..13)
                .map(|i| (format!("C{i}.class"), "bad".into()))
                .collect(),
            ..JarReport::default()
        };
        let out = render_version(&r);
        assert!(out.contains("Invalid class files: 13\n"));
        assert!(out.contains("  C9.class: bad\n"));
        assert!(!out.contains("C10.class"));
        assert!(out.ends_with("  … and 3 more\n"));
    }

    #[test]
    fn mixed_preview_majors() {
        let r = JarReport {
            preview: vec![("A.class".into(), 61), ("B.class".into(), 65)],
            ..JarReport::default()
        };
        let out = render_version(&r);
        assert!(out.contains(
            "Preview classes: 2 — need exactly the Java version shown, with --enable-preview\n  A.class (Java 17)\n  B.class (Java 21)\n"
        ));
    }
}
