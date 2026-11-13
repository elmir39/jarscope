mod common;

use std::io::{BufReader, Cursor, Write};

use common::{RawEntry, RawZip, ZIP64_MARK, extra_field, raw_zip, unicode_path_extra};
use jarscope::cdir::{CentralDirectory, Entry, EntryInfo, read_entry};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rec {
    name: String,
    size: u64,
    compressed_size: u64,
    method: u16,
    descriptor: bool,
    crc32: u32,
    header_offset: u64,
}

impl From<Entry<'_>> for Rec {
    fn from(e: Entry<'_>) -> Self {
        Rec {
            name: e.name.to_string(),
            size: e.size,
            compressed_size: e.compressed_size,
            method: e.method,
            descriptor: e.flags & 0x8 != 0,
            crc32: e.crc32,
            header_offset: e.header_offset,
        }
    }
}

fn read_all(bytes: &[u8]) -> anyhow::Result<Vec<Rec>> {
    let mut cd = CentralDirectory::new(Cursor::new(bytes))?;
    let mut out = Vec::new();
    while let Some(e) = cd.next_entry()? {
        out.push(Rec::from(e));
    }
    assert_eq!(out.len() as u64, cd.len());
    assert!(cd.next_entry()?.is_none(), "stays at the end");
    Ok(out)
}

fn read_ok(bytes: &[u8]) -> Vec<Rec> {
    read_all(bytes).expect("cdir reads the archive")
}

fn read_err(bytes: &[u8]) -> String {
    format!("{:#}", read_all(bytes).expect_err("cdir should fail"))
}

fn names(recs: &[Rec]) -> Vec<&str> {
    recs.iter().map(|r| r.name.as_str()).collect()
}

fn zip_rows(bytes: &[u8]) -> Vec<Rec> {
    let archive = ZipArchive::new(Cursor::new(bytes)).expect("zip opens the archive");
    (0..archive.len())
        .map(|i| {
            let d = archive.by_index_data(i).expect("entry data");
            Rec {
                name: d.name().expect("name").into_owned(),
                size: d.size(),
                compressed_size: d.compressed_size(),
                method: match d.compression() {
                    CompressionMethod::Stored => 0,
                    CompressionMethod::Deflated => 8,
                    other => panic!("unexpected method {other:?}"),
                },
                descriptor: d.flags().is_using_data_descriptor(),
                crc32: d.crc32(),
                header_offset: d.header_start(),
            }
        })
        .collect()
}

fn assert_matches_zip(bytes: &[u8]) -> Vec<Rec> {
    let recs = read_ok(bytes);
    assert_eq!(recs, zip_rows(bytes));
    recs
}

fn assert_local_headers(bytes: &[u8], recs: &[Rec]) {
    for r in recs {
        let at = usize::try_from(r.header_offset).expect("offset fits");
        assert_eq!(&bytes[at..at + 4], b"PK\x03\x04", "{}", r.name);
    }
}

fn mixed_jar<W: Write + std::io::Seek>(zip: &mut ZipWriter<W>) {
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.add_directory("META-INF/", stored).expect("dir");
    zip.start_file("META-INF/MANIFEST.MF", deflated)
        .expect("file");
    zip.write_all(b"Manifest-Version: 1.0\r\n\r\n")
        .expect("write");
    zip.add_directory("com/foo/", deflated).expect("dir");
    zip.start_file("com/foo/Bar.class", deflated).expect("file");
    zip.write_all(&[0xCA; 500]).expect("write");
    zip.start_file("com/foo/data.bin", stored).expect("file");
    zip.write_all(b"stored bytes").expect("write");
    zip.start_file("empty.txt", deflated).expect("file");
}

fn zipwriter_jar() -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    mixed_jar(&mut zip);
    zip.finish().expect("finish").into_inner()
}

fn small_raw() -> Vec<RawEntry> {
    vec![
        RawEntry::new("a/", ""),
        RawEntry::new("a/A.class", "aaaa"),
        RawEntry::new("b.txt", "bee"),
    ]
}

