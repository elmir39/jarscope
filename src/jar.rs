use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use anyhow::Context;
use zip::ZipArchive;

use crate::cdir::{CentralDirectory, EntryInfo, read_entry};
use crate::classfile::{ClassHeader, parse_header};
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

const HEADER_LEN: u64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

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
    pub entries: Vec<Entry>,
    pub duplicate_names: bool,
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

pub(crate) fn open(path: &Path) -> anyhow::Result<ZipArchive<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    // Unbuffered File means one syscall per small central-directory read; threads then contend in the kernel.
    ZipArchive::new(BufReader::new(file)).with_context(|| invalid_jar(path))
}

pub(crate) fn invalid_jar(path: &Path) -> String {
    format!("{} is not a valid JAR/ZIP file", path.display())
}

pub(crate) fn open_cdir(path: &Path) -> anyhow::Result<CentralDirectory<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    CentralDirectory::new(BufReader::new(file)).with_context(|| invalid_jar(path))
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
    let mut archive = open(path)?;
    let mut report = index(&mut archive);

    let mut buf = Vec::with_capacity(HEADER_LEN as usize);
    for entry in &report.entries {
        let kind = classify(&entry.name, entry.is_dir());
        if kind == EntryKind::Other {
            continue;
        }
        if matches!(kind, EntryKind::Versioned { .. }) && !report.multi_release {
            report.ignored_versioned += 1;
            continue;
        }

        let header = match read_header(&mut archive, entry.index, &mut buf) {
            Ok(header) => header,
            Err(reason) => {
                report.invalid.push((entry.name.clone(), reason));
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
                report.module_info.push((entry.name.clone(), header.major));
                continue;
            }
            EntryKind::Other => continue,
        }
        if header.is_preview() {
            report.preview.push((entry.name.clone(), header.major));
        }
    }
    Ok(report)
}

fn index<R: Read + Seek>(archive: &mut ZipArchive<R>) -> JarReport {
    let manifest = read_manifest(archive);
    let mut report = JarReport {
        multi_release: manifest.as_ref().is_some_and(Manifest::is_multi_release),
        manifest,
        duplicate_names: archive.has_duplicate_names(),
        entries: Vec::with_capacity(archive.len()),
        ..JarReport::default()
    };
    for i in 0..archive.len() {
        let entry = archive.by_index_data(i).and_then(|data| {
            Ok(Entry {
                index: i,
                name: data.name()?.into_owned(),
                size: data.size(),
                compressed_size: data.compressed_size(),
            })
        });
        match entry {
            Ok(entry) => report.entries.push(entry),
            Err(e) => report.invalid.push((format!("entry #{i}"), e.to_string())),
        }
    }
    report
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<Manifest> {
    archive.index_for_name(MANIFEST_NAME)?;
    let Ok(entry) = archive.by_name(MANIFEST_NAME) else {
        return Some(Manifest::default());
    };
    let mut bytes = Vec::new();
    match entry.take(MANIFEST_LIMIT).read_to_end(&mut bytes) {
        Ok(_) => Some(parse_manifest(&bytes)),
        Err(_) => Some(Manifest::default()),
    }
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
