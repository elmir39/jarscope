#![allow(dead_code)]
//! Helpers for building small test JARs on the fly (no JDK needed).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Minimal fake class file: CAFEBABE, minor, major (big-endian), then padding.
pub fn class_bytes(minor: u16, major: u16) -> Vec<u8> {
    let mut v = vec![0xCA, 0xFE, 0xBA, 0xBE];
    v.extend_from_slice(&minor.to_be_bytes());
    v.extend_from_slice(&major.to_be_bytes());
    v.extend_from_slice(&[0u8; 64]);
    v
}

/// Manifest bytes in real JAR format (CRLF line endings, trailing blank line).
pub fn manifest_bytes(attrs: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::from("Manifest-Version: 1.0\r\n");
    for (k, v) in attrs {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.into_bytes()
}

/// Writes a ZIP (all entries Deflated) at `dir/name`. Names ending in `/` become directories.
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

/// Like `make_jar`, with `META-INF/MANIFEST.MF` as the first entry.
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