fn zip64_opts() -> RawZip {
    RawZip {
        zip64: true,
        ..RawZip::default()
    }
}

fn zip64_raw() -> Vec<RawEntry> {
    small_raw()
        .into_iter()
        .map(|e| RawEntry { zip64: true, ..e })
        .collect()
}

fn script() -> Vec<u8> {
    let mut s = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n".to_vec();
    s.resize(99, b'#');
    s.push(b'\n');
    s
}

fn central_headers(bytes: &[u8]) -> Vec<usize> {
    bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"PK\x01\x02")
        .map(|(i, _)| i)
        .collect()
}

fn patch(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

fn with_entries(n: u32) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for i in 0..n {
        zip.start_file(format!("e/{i:05}"), opts).expect("file");
    }
    zip.finish().expect("finish").into_inner()
}

#[test]
fn zipwriter_jar_matches_zip() {
    let bytes = zipwriter_jar();
    let recs = assert_matches_zip(&bytes);
    assert_eq!(
        names(&recs),
        [
            "META-INF/",
            "META-INF/MANIFEST.MF",
            "com/foo/",
            "com/foo/Bar.class",
            "com/foo/data.bin",
            "empty.txt"
        ]
    );
    assert!(recs.iter().any(|r| r.method == 0 && r.size > 0));
    assert!(
        recs.iter()
            .any(|r| r.method == 8 && r.size > r.compressed_size)
    );
    assert_local_headers(&bytes, &recs);
}

#[test]
fn streamed_jar_with_data_descriptors_matches_zip() {
    let mut zip = ZipWriter::new_stream(Vec::new());
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("META-INF/MANIFEST.MF", opts).expect("file");
    zip.write_all(b"Manifest-Version: 1.0\r\n\r\n")
        .expect("write");
    zip.start_file("a/A.class", opts).expect("file");
    zip.write_all(&[7; 300]).expect("write");
    let bytes = zip.finish().expect("finish").into_inner();
    let recs = assert_matches_zip(&bytes);
    assert!(recs.iter().all(|r| r.descriptor && r.size > 0), "{recs:?}");
}

#[test]
fn entry_fields_and_is_dir() {
    let bytes = zipwriter_jar();
    let mut cd = CentralDirectory::new(Cursor::new(&bytes)).expect("open");
    assert_eq!(cd.len(), 6);
    assert!(!cd.is_empty());
    let first = cd.next_entry().expect("read").expect("entry");
    assert_eq!(first.name, "META-INF/");
    assert!(first.is_dir());
    assert_eq!(first.header_offset, 0);
    let second = cd.next_entry().expect("read").expect("entry");
    assert!(!second.is_dir());
    assert_eq!(second.method, 8);
    assert_eq!(second.size, 25);
}

#[test]
fn reads_through_bufreader_file_and_gives_the_reader_back() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("a.jar");
    std::fs::write(&path, zipwriter_jar()).expect("write");
    let file = std::fs::File::open(&path).expect("open");
    let mut cd = CentralDirectory::new(BufReader::new(file)).expect("cdir");
    let mut count = 0;
    while let Some(e) = cd.next_entry().expect("entry") {
        assert!(!e.name.is_empty());
        count += 1;
    }
    assert_eq!(count, 6);
    let reader = cd.into_inner();
    assert_eq!(
        reader.get_ref().metadata().expect("meta").len(),
        zipwriter_jar().len() as u64
    );
}

#[test]
fn empty_archive() {
    let bytes = ZipWriter::new(Cursor::new(Vec::new()))
        .finish()
        .expect("finish")
        .into_inner();
    assert_eq!(bytes.len(), 22);
    let mut cd = CentralDirectory::new(Cursor::new(&bytes)).expect("open");
    assert_eq!(cd.len(), 0);
    assert!(cd.is_empty());
    assert!(cd.next_entry().expect("read").is_none());
    assert!(cd.next_entry().expect("read").is_none());
}

#[test]
fn empty_archive_ignores_offset() {
    let bytes = raw_zip(
        &[],
        &RawZip {
            cd_offset: Some(12_345),
            cd_size: Some(99),
            ..RawZip::default()
        },
    );
    assert_eq!(read_ok(&bytes), []);
}

