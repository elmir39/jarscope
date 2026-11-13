#![allow(dead_code)]

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub fn class_bytes(minor: u16, major: u16) -> Vec<u8> {
    let mut v = vec![0xCA, 0xFE, 0xBA, 0xBE];
    v.extend_from_slice(&minor.to_be_bytes());
    v.extend_from_slice(&major.to_be_bytes());
    v.extend_from_slice(&[0u8; 64]);
    v
}

pub fn manifest_bytes(attrs: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::from("Manifest-Version: 1.0\r\n");
    for (k, v) in attrs {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.into_bytes()
}

pub fn make_jar(dir: &Path, name: &str, entries: &[(&str, Vec<u8>)]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, jar_bytes(entries)).expect("write jar file");
    path
}

pub fn jar_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    jar_bytes_with(entries, CompressionMethod::Deflated)
}

pub fn stored_jar_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    jar_bytes_with(entries, CompressionMethod::Stored)
}

fn jar_bytes_with(entries: &[(&str, Vec<u8>)], method: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(method);
    for (entry, data) in entries {
        if entry.ends_with('/') {
            zip.add_directory(*entry, opts).expect("add directory");
        } else {
            zip.start_file(*entry, opts).expect("start file");
            zip.write_all(data).expect("write entry");
        }
    }
    zip.finish().expect("finish jar").into_inner()
}

// Only the central directory stays readable: `ZipArchive::new` still opens the result, but
// `by_index` fails. Build it from stored entries whose data can't contain `PK\x03\x04`.
pub fn break_local_header_bytes(bytes: &mut [u8]) -> usize {
    let sig = b"PK\x03\x04";
    let mut count = 0;
    let mut i = 0;
    while i + sig.len() <= bytes.len() {
        if &bytes[i..i + sig.len()] == sig {
            bytes[i + 2..i + sig.len()].copy_from_slice(&[0, 0]);
            count += 1;
            i += sig.len();
        } else {
            i += 1;
        }
    }
    count
}

pub fn break_local_headers(path: &Path) {
    let mut bytes = std::fs::read(path).expect("read jar");
    let entries = ZipArchive::new(Cursor::new(&bytes))
        .expect("open jar")
        .len();
    assert_eq!(
        break_local_header_bytes(&mut bytes),
        entries,
        "expected one local header signature per entry"
    );
    std::fs::write(path, bytes).expect("write jar");
}

pub fn make_jar_with_manifest(
    dir: &Path,
    name: &str,
    attrs: &[(&str, &str)],
    entries: &[(&str, Vec<u8>)],
) -> PathBuf {
    let mut all = vec![("META-INF/MANIFEST.MF", manifest_bytes(attrs))];
    all.extend(entries.iter().map(|(n, d)| (*n, d.clone())));
    make_jar(dir, name, &all)
}

// `ZipWriter` refuses duplicate names, so write `a/B.class` and rename it in place to
// `a/A.class` (same length, and the CRC doesn't cover the name).
pub fn make_jar_with_duplicate(dir: &Path, name: &str, first: Vec<u8>, second: Vec<u8>) -> PathBuf {
    let path = make_jar(dir, name, &[("a/A.class", first), ("a/B.class", second)]);
    let mut bytes = std::fs::read(&path).expect("read jar");
    let (from, to) = (b"a/B.class", b"a/A.class");
    let mut replaced = 0;
    let mut i = 0;
    while i + from.len() <= bytes.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + from.len()].copy_from_slice(to);
            replaced += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    assert_eq!(
        replaced, 2,
        "expected the name in one local and one central header"
    );
    std::fs::write(&path, bytes).expect("write jar");
    path
}

pub const ZIP64_MARK: u32 = 0xFFFF_FFFF;

// One stored entry for `raw_zip`. `zip64` writes 0xFFFFFFFF sizes and offset in the central
// header plus a 0x0001 extra field holding `zip64_values` (default: size, size, offset).
// `flags` and `method` go into both headers as given; the data is written stored anyway.
#[derive(Clone, Debug, Default)]
pub struct RawEntry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub utf8: bool,
    pub extra: Vec<u8>,
    pub local_name: Option<Vec<u8>>,
    pub local_extra: Vec<u8>,
    pub zip64: bool,
    pub zip64_values: Option<Vec<u64>>,
    pub flags: u16,
    pub method: u16,
}

