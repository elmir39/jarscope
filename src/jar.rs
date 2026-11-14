use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;

use anyhow::Context;

use crate::cdir::{CentralDirectory, ENTRY_READER_CAPACITY, EntryInfo, EntryReader, read_entry};
use crate::classfile::parse_header;
use crate::manifest::{Manifest, parse_manifest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Class,
    Versioned { release: u32 },
    ModuleInfo { release: Option<u32> },
    Other,
}

pub(crate) const VERSIONS_PREFIX: &str = "META-INF/versions/";
const MODULE_INFO: &str = "module-info.class";

// Digits only: `"+9".parse::<u32>()` succeeds.
pub(crate) fn parse_release(s: &str) -> Option<u32> {
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

const HEADER_LEN: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarReport {
    pub base: BTreeMap<u16, usize>,
    pub preview: Vec<(String, u16)>,
    pub multi_release: bool,
    pub versioned: BTreeMap<u32, BTreeMap<u16, usize>>,
    pub ignored_versioned: usize,
    pub module_info: Vec<(String, u16)>,
    pub invalid: Vec<(String, String)>,
    pub manifest: Option<Manifest>,
}

impl JarReport {
    pub fn required_major(&self) -> Option<u16> {
        self.base.keys().next_back().copied()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarInfo {
    pub manifest: Option<Manifest>,
    pub multi_release: bool,
    pub entries: u64,
    pub classes: u64,
    pub size: u64,
    pub compressed_size: u64,
}

// One arena instead of a `String` per name: ~70 bytes per entry on huge JARs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryNames {
    text: String,
    ends: Vec<usize>,
}

impl EntryNames {
    // A hint only: failing to reserve is not an error, and both bounds come from a
    // central directory already checked against the file size.
    fn reserve(&mut self, text: u64, count: u64) {
        let _ = self.text.try_reserve(usize::try_from(text).unwrap_or(0));
        let _ = self.ends.try_reserve(usize::try_from(count).unwrap_or(0));
    }

    pub fn push(&mut self, name: &str) {
        self.text.push_str(name);
        self.ends.push(self.text.len());
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let mut start = 0;
        self.ends.iter().map(move |&end| {
            let name = &self.text[start..end];
            start = end;
            name
        })
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
}

pub(crate) fn invalid_jar(path: &Path) -> String {
    format!("{} is not a valid JAR/ZIP file", path.display())
}

fn open_file(path: &Path) -> anyhow::Result<File> {
    File::open(path).with_context(|| format!("cannot open {}", path.display()))
}

pub(crate) fn open_cdir(path: &Path) -> anyhow::Result<CentralDirectory<BufReader<File>>> {
    CentralDirectory::new(BufReader::new(open_file(path)?)).with_context(|| invalid_jar(path))
}

pub fn read_info(path: &Path) -> anyhow::Result<JarInfo> {
    let mut cd = open_cdir(path)?;
    let mut info = JarInfo::default();
    let mut manifest = None;
    while let Some(entry) = cd.next_entry().with_context(|| invalid_jar(path))? {
        info.entries += 1;
        if !entry.is_dir() && entry.name.ends_with(".class") {
            info.classes += 1;
        }
        // Sizes come from untrusted headers; a plain `+` panics on overflow in debug builds.
        info.size = info.size.saturating_add(entry.size);
        info.compressed_size = info.compressed_size.saturating_add(entry.compressed_size);
        // The last record wins for a duplicated name, as in `zip` and the JVM's `ZipFile`.
        if entry.name == MANIFEST_NAME {
            manifest = Some(entry.to_info());
        }
    }
    if let Some(entry) = manifest {
        let m = read_manifest_entry(&mut cd.into_inner(), &entry);
        info.multi_release = m.is_multi_release();
        info.manifest = Some(m);
    }
    Ok(info)
}

pub fn read_names(path: &Path) -> anyhow::Result<EntryNames> {
    let mut cd = open_cdir(path)?;
    let mut names = EntryNames::default();
    names.reserve(cd.variable_bytes(), cd.len());
    while let Some(entry) = cd.next_entry().with_context(|| invalid_jar(path))? {
        names.push(entry.name);
    }
    Ok(names)
}

pub fn scan_jar(path: &Path) -> anyhow::Result<JarReport> {
    let mut cd = open_cdir(path)?;
    // A second `File::open`, not `try_clone`: a cloned descriptor shares the file offset on
    // Unix, so the two buffered readers would move each other's position.
    let data = BufReader::with_capacity(ENTRY_READER_CAPACITY, open_file(path)?);
    let mut reader = EntryReader::new(data);

    let mut report = JarReport::default();
    let mut multi_release = None;
    let mut manifest_entry = None;
    let mut prefix = [0u8; HEADER_LEN];
    while let Some(entry) = cd.next_entry().with_context(|| invalid_jar(path))? {
        if entry.name == MANIFEST_NAME && multi_release.is_none() {
            manifest_entry = Some(entry.to_info());
            continue;
        }
        let kind = classify(entry.name, entry.is_dir());
        match kind {
            EntryKind::Other => continue,
            EntryKind::Versioned { .. } => {
                let counted = match multi_release {
                    Some(mr) => mr,
                    None => {
                        report.manifest = look_ahead_manifest(path, &mut reader)?;
                        let mr = report
                            .manifest
                            .as_ref()
                            .is_some_and(Manifest::is_multi_release);
                        multi_release = Some(mr);
                        mr
                    }
                };
                if !counted {
                    report.ignored_versioned += 1;
                    continue;
                }
            }
            EntryKind::Class | EntryKind::ModuleInfo { .. } => {}
        }

        let parsed = reader
            .read_prefix(&entry, &mut prefix)
            .and_then(|n| Ok(parse_header(&prefix[..n])?));
        let header = match parsed {
            Ok(header) => header,
            Err(e) => {
                report
                    .invalid
                    .push((entry.name.to_string(), format!("{e:#}")));
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
                    .push((entry.name.to_string(), header.major));
                continue;
            }
            EntryKind::Other => continue,
        }
        if header.is_preview() {
            report.preview.push((entry.name.to_string(), header.major));
        }
    }

    match multi_release {
        Some(mr) => report.multi_release = mr,
        None => {
            if let Some(entry) = manifest_entry {
                let m = read_manifest_entry(reader.get_mut(), &entry);
                report.multi_release = m.is_multi_release();
                report.manifest = Some(m);
            }
        }
    }
    Ok(report)
}

// A versioned class can come before the manifest, and with a duplicated manifest the last
// record wins, so only a pass over the whole directory decides `Multi-Release`.
fn look_ahead_manifest<R: BufRead + Seek>(
    path: &Path,
    reader: &mut EntryReader<R>,
) -> anyhow::Result<Option<Manifest>> {
    let mut last = None;
    {
        let mut cd = CentralDirectory::new(reader.get_mut()).with_context(|| invalid_jar(path))?;
        while let Some(entry) = cd.next_entry().with_context(|| invalid_jar(path))? {
            if entry.name == MANIFEST_NAME {
                last = Some(entry.to_info());
            }
        }
    }
    Ok(last.map(|entry| read_manifest_entry(reader.get_mut(), &entry)))
}

fn read_manifest_entry<R: Read + Seek>(reader: &mut R, entry: &EntryInfo) -> Manifest {
    let mut bytes = Vec::new();
    match read_entry(reader, entry, MANIFEST_LIMIT, &mut bytes) {
        Ok(()) => {
            bytes.truncate(MANIFEST_LIMIT as usize);
            parse_manifest(&bytes)
        }
        Err(_) => Manifest::default(),
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