#[test]
fn eocd_comment_is_skipped() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    mixed_jar(&mut zip);
    zip.set_comment("built by a tool").expect("comment");
    let bytes = zip.finish().expect("finish").into_inner();
    assert_eq!(assert_matches_zip(&bytes).len(), 6);
}

#[test]
fn eocd_comment_containing_the_signature() {
    let mut long = vec![b'x'; 5000];
    long.extend_from_slice(b"PK\x05\x06 near the end");
    let mut max = vec![b'y'; 65_535 - 30];
    max.extend_from_slice(b"PK\x05\x06PK\x05\x06");
    max.resize(65_535, b'z');
    for comment in [
        b"see PK\x05\x06 here".to_vec(),
        b"PK\x05\x06".to_vec(),
        long,
        max,
    ] {
        let opts = RawZip {
            comment: comment.clone(),
            ..RawZip::default()
        };
        let bytes = raw_zip(&small_raw(), &opts);
        let recs = assert_matches_zip(&bytes);
        assert_eq!(
            names(&recs),
            ["a/", "a/A.class", "b.txt"],
            "{}",
            comment.len()
        );
    }
}

#[test]
fn zip64_end_record_with_65536_entries() {
    let bytes = with_entries(65_536);
    let eocd = bytes.len() - 22;
    assert_eq!(&bytes[eocd + 10..eocd + 12], [0xFF, 0xFF]);
    assert_eq!(&bytes[eocd - 20..eocd - 16], b"PK\x06\x07");
    let mut cd = CentralDirectory::new(Cursor::new(&bytes)).expect("open");
    assert_eq!(cd.len(), 65_536);
    let mut count = 0u32;
    let mut last = String::new();
    while let Some(e) = cd.next_entry().expect("entry") {
        assert_eq!(e.name, format!("e/{count:05}"));
        count += 1;
        last = e.name.to_string();
    }
    assert_eq!(count, 65_536);
    assert_eq!(last, "e/65535");
}

#[test]
fn exactly_65535_entries_without_locator_is_plain() {
    let bytes = with_entries(65_535);
    let eocd = bytes.len() - 22;
    assert_eq!(&bytes[eocd + 10..eocd + 12], [0xFF, 0xFF]);
    assert_ne!(&bytes[eocd - 20..eocd - 16], b"PK\x06\x07");
    let recs = read_ok(&bytes);
    assert_eq!(recs.len(), 65_535);
    assert_eq!(recs[65_534].name, "e/65534");
}

#[test]
fn zip64_sizes_from_large_file() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    zip.start_file("big.bin", opts).expect("file");
    zip.write_all(&[1; 1000]).expect("write");
    zip.start_file("small.txt", opts).expect("file");
    zip.write_all(b"hi").expect("write");
    let bytes = zip.finish().expect("finish").into_inner();
    let first = central_headers(&bytes)[0];
    assert_eq!(
        &bytes[first + 20..first + 28],
        [0xFF; 8],
        "Zip64 sizes in the header"
    );
    let recs = assert_matches_zip(&bytes);
    assert_eq!(recs[0].size, 1000);
    assert_eq!(recs[1].size, 2);
}

#[test]
fn zip64_hand_built_with_offsets() {
    let bytes = raw_zip(&zip64_raw(), &zip64_opts());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["a/", "a/A.class", "b.txt"]);
    assert_eq!(recs[1].size, 4);
    assert_eq!(recs[2].compressed_size, 3);
    assert!(recs[2].header_offset > 0);
    assert_local_headers(&bytes, &recs);
}

#[test]
fn zip64_extra_only_for_marked_fields() {
    let mut entries = small_raw();
    let offset = (30 + 2) + (30 + 9 + 4);
    entries[2].extra = common::zip64_extra(&[offset, 0xDEAD]);
    let mut bytes = raw_zip(&entries, &RawZip::default());
    let third = central_headers(&bytes)[2];
    patch(&mut bytes, third + 42, &ZIP64_MARK.to_le_bytes());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(recs[2].header_offset, offset);
    assert_eq!((recs[2].size, recs[2].compressed_size), (3, 3));
    assert_local_headers(&bytes, &recs);
}

