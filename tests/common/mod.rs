#![allow(dead_code)]

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

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
    let file = File::create(&path).expect("create jar file");
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (entry, data) in entries {
        if entry.ends_with('/') {
            zip.add_directory(*entry, opts).expect("add directory");
        } else {
            zip.start_file(*entry, opts).expect("start file");
            zip.write_all(data).expect("write entry");
        }
    }
    zip.finish().expect("finish jar");
    path
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