impl RawEntry {
    pub fn new(name: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Self {
        RawEntry {
            name: name.as_ref().to_vec(),
            data: data.as_ref().to_vec(),
            ..RawEntry::default()
        }
    }

    fn flag_bits(&self) -> u16 {
        self.flags | if self.utf8 { 0x0800 } else { 0 }
    }
}

// Archive-level knobs for `raw_zip`. Offsets inside the archive don't count `prefix`, as with
// a launch script prepended to a finished JAR. `count`, `cd_size` and `cd_offset` replace the
// values written in the end record (the Zip64 one when `zip64` is set).
#[derive(Clone, Debug, Default)]
pub struct RawZip {
    pub prefix: Vec<u8>,
    pub comment: Vec<u8>,
    pub zip64: bool,
    pub count: Option<u64>,
    pub cd_size: Option<u64>,
    pub cd_offset: Option<u64>,
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

pub fn extra_field(id: u16, data: &[u8]) -> Vec<u8> {
    let mut v = id.to_le_bytes().to_vec();
    v.extend_from_slice(
        &u16::try_from(data.len())
            .expect("extra field length")
            .to_le_bytes(),
    );
    v.extend_from_slice(data);
    v
}

pub fn zip64_extra(values: &[u64]) -> Vec<u8> {
    let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    extra_field(0x0001, &data)
}

pub fn unicode_path_extra(raw_name: &[u8], name: &str) -> Vec<u8> {
    let mut data = vec![1];
    data.extend_from_slice(&crc32(raw_name).to_le_bytes());
    data.extend_from_slice(name.as_bytes());
    extra_field(0x7075, &data)
}

fn u16_len(len: usize) -> [u8; 2] {
    u16::try_from(len)
        .expect("length fits in u16")
        .to_le_bytes()
}

fn u32_of(value: u64) -> [u8; 4] {
    u32::try_from(value)
        .expect("value fits in u32")
        .to_le_bytes()
}

pub fn raw_zip(entries: &[RawEntry], opts: &RawZip) -> Vec<u8> {
    let mut zip = Vec::new();
    let mut offsets = Vec::new();
    for e in entries {
        offsets.push(zip.len() as u64);
        let size = e.data.len() as u64;
        let local_name = e.local_name.as_deref().unwrap_or(&e.name);
        let mut local_extra = Vec::new();
        let local_size = if e.zip64 {
            local_extra.extend(zip64_extra(&[size, size]));
            ZIP64_MARK.to_le_bytes()
        } else {
            u32_of(size)
        };
        local_extra.extend_from_slice(&e.local_extra);

        zip.extend_from_slice(b"PK\x03\x04");
        zip.extend_from_slice(&(if e.zip64 { 45u16 } else { 20 }).to_le_bytes());
        zip.extend_from_slice(&e.flag_bits().to_le_bytes());
        zip.extend_from_slice(&e.method.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0x21, 0]);
        zip.extend_from_slice(&crc32(&e.data).to_le_bytes());
        zip.extend_from_slice(&local_size);
        zip.extend_from_slice(&local_size);
        zip.extend_from_slice(&u16_len(local_name.len()));
        zip.extend_from_slice(&u16_len(local_extra.len()));
        zip.extend_from_slice(local_name);
        zip.extend_from_slice(&local_extra);
        zip.extend_from_slice(&e.data);
    }

    let cd_start = zip.len() as u64;
    for (e, &offset) in entries.iter().zip(&offsets) {
        let size = e.data.len() as u64;
        let mut extra = Vec::new();
        let (size_field, offset_field) = if e.zip64 {
            let values = e.zip64_values.clone().unwrap_or(vec![size, size, offset]);
            extra.extend(zip64_extra(&values));
            (ZIP64_MARK.to_le_bytes(), ZIP64_MARK.to_le_bytes())
        } else {
            (u32_of(size), u32_of(offset))
        };
        extra.extend_from_slice(&e.extra);

        zip.extend_from_slice(b"PK\x01\x02");
        zip.extend_from_slice(&(if e.zip64 { 45u16 } else { 20 }).to_le_bytes());
        zip.extend_from_slice(&(if e.zip64 { 45u16 } else { 20 }).to_le_bytes());
        zip.extend_from_slice(&e.flag_bits().to_le_bytes());
        zip.extend_from_slice(&e.method.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0x21, 0]);
        zip.extend_from_slice(&crc32(&e.data).to_le_bytes());
        zip.extend_from_slice(&size_field);
        zip.extend_from_slice(&size_field);
        zip.extend_from_slice(&u16_len(e.name.len()));
        zip.extend_from_slice(&u16_len(extra.len()));
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&offset_field);
        zip.extend_from_slice(&e.name);
        zip.extend_from_slice(&extra);
    }

    let count = opts.count.unwrap_or(entries.len() as u64);
    let cd_size = opts.cd_size.unwrap_or(zip.len() as u64 - cd_start);
    let cd_offset = opts.cd_offset.unwrap_or(cd_start);
    if opts.zip64 {
        let record_pos = zip.len() as u64;
        zip.extend_from_slice(b"PK\x06\x06");
        zip.extend_from_slice(&44u64.to_le_bytes());
        zip.extend_from_slice(&45u16.to_le_bytes());
        zip.extend_from_slice(&45u16.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&count.to_le_bytes());
        zip.extend_from_slice(&count.to_le_bytes());
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());

        zip.extend_from_slice(b"PK\x06\x07");
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&record_pos.to_le_bytes());
        zip.extend_from_slice(&1u32.to_le_bytes());

        zip.extend_from_slice(b"PK\x05\x06");
        zip.extend_from_slice(&[0; 4]);
        zip.extend_from_slice(&[0xFF; 4]);
        zip.extend_from_slice(&ZIP64_MARK.to_le_bytes());
        zip.extend_from_slice(&ZIP64_MARK.to_le_bytes());
    } else {
        let count = u16::try_from(count)
            .expect("count fits in u16")
            .to_le_bytes();
        zip.extend_from_slice(b"PK\x05\x06");
        zip.extend_from_slice(&[0; 4]);
        zip.extend_from_slice(&count);
        zip.extend_from_slice(&count);
        zip.extend_from_slice(&u32_of(cd_size));
        zip.extend_from_slice(&u32_of(cd_offset));
    }
    zip.extend_from_slice(&u16_len(opts.comment.len()));
    zip.extend_from_slice(&opts.comment);

    let mut out = opts.prefix.clone();
    out.extend(zip);
    out
}