#[test]
fn zip64_mark_without_extra_keeps_header_value() {
    let mut bytes = raw_zip(&small_raw(), &RawZip::default());
    let second = central_headers(&bytes)[1];
    patch(&mut bytes, second + 24, &ZIP64_MARK.to_le_bytes());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(recs[1].size, u64::from(ZIP64_MARK));
}

#[test]
fn prepended_data_before_a_normal_jar() {
    let mut bytes = script();
    assert_eq!(bytes.len(), 100);
    bytes.extend(zipwriter_jar());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(recs.len(), 6);
    assert_eq!(recs[0].header_offset, 100);
    assert_local_headers(&bytes, &recs);
}

#[test]
fn prepended_data_before_a_zip64_jar() {
    let opts = RawZip {
        prefix: script(),
        ..zip64_opts()
    };
    let bytes = raw_zip(&zip64_raw(), &opts);
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["a/", "a/A.class", "b.txt"]);
    assert_eq!(recs[0].header_offset, 100);
    assert_local_headers(&bytes, &recs);
}

#[test]
fn prepended_data_with_a_comment() {
    let opts = RawZip {
        prefix: script(),
        comment: b"comment".to_vec(),
        ..RawZip::default()
    };
    let bytes = raw_zip(&small_raw(), &opts);
    let recs = assert_matches_zip(&bytes);
    assert_local_headers(&bytes, &recs);
}

#[test]
fn utf8_names_with_and_without_flag() {
    let flagged = RawEntry {
        utf8: true,
        ..RawEntry::new("é/ü.class", "x")
    };
    let unflagged = RawEntry::new("日本/名前.class", "y");
    let bytes = raw_zip(&[flagged, unflagged], &RawZip::default());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["é/ü.class", "日本/名前.class"]);
}

#[test]
fn invalid_utf8_names_are_cp437() {
    let bytes = raw_zip(&[RawEntry::new(b"caf\x82.txt", "z")], &RawZip::default());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["café.txt"]);
}

#[test]
fn every_high_byte_matches_zip() {
    let name: Vec<u8> = (0x80..=0xFF).collect();
    let mut reversed = name.clone();
    reversed.reverse();
    let bytes = raw_zip(
        &[RawEntry::new(&name, ""), RawEntry::new(&reversed, "")],
        &RawZip::default(),
    );
    let recs = assert_matches_zip(&bytes);
    assert_eq!(recs[0].name.chars().count(), 128);
    assert!(recs[0].name.starts_with("ÇüéâäàåçêëèïîìÄÅ"));
    assert!(recs[0].name.ends_with("√ⁿ²■\u{a0}"));
}

#[test]
fn unicode_path_extra_replaces_the_name() {
    let raw = b"caf\x82.txt";
    let good = RawEntry {
        extra: unicode_path_extra(raw, "café-unicode.txt"),
        ..RawEntry::new(raw, "a")
    };
    let mut bad_extra = unicode_path_extra(b"other", "ignored.txt");
    bad_extra.extend(extra_field(0xCAFE, b"opaque"));
    let bad = RawEntry {
        extra: bad_extra,
        ..RawEntry::new("plain.txt", "b")
    };
    let ascii = RawEntry {
        extra: unicode_path_extra(b"libx.so", "libx.so"),
        ..RawEntry::new("libx.so", "c")
    };
    let bytes = raw_zip(&[good, bad, ascii], &RawZip::default());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["café-unicode.txt", "plain.txt", "libx.so"]);
}

#[test]
fn unicode_path_extra_with_invalid_utf8_is_ignored() {
    let raw = b"a.txt";
    let mut extra = extra_field(0x7075, &[1]);
    extra.extend(unicode_path_extra(raw, "x"));
    let last = extra.len() - 1;
    extra[last] = 0xFF;
    let bytes = raw_zip(
        &[RawEntry {
            extra,
            ..RawEntry::new(raw, "")
        }],
        &RawZip::default(),
    );
    assert_eq!(names(&read_ok(&bytes)), ["a.txt"]);
}

