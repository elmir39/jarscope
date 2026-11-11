use crate::classfile::java_version;
use crate::jar::JarReport;
use crate::names::{class_name, sort_classes, sort_for_list};

const MAX_LISTED: usize = 10;

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

pub fn render_list(report: &JarReport, classes_only: bool) -> String {
    let mut out = String::new();
    if classes_only {
        let mut classes: Vec<_> = report
            .entries
            .iter()
            .filter_map(|e| class_name(&e.name))
            .collect();
        sort_classes(&mut classes);
        for c in &classes {
            out.push_str(&c.name);
            if let Some(r) = c.release {
                out.push_str(&format!(" (META-INF/versions/{r})"));
            }
            out.push('\n');
        }
    } else {
        let mut names: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
        sort_for_list(&mut names);
        for n in names {
            out.push_str(n);
            out.push('\n');
        }
    }
    out
}

pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn human_size(n: u64) -> String {
    if n < 1000 {
        return if n == 1 {
            "1 byte".to_string()
        } else {
            format!("{n} bytes")
        };
    }
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    let mut value = n as f64 / 1000.0;
    let mut unit = 0;
    // Rounding first keeps 999,999 at "1.0 MB" instead of "1000.0 KB".
    while (value * 10.0).round() / 10.0 >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn size_line(n: u64) -> String {
    if n < 1000 {
        human_size(n)
    } else {
        format!("{} ({} bytes)", human_size(n), thousands(n))
    }
}

pub fn render_info(report: &JarReport) -> String {
    let mut rows: Vec<(&str, String)> = Vec::new();
    match &report.manifest {
        None => rows.push(("Manifest", "none".to_string())),
        Some(m) => {
            for name in ["Main-Class", "Created-By", "Build-Jdk", "Build-Jdk-Spec"] {
                if let Some(v) = m.get(name) {
                    rows.push((name, v.to_string()));
                }
            }
            rows.push(("Multi-Release", report.multi_release.to_string()));
            if let Some(v) = m.get("Automatic-Module-Name") {
                rows.push(("Automatic-Module-Name", v.to_string()));
            }
        }
    }
    let classes = report.class_count();
    let noun = if classes == 1 { "class" } else { "classes" };
    rows.push((
        "Entries",
        format!(
            "{} ({} {noun})",
            thousands(report.entries.len() as u64),
            thousands(classes as u64)
        ),
    ));
    rows.push(("Compressed", size_line(report.total_compressed_size())));
    rows.push(("Uncompressed", size_line(report.total_size())));

    let width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0) + 3;
    let mut out = String::new();
    for (l, v) in &rows {
        out.push_str(&format!("{:<width$}{v}\n", format!("{l}:")));
    }
    out
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

    fn entries(names: &[&str]) -> JarReport {
        JarReport {
            entries: names
                .iter()
                .enumerate()
                .map(|(index, n)| crate::jar::Entry {
                    index,
                    name: n.to_string(),
                    size: 0,
                    compressed_size: 0,
                })
                .collect(),
            ..JarReport::default()
        }
    }

    #[test]
    fn list_all_in_list_order() {
        let r = entries(&[
            "com/foo/bar/X.class",
            "com/foo/baz.txt",
            "com/foo/",
            "A.txt",
        ]);
        assert_eq!(
            render_list(&r, false),
            "A.txt\ncom/foo/\ncom/foo/baz.txt\ncom/foo/bar/X.class\n"
        );
    }

    #[test]
    fn list_classes_only_with_release_suffix() {
        let r = entries(&[
            "META-INF/versions/17/com/foo/Bar.class",
            "README.md",
            "com/foo/",
            "com/foo/Bar.class",
            "META-INF/versions/9/module-info.class",
        ]);
        assert_eq!(
            render_list(&r, true),
            "com.foo.Bar\nmodule-info (META-INF/versions/9)\ncom.foo.Bar (META-INF/versions/17)\n"
        );
    }

    #[test]
    fn list_empty() {
        let r = JarReport::default();
        assert_eq!(render_list(&r, false), "");
        assert_eq!(render_list(&r, true), "");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(2_610_717), "2,610,717");
        assert_eq!(thousands(u64::MAX), "18,446,744,073,709,551,615");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(999), "999 bytes");
        assert_eq!(human_size(1000), "1.0 KB");
        assert_eq!(human_size(1293), "1.3 KB");
        assert_eq!(human_size(999_999), "1.0 MB");
        assert_eq!(human_size(2_610_717), "2.6 MB");
        assert_eq!(human_size(u64::MAX), "18.4 EB");
    }

    fn with_manifest(attrs: &[(&str, &str)]) -> JarReport {
        let mut text = String::from("Manifest-Version: 1.0\r\n");
        for (k, v) in attrs {
            text.push_str(&format!("{k}: {v}\r\n"));
        }
        let m = crate::manifest::parse_manifest(text.as_bytes());
        JarReport {
            multi_release: m.is_multi_release(),
            manifest: Some(m),
            ..JarReport::default()
        }
    }

    #[test]
    fn info_full_manifest() {
        let mut r = with_manifest(&[
            ("Automatic-Module-Name", "com.example"),
            ("Build-Jdk-Spec", "21"),
            ("Created-By", "Maven"),
            ("Main-Class", "com.example.Main"),
            ("Build-Jdk", "21.0.5"),
        ]);
        r.entries = vec![
            crate::jar::Entry {
                index: 0,
                name: "A.class".into(),
                size: 2_000,
                compressed_size: 1_000,
            },
            crate::jar::Entry {
                index: 1,
                name: "B.txt".into(),
                size: 5,
                compressed_size: 1,
            },
        ];
        assert_eq!(
            render_info(&r),
            "\
Main-Class:             com.example.Main
Created-By:             Maven
Build-Jdk:              21.0.5
Build-Jdk-Spec:         21
Multi-Release:          false
Automatic-Module-Name:  com.example
Entries:                2 (1 class)
Compressed:             1.0 KB (1,001 bytes)
Uncompressed:           2.0 KB (2,005 bytes)
"
        );
    }

    #[test]
    fn info_no_manifest() {
        assert_eq!(
            render_info(&JarReport::default()),
            "Manifest:      none\nEntries:       0 (0 classes)\nCompressed:    0 bytes\nUncompressed:  0 bytes\n"
        );
    }

    #[test]
    fn info_only_manifest_version() {
        let r = with_manifest(&[]);
        assert_eq!(
            render_info(&r),
            "Multi-Release:  false\nEntries:        0 (0 classes)\nCompressed:     0 bytes\nUncompressed:   0 bytes\n"
        );
    }

    #[test]
    fn info_trailing_space_multi_release_is_false() {
        let r = with_manifest(&[("Multi-Release", "true ")]);
        assert!(render_info(&r).starts_with("Multi-Release:  false\n"));
    }
}
