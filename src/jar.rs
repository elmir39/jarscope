use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::Context;
use zip::ZipArchive;

use crate::classfile::{ClassHeader, parse_header};
use crate::manifest::{Manifest, parse_manifest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Class,
    Versioned { release: u32 },
    ModuleInfo { release: Option<u32> },
    Other,
}

const VERSIONS_PREFIX: &str = "META-INF/versions/";
const MODULE_INFO: &str = "module-info.class";

// Digits only: `"+9".parse::<u32>()` succeeds.
fn parse_release(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|&n| n >= 9)
}

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

const MANIFEST_NAME: &str = "META-INF/MANIFEST.MF";

// Caps memory use on hostile or zip-bomb manifests.
const MANIFEST_LIMIT: u64 = 1 << 20;

const HEADER_LEN: u64 = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarReport {
    pub base: BTreeMap<u16, usize>,
    pub preview: Vec<(String, u16)>,
    pub multi_release: bool,
    pub versioned: BTreeMap<u32, BTreeMap<u16, usize>>,
    pub ignored_versioned: usize,
    pub module_info: Vec<(String, u16)>,
    pub invalid: Vec<(String, String)>,
    pub manifest: Manifest,
}

impl JarReport {
    pub fn required_major(&self) -> Option<u16> {
        self.base.keys().next_back().copied()
    }
}

pub fn scan_jar(path: &Path) -> anyhow::Result<JarReport> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid JAR/ZIP file", path.display()))?;

    let manifest = read_manifest(&mut archive);
    let mut report = JarReport {
        multi_release: manifest.is_multi_release(),
        manifest,
        ..JarReport::default()
    };

    let mut buf = Vec::with_capacity(HEADER_LEN as usize);
    for i in 0..archive.len() {
        let kind = match archive.name_for_index(i) {
            Some(Ok(name)) => classify(&name, name.ends_with('/')),
            Some(Err(e)) => {
                report.invalid.push((format!("entry #{i}"), e.to_string()));
                continue;
            }
            None => {
                report
                    .invalid
                    .push((format!("entry #{i}"), "missing entry".to_string()));
                continue;
            }
        };
        if kind == EntryKind::Other {
            continue;
        }
        if matches!(kind, EntryKind::Versioned { .. }) && !report.multi_release {
            report.ignored_versioned += 1;
            continue;
        }

        let header = match read_header(&mut archive, i, &mut buf) {
            Ok(header) => header,
            Err(reason) => {
                report.invalid.push((entry_name(&archive, i), reason));
                continue;
            }
        };
        match kind {
            EntryKind::Class => {
                *report.base.entry(header.major).or_default() += 1;
            }
            EntryKind::Versioned { release } => {
                *report
                    .versioned
                    .entry(release)
                    .or_default()
                    .entry(header.major)
                    .or_default() += 1;
            }
            EntryKind::ModuleInfo { .. } => {
                report
                    .module_info
                    .push((entry_name(&archive, i), header.major));
                continue;
            }
            EntryKind::Other => continue,
        }
        if header.is_preview() {
            report.preview.push((entry_name(&archive, i), header.major));
        }
    }
    Ok(report)
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Manifest {
    let Ok(entry) = archive.by_name(MANIFEST_NAME) else {
        return Manifest::default();
    };
    let mut bytes = Vec::new();
    match entry.take(MANIFEST_LIMIT).read_to_end(&mut bytes) {
        Ok(_) => parse_manifest(&bytes),
        Err(_) => Manifest::default(),
    }
}

fn read_header<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    i: usize,
    buf: &mut Vec<u8>,
) -> Result<ClassHeader, String> {
    let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
    buf.clear();
    entry
        .by_ref()
        .take(HEADER_LEN)
        .read_to_end(buf)
        .map_err(|e| e.to_string())?;
    parse_header(buf).map_err(|e| e.to_string())
}

fn entry_name<R: Read + Seek>(archive: &ZipArchive<R>, i: usize) -> String {
    match archive.name_for_index(i) {
        Some(Ok(name)) => name.into_owned(),
        _ => format!("entry #{i}"),
    }
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