#[test]
fn local_name_and_extra_may_differ() {
    let entry = RawEntry {
        local_name: Some(b"local-only-name.txt".to_vec()),
        local_extra: extra_field(0xCAFE, &[0; 40]),
        ..RawEntry::new("central.txt", "data")
    };
    let bytes = raw_zip(&[entry, RawEntry::new("next.txt", "n")], &RawZip::default());
    let recs = assert_matches_zip(&bytes);
    assert_eq!(names(&recs), ["central.txt", "next.txt"]);
    assert_local_headers(&bytes, &recs);
}

// `zip` rejects the archive here ("Extra field content truncated"); `cdir` keeps the header
// values instead, the one place it is more lenient, because `find` never needs the extras.
#[test]
fn malformed_extra_block_keeps_header_values() {
    let mut extra = unicode_path_extra(b"a.txt", "renamed.txt");
    extra.extend([0x34, 0x12, 0xFF, 0x00, 1, 2]);
    let entries = [
        RawEntry {
            extra,
            ..RawEntry::new("a.txt", "abc")
        },
        RawEntry {
            extra: vec![0x01, 0x00, 0x08],
            ..RawEntry::new("b.txt", "de")
        },
    ];
    let bytes = raw_zip(&entries, &RawZip::default());
    assert!(ZipArchive::new(Cursor::new(&bytes)).is_err());
    let recs = read_ok(&bytes);
    assert_eq!(names(&recs), ["renamed.txt", "b.txt"]);
    assert_eq!((recs[0].size, recs[1].size), (3, 2));
    assert_local_headers(&bytes, &recs);
}

#[test]
fn count_larger_than_records_present() {
    let entries = [
        RawEntry::new("x".repeat(30), ""),
        RawEntry::new("y".repeat(30), ""),
    ];
    let bytes = raw_zip(
        &entries,
        &RawZip {
            count: Some(3),
            ..RawZip::default()
        },
    );
    assert!(read_err(&bytes).contains("truncated or corrupt central directory"));

    let bytes = raw_zip(
        &small_raw(),
        &RawZip {
            count: Some(100),
            ..RawZip::default()
        },
    );
    assert!(read_err(&bytes).contains("can't fit"));
}

#[test]
fn cd_size_past_the_end_record() {
    let good = raw_zip(&small_raw(), &RawZip::default());
    let real = u64::from(u32::from_le_bytes(
        good[good.len() - 10..good.len() - 6]
            .try_into()
            .expect("4 bytes"),
    ));
    let bytes = raw_zip(
        &small_raw(),
        &RawZip {
            cd_size: Some(real + 1),
            ..RawZip::default()
        },
    );
    assert!(read_err(&bytes).contains("runs past its end record"));
}

#[test]
fn cd_offset_plus_size_overflows() {
    let bytes = raw_zip(
        &small_raw(),
        &RawZip {
            cd_offset: Some(u64::MAX - 10),
            ..zip64_opts()
        },
    );
    assert!(read_err(&bytes).contains("runs past its end record"));
}

#[test]
fn central_directory_not_found() {
    let good = raw_zip(&small_raw(), &RawZip::default());
    let real = u64::from(u32::from_le_bytes(
        good[good.len() - 10..good.len() - 6]
            .try_into()
            .expect("4 bytes"),
    ));
    let bytes = raw_zip(
        &small_raw(),
        &RawZip {
            cd_offset: Some(0),
            cd_size: Some(real - 1),
            ..RawZip::default()
        },
    );
    assert!(read_err(&bytes).contains("central directory not found"));
}

#[test]
fn central_header_with_bad_signature() {
    let mut bytes = raw_zip(&small_raw(), &RawZip::default());
    let second = central_headers(&bytes)[1];
    patch(&mut bytes, second + 2, &[0, 0]);
    assert!(read_err(&bytes).contains("truncated or corrupt central directory"));
}

#[test]
fn name_length_past_the_directory() {
    let mut bytes = raw_zip(&small_raw(), &RawZip::default());
    let last = central_headers(&bytes)[2];
    patch(&mut bytes, last + 28, &[0xFF, 0xFF]);
    assert!(read_err(&bytes).contains("truncated or corrupt central directory"));
}

#[test]
fn error_in_the_middle_stops_the_iteration() {
    let mut bytes = raw_zip(&small_raw(), &RawZip::default());
    let second = central_headers(&bytes)[1];
    patch(&mut bytes, second, b"XXXX");
    let mut cd = CentralDirectory::new(Cursor::new(&bytes)).expect("open");
    assert_eq!(cd.next_entry().expect("first").expect("entry").name, "a/");
    assert!(cd.next_entry().is_err());
    assert!(cd.next_entry().expect("no more").is_none());
}

#[test]
fn zip64_extra_field_missing_a_value() {
    let mut entries = zip64_raw();
    entries[1].zip64_values = Some(vec![4, 4]);
    let bytes = raw_zip(&entries, &zip64_opts());
    assert!(read_err(&bytes).contains("Zip64 extra field is missing a value"));
}

#[test]
fn truncated_files_never_panic() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    mixed_jar(&mut zip);
    zip.set_comment("note").expect("comment");
    let plain = zip.finish().expect("finish").into_inner();
    let zip64 = raw_zip(
        &zip64_raw(),
        &RawZip {
            prefix: script(),
            ..zip64_opts()
        },
    );
    for bytes in [plain, zip64] {
        for cut in 0..bytes.len() {
            assert!(read_all(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        read_ok(&bytes);
    }
}

#[test]
fn multi_disk_values_are_rejected() {
    let plain = raw_zip(&small_raw(), &RawZip::default());
    let eocd = plain.len() - 22;
    for (at, value) in [(4, 1u16), (6, 1), (8, 2)] {
        let mut bytes = plain.clone();
        patch(&mut bytes, eocd + at, &value.to_le_bytes());
        assert!(read_err(&bytes).contains("multi-disk"), "EOCD +{at}");
    }

    let zip64 = raw_zip(&small_raw(), &zip64_opts());
    let locator = zip64.len() - 22 - 20;
    let record = locator - 56;
    for (at, value) in [(16, 1u64), (20, 1), (24, 2)] {
        let mut bytes = zip64.clone();
        let width = if at == 24 { 8 } else { 4 };
        patch(&mut bytes, record + at, &value.to_le_bytes()[..width]);
        assert!(
            read_err(&bytes).contains("multi-disk"),
            "Zip64 record +{at}"
        );
    }
    let mut bytes = zip64.clone();
    patch(&mut bytes, locator + 16, &2u32.to_le_bytes());
    assert!(read_err(&bytes).contains("multi-disk"));
}

#[test]
fn zip64_locator_pointing_past_itself() {
    let zip64 = raw_zip(&small_raw(), &zip64_opts());
    let locator = zip64.len() - 22 - 20;
    for stored in [locator as u64 - 55, locator as u64, u64::MAX - 1] {
        let mut bytes = zip64.clone();
        patch(&mut bytes, locator + 8, &stored.to_le_bytes());
        assert!(read_err(&bytes).contains("points past itself"), "{stored}");
    }
}

#[test]
fn zip64_record_missing() {
    let mut bytes = raw_zip(&small_raw(), &zip64_opts());
    let record = bytes.len() - 22 - 20 - 56;
    patch(&mut bytes, record, b"XXXX");
    assert!(read_err(&bytes).contains("Zip64 end of central directory record not found"));
}

#[test]
fn huge_zip64_count_is_rejected_up_front() {
    let opts = RawZip {
        count: Some(u64::MAX),
        ..zip64_opts()
    };
    let bytes = raw_zip(&[RawEntry::new("", "")], &opts);
    let err = CentralDirectory::new(Cursor::new(&bytes))
        .err()
        .expect("new() fails");
    assert!(format!("{err:#}").contains("can't fit in a central directory of 46 bytes"));
}

#[test]
fn garbage_inputs_are_errors() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let random: Vec<u8> = (0..10_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let text = "just some text, not a jar\n".repeat(200).into_bytes();
    for bytes in [random, text, Vec::new(), b"PK\x05\x06".to_vec()] {
        assert!(
            read_err(&bytes).contains("no end of central directory record found"),
            "{} bytes",
            bytes.len()
        );
    }
}

fn infos(bytes: &[u8]) -> Vec<EntryInfo> {
    let mut cd = CentralDirectory::new(Cursor::new(bytes)).expect("open");
    let mut out = Vec::new();
    while let Some(e) = cd.next_entry().expect("entry") {
        out.push(e.to_info());
    }
    out
}

fn read_with(bytes: &[u8], info: &EntryInfo, cap: u64) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    read_entry(&mut Cursor::new(bytes), info, cap, &mut out)?;
    Ok(out)
}

fn read_entry_err(bytes: &[u8], info: &EntryInfo) -> String {
    format!(
        "{:#}",
        read_with(bytes, info, u64::MAX / 2).expect_err("must fail")
    )
}

fn deflated(entries: &[(&str, &[u8])], stream: bool) -> Vec<u8> {
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut buf = Vec::new();
    if stream {
        let mut zip = ZipWriter::new_stream(&mut buf);
        for (name, data) in entries {
            zip.start_file(*name, opts).expect("file");
            zip.write_all(data).expect("write");
        }
        zip.finish().expect("finish");
    } else {
        let mut zip = ZipWriter::new(Cursor::new(&mut buf));
        for (name, data) in entries {
            zip.start_file(*name, opts).expect("file");
            zip.write_all(data).expect("write");
        }
        zip.finish().expect("finish");
    }
    buf
}

#[test]
fn reads_stored_and_deflated_entries() {
    let text = b"Manifest-Version: 1.0\r\n\r\n".repeat(20);
    let bytes = zipwriter_jar();
    let mut zip = ZipArchive::new(Cursor::new(&bytes)).expect("zip");
    for info in infos(&bytes) {
        let mut want = Vec::new();
        std::io::Read::read_to_end(&mut zip.by_name(&info.name).expect("by_name"), &mut want)
            .expect("read");
        assert_eq!(
            read_with(&bytes, &info, 1 << 20).expect("read"),
            want,
            "{}",
            info.name
        );
    }
    let bytes = deflated(&[("a.txt", &text)], false);
    assert_eq!(
        read_with(&bytes, &infos(&bytes)[0], 1 << 20).expect("read"),
        text
    );
}

#[test]
fn reads_data_descriptor_entries() {
    let data = vec![7u8; 300];
    let bytes = deflated(&[("a/A.class", &data), ("b.txt", b"hello")], true);
    let infos = infos(&bytes);
    assert!(infos.iter().all(|i| i.flags & 8 != 0));
    assert_eq!(read_with(&bytes, &infos[0], 1000).expect("read"), data);
    assert_eq!(read_with(&bytes, &infos[1], 1000).expect("read"), b"hello");
}

#[test]
fn local_lengths_may_differ_from_central() {
    let mut entry = RawEntry::new("a.txt", b"payload");
    entry.local_name = Some(b"a-much-longer-local-name.txt".to_vec());
    entry.local_extra = vec![0xAA; 11];
    let bytes = raw_zip(&[entry], &RawZip::default());
    assert_eq!(
        read_with(&bytes, &infos(&bytes)[0], 100).expect("read"),
        b"payload"
    );
}

#[test]
fn prepended_data_shifts_the_offset() {
    let opts = RawZip {
        prefix: script(),
        ..RawZip::default()
    };
    let bytes = raw_zip(
        &[RawEntry::new("a", b"one"), RawEntry::new("b", b"two")],
        &opts,
    );
    let infos = infos(&bytes);
    assert_eq!(read_with(&bytes, &infos[0], 100).expect("read"), b"one");
    assert_eq!(read_with(&bytes, &infos[1], 100).expect("read"), b"two");
}

#[test]
fn cap_boundaries() {
    let data = vec![b'x'; 100];
    for bytes in [
        raw_zip(&[RawEntry::new("a", &data)], &RawZip::default()),
        deflated(&[("a", &data)], false),
    ] {
        let info = &infos(&bytes)[0];
        assert_eq!(read_with(&bytes, info, 100).expect("at cap"), data);
        assert_eq!(read_with(&bytes, info, 99).expect("over cap").len(), 100);
        assert_eq!(read_with(&bytes, info, 10).expect("far over").len(), 11);
        assert_eq!(read_with(&bytes, info, 0).expect("zero").len(), 1);
    }
}

#[test]
fn zip_bomb_stops_at_cap_plus_one() {
    let zeros = vec![0u8; 10 << 20];
    let bytes = deflated(&[("bomb", &zeros)], false);
    let info = &infos(&bytes)[0];
    assert!(info.compressed_size < 100_000);
    let cap = 1 << 20;
    assert_eq!(
        read_with(&bytes, info, cap).expect("read").len() as u64,
        cap + 1
    );
}

#[test]
fn appends_to_existing_output() {
    let bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    let mut out = b"keep".to_vec();
    read_entry(&mut Cursor::new(&bytes), &infos(&bytes)[0], 100, &mut out).expect("read");
    assert_eq!(out, b"keepdata");
}

#[test]
fn bad_local_signature_is_an_error() {
    let mut bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    assert_eq!(common::break_local_header_bytes(&mut bytes), 1);
    let err = read_entry_err(&bytes, &infos(&bytes)[0]);
    assert!(err.contains("invalid local header signature"), "{err}");
}

#[test]
fn local_header_past_the_end_is_an_error() {
    let bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    let mut info = infos(&bytes)[0].clone();
    info.header_offset = bytes.len() as u64 - 5;
    assert!(read_entry_err(&bytes, &info).contains("truncated"));
    info.header_offset = u64::MAX - 3;
    read_entry_err(&bytes, &info);
}

#[test]
fn wrong_crc_is_an_error() {
    let bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    let mut info = infos(&bytes)[0].clone();
    info.crc32 ^= 1;
    assert!(read_entry_err(&bytes, &info).contains("CRC-32 mismatch"));
    let bytes = deflated(&[("a", b"data")], false);
    let mut info = infos(&bytes)[0].clone();
    info.crc32 ^= 1;
    assert!(read_entry_err(&bytes, &info).contains("CRC-32 mismatch"));
}

#[test]
fn size_mismatch_is_an_error() {
    let bytes = deflated(&[("a", b"0123456789")], false);
    let good = infos(&bytes)[0].clone();
    for size in [5, 20] {
        let info = EntryInfo {
            size,
            ..good.clone()
        };
        let err = read_entry_err(&bytes, &info);
        assert!(err.contains("central directory says"), "{err}");
    }
    let bytes = raw_zip(&[RawEntry::new("a", b"0123456789")], &RawZip::default());
    let good = infos(&bytes)[0].clone();
    for compressed_size in [5, 1000] {
        let info = EntryInfo {
            compressed_size,
            ..good.clone()
        };
        read_entry_err(&bytes, &info);
    }
}

#[test]
fn truncated_deflate_stream_is_an_error() {
    let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
    let bytes = deflated(&[("a", &data)], false);
    let good = infos(&bytes)[0].clone();
    let info = EntryInfo {
        compressed_size: good.compressed_size / 2,
        ..good
    };
    read_entry_err(&bytes, &info);
}

#[test]
fn encrypted_entry_is_an_error() {
    let bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    let mut info = infos(&bytes)[0].clone();
    info.flags |= 1;
    assert!(read_entry_err(&bytes, &info).contains("encrypted entry"));
}

#[test]
fn unsupported_method_is_an_error() {
    let bytes = raw_zip(&[RawEntry::new("a", b"data")], &RawZip::default());
    let mut info = infos(&bytes)[0].clone();
    info.method = 12;
    assert!(read_entry_err(&bytes, &info).contains("unsupported compression method 12"));
}
