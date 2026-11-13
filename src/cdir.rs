use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

use anyhow::{Context, anyhow, bail};
use flate2::Crc;
use flate2::read::DeflateDecoder;

const EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
const ZIP64_LOCATOR_LEN: u64 = 20;
const ZIP64_EOCD_SIG: [u8; 4] = *b"PK\x06\x06";
const ZIP64_EOCD_LEN: usize = 56;
const CENTRAL_SIG: [u8; 4] = *b"PK\x01\x02";
const CENTRAL_LEN: usize = 46;
const LOCAL_SIG: [u8; 4] = *b"PK\x03\x04";
const LOCAL_LEN: usize = 30;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;
const FLAG_ENCRYPTED: u16 = 1;
// Initial reservation for `out`; the declared size is untrusted, so the buffer grows with real data.
const RESERVE_LIMIT: u64 = 64 * 1024;

const ZIP64_EXTRA: u16 = 0x0001;
const UNICODE_PATH_EXTRA: u16 = 0x7075;
const ZIP64_MARK: u32 = 0xFFFF_FFFF;

// Almost every JAR has no archive comment, so its EOCD is in the last 22 bytes; reading
// 64 KiB per JAR would multiply the I/O of folder scans.
const FIRST_WINDOW: u64 = 1024;
// The EOCD is 22 bytes plus a comment of at most 65,535 bytes, so it can't start earlier.
const MAX_WINDOW: u64 = EOCD_LEN as u64 + u16::MAX as u64;

const CORRUPT: &str = "truncated or corrupt central directory";
const MULTI_DISK: &str = "multi-disk archives are not supported";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub size: u64,
    pub compressed_size: u64,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub header_offset: u64,
}

impl Entry<'_> {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    pub fn to_info(&self) -> EntryInfo {
        EntryInfo {
            name: self.name.to_string(),
            size: self.size,
            compressed_size: self.compressed_size,
            method: self.method,
            flags: self.flags,
            crc32: self.crc32,
            header_offset: self.header_offset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub header_offset: u64,
}

pub fn read_entry<R: Read + Seek>(
    reader: &mut R,
    entry: &EntryInfo,
    cap: u64,
    out: &mut Vec<u8>,
) -> anyhow::Result<()> {
    if entry.flags & FLAG_ENCRYPTED != 0 {
        bail!("encrypted entry");
    }
    if !matches!(entry.method, METHOD_STORED | METHOD_DEFLATED) {
        bail!("unsupported compression method {}", entry.method);
    }

    let mut h = [0u8; LOCAL_LEN];
    reader.seek(SeekFrom::Start(entry.header_offset))?;
    reader.read_exact(&mut h).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => anyhow!("local header is truncated"),
        _ => e.into(),
    })?;
    if h[..4] != LOCAL_SIG {
        bail!("invalid local header signature");
    }
    // The local lengths can differ from the central ones, so only they locate the data.
    let data_start = entry
        .header_offset
        .checked_add(LOCAL_LEN as u64 + u64::from(le16(&h, 26)) + u64::from(le16(&h, 28)))
        .context("local header offset is too large")?;
    reader.seek(SeekFrom::Start(data_start))?;

    let start = out.len();
    out.reserve(entry.size.min(cap).min(RESERVE_LIMIT) as usize);
    let data = reader.take(entry.compressed_size);
    let limit = cap.saturating_add(1);
    let read = if entry.method == METHOD_STORED {
        data.take(limit).read_to_end(out)
    } else {
        DeflateDecoder::new(data).take(limit).read_to_end(out)
    }
    .map_err(|e| anyhow!(e))? as u64;

    // Past the cap the caller discards the data, so the checks would only reject a prefix.
    if read > cap {
        return Ok(());
    }
    if read != entry.size {
        bail!(
            "size is {read} bytes, central directory says {}",
            entry.size
        );
    }
    let mut crc = Crc::new();
    crc.update(&out[start..]);
    if crc.sum() != entry.crc32 {
        bail!("CRC-32 mismatch");
    }
    Ok(())
}

pub struct CentralDirectory<R> {
    reader: R,
    count: u64,
    remaining: u64,
    pos: u64,
    end: u64,
    shift: u64,
    // Name, extra and comment lengths are u16, so these buffers never exceed 64 KiB each:
    // the memory cap per reader no matter what the headers claim.
    name: Vec<u8>,
    extra: Vec<u8>,
    decoded: String,
}

struct Directory {
    count: u64,
    size: u64,
    offset: u64,
    end_pos: u64,
}

struct Record {
    size: u64,
    compressed_size: u64,
    header_offset: u64,
    method: u16,
    flags: u16,
    crc32: u32,
    unicode_name: Option<Range<usize>>,
}

impl<R: Read + Seek> CentralDirectory<R> {
    pub fn new(mut reader: R) -> anyhow::Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let (eocd_pos, eocd) = find_eocd(&mut reader, file_len)?;
        let entries = le16(&eocd, 10);
        let size = le32(&eocd, 12);
        let offset = le32(&eocd, 16);

        let zip64 = if entries == u16::MAX || size == ZIP64_MARK || offset == ZIP64_MARK {
            read_zip64(&mut reader, eocd_pos)?
        } else {
            None
        };
        let dir = match zip64 {
            Some(dir) => dir,
            None => {
                if le16(&eocd, 4) != 0 || le16(&eocd, 6) != 0 || le16(&eocd, 8) != entries {
                    bail!(MULTI_DISK);
                }
                Directory {
                    count: entries.into(),
                    size: size.into(),
                    offset: offset.into(),
                    end_pos: eocd_pos,
                }
            }
        };

        if dir.count == 0 {
            return Ok(Self::with(reader, 0, 0, 0, 0));
        }
        if dir.count > dir.size / CENTRAL_LEN as u64 {
            bail!(
                "{} entries can't fit in a central directory of {} bytes",
                dir.count,
                dir.size
            );
        }
        let cd_end = dir
            .offset
            .checked_add(dir.size)
            .filter(|&end| end <= dir.end_pos)
            .context("central directory runs past its end record")?;

        // Data prepended to the archive (e.g. a launch script) shifts every offset; the
        // directory normally ends right where its end record starts, which gives the shift.
        let mut shift = dir.end_pos - cd_end;
        if !probe(&mut reader, dir.offset + shift, CENTRAL_SIG)? {
            if shift != 0 && probe(&mut reader, dir.offset, CENTRAL_SIG)? {
                shift = 0;
            } else {
                bail!("central directory not found");
            }
        }
        let start = dir.offset + shift;
        Ok(Self::with(
            reader,
            dir.count,
            start,
            start + dir.size,
            shift,
        ))
    }

    fn with(reader: R, count: u64, pos: u64, end: u64, shift: u64) -> Self {
        Self {
            reader,
            count,
            remaining: count,
            pos,
            end,
            shift,
            name: Vec::new(),
            extra: Vec::new(),
            decoded: String::new(),
        }
    }

    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    pub fn next_entry(&mut self) -> anyhow::Result<Option<Entry<'_>>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let record = match self.read_record() {
            Ok(record) => record,
            Err(e) => {
                self.remaining = 0;
                return Err(e);
            }
        };
        self.remaining -= 1;
        let name = decode_name(
            &self.name,
            &self.extra,
            record.unicode_name,
            &mut self.decoded,
        );
        Ok(Some(Entry {
            name,
            size: record.size,
            compressed_size: record.compressed_size,
            method: record.method,
            flags: record.flags,
            crc32: record.crc32,
            header_offset: record.header_offset,
        }))
    }

    fn read_record(&mut self) -> anyhow::Result<Record> {
        let header_end = self.pos + CENTRAL_LEN as u64;
        if header_end > self.end {
            bail!(CORRUPT);
        }
        let mut h = [0u8; CENTRAL_LEN];
        read_exact(&mut self.reader, &mut h)?;
        if h[..4] != CENTRAL_SIG {
            bail!(CORRUPT);
        }
        let name_len = le16(&h, 28);
        let extra_len = le16(&h, 30);
        let comment_len = le16(&h, 32);
        let record_end =
            header_end + u64::from(name_len) + u64::from(extra_len) + u64::from(comment_len);
        if record_end > self.end {
            bail!(CORRUPT);
        }
        self.name.resize(name_len.into(), 0);
        read_exact(&mut self.reader, &mut self.name)?;
        self.extra.resize(extra_len.into(), 0);
        read_exact(&mut self.reader, &mut self.extra)?;
        if comment_len > 0 {
            self.reader.seek_relative(comment_len.into())?;
        }
        self.pos = record_end;

        let mut record = Record {
            size: le32(&h, 24).into(),
            compressed_size: le32(&h, 20).into(),
            header_offset: le32(&h, 42).into(),
            method: le16(&h, 10),
            flags: le16(&h, 8),
            crc32: le32(&h, 16),
            unicode_name: None,
        };
        apply_extras(&mut record, &self.extra, &self.name)?;
        record.header_offset = record
            .header_offset
            .checked_add(self.shift)
            .context("local header offset is too large")?;
        Ok(record)
    }
}

fn find_eocd<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> anyhow::Result<(u64, [u8; EOCD_LEN])> {
    let mut buf = Vec::new();
    for window in [FIRST_WINDOW, MAX_WINDOW] {
        let len = window.min(file_len);
        let base = file_len - len;
        buf.resize(len as usize, 0);
        reader.seek(SeekFrom::Start(base))?;
        reader.read_exact(&mut buf)?;
        if let Some((i, eocd)) = last_eocd(&buf) {
            return Ok((base + i as u64, eocd));
        }
        if len == file_len {
            break;
        }
    }
    bail!("no end of central directory record found")
}

fn last_eocd(buf: &[u8]) -> Option<(usize, [u8; EOCD_LEN])> {
    let last = buf.len().checked_sub(EOCD_LEN)?;
    (0..=last).rev().find_map(|i| {
        if buf.get(i..i + 4)? != EOCD_SIG {
            return None;
        }
        let eocd: [u8; EOCD_LEN] = buf.get(i..i + EOCD_LEN)?.try_into().ok()?;
        let comment_len = usize::from(le16(&eocd, 20));
        (i + EOCD_LEN + comment_len <= buf.len()).then_some((i, eocd))
    })
}

// A count of 0xFFFF or a size / offset of 0xFFFFFFFF only *may* mean Zip64: a plain archive
// with exactly 65,535 entries has no locator and keeps its 32-bit values.
fn read_zip64<R: Read + Seek>(reader: &mut R, eocd_pos: u64) -> anyhow::Result<Option<Directory>> {
    let Some(locator_pos) = eocd_pos.checked_sub(ZIP64_LOCATOR_LEN) else {
        return Ok(None);
    };
    let mut locator = [0u8; ZIP64_LOCATOR_LEN as usize];
    reader.seek(SeekFrom::Start(locator_pos))?;
    read_exact(reader, &mut locator)?;
    if locator[..4] != ZIP64_LOCATOR_SIG {
        return Ok(None);
    }
    if le32(&locator, 16) > 1 {
        bail!(MULTI_DISK);
    }
    let stored = le64(&locator, 8);
    if stored
        .checked_add(ZIP64_EOCD_LEN as u64)
        .is_none_or(|end| end > locator_pos)
    {
        bail!("Zip64 end of central directory locator points past itself");
    }

    let mut record = [0u8; ZIP64_EOCD_LEN];
    let mut record_pos = stored;
    reader.seek(SeekFrom::Start(record_pos))?;
    read_exact(reader, &mut record)?;
    if record[..4] != ZIP64_EOCD_SIG {
        // With prepended data the stored offset is short by the shift; the record then sits
        // right before the locator.
        record_pos = locator_pos - ZIP64_EOCD_LEN as u64;
        reader.seek(SeekFrom::Start(record_pos))?;
        read_exact(reader, &mut record)?;
        if record[..4] != ZIP64_EOCD_SIG {
            bail!("Zip64 end of central directory record not found");
        }
    }
    let count = le64(&record, 32);
    if le32(&record, 16) != 0 || le32(&record, 20) != 0 || le64(&record, 24) != count {
        bail!(MULTI_DISK);
    }
    Ok(Some(Directory {
        count,
        size: le64(&record, 40),
        offset: le64(&record, 48),
        end_pos: record_pos,
    }))
}

fn probe<R: Read + Seek>(reader: &mut R, pos: u64, sig: [u8; 4]) -> io::Result<bool> {
    let mut buf = [0u8; 4];
    reader.seek(SeekFrom::Start(pos))?;
    match reader.read_exact(&mut buf) {
        Ok(()) if buf == sig => {
            reader.seek_relative(-4)?;
            Ok(true)
        }
        Ok(()) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8]) -> anyhow::Result<()> {
    reader.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            anyhow!(CORRUPT)
        } else {
            e.into()
        }
    })
}

// A malformed block (length past the end) stops parsing and keeps the header values; `zip`
// rejects the archive instead, but `find` never needs the extras.
fn apply_extras(record: &mut Record, extra: &[u8], raw_name: &[u8]) -> anyhow::Result<()> {
    let header = (record.size, record.compressed_size, record.header_offset);
    let mut name_crc = None;
    let mut at = 0;
    while let Some((head, rest)) = extra.get(at..).and_then(<[u8]>::split_first_chunk::<4>) {
        let id = u16::from_le_bytes([head[0], head[1]]);
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        let Some(data) = rest.get(..len) else {
            break;
        };
        let data_start = at + 4;
        match id {
            // Only the fields whose header value is 0xFFFFFFFF are present, always in this
            // order (APPNOTE 4.5.3).
            ZIP64_EXTRA => {
                let mut values = data;
                for (raw, value) in [
                    (header.0, &mut record.size),
                    (header.1, &mut record.compressed_size),
                    (header.2, &mut record.header_offset),
                ] {
                    if raw == u64::from(ZIP64_MARK) {
                        let (v, rest) = values
                            .split_first_chunk::<8>()
                            .context("Zip64 extra field is missing a value")?;
                        *value = u64::from_le_bytes(*v);
                        values = rest;
                    }
                }
            }
            UNICODE_PATH_EXTRA => {
                if let Some((crc, content)) = data.get(1..).and_then(<[u8]>::split_first_chunk)
                    && u32::from_le_bytes(*crc)
                        == *name_crc.get_or_insert_with(|| {
                            let mut crc = Crc::new();
                            crc.update(raw_name);
                            crc.sum()
                        })
                    && std::str::from_utf8(content).is_ok()
                {
                    record.unicode_name = Some(data_start + 5..data_start + len);
                }
            }
            _ => {}
        }
        at = data_start + len;
    }
    Ok(())
}

// `zip` decodes valid UTF-8 as UTF-8 whether or not flag bit 11 is set, and only falls back
// to CP437 for invalid UTF-8; doing the same yields the same names.
fn decode_name<'a>(
    raw: &'a [u8],
    extra: &'a [u8],
    unicode: Option<Range<usize>>,
    decoded: &'a mut String,
) -> &'a str {
    if let Some(name) = unicode
        .and_then(|range| extra.get(range))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    {
        return name;
    }
    if let Ok(name) = std::str::from_utf8(raw) {
        return name;
    }
    decoded.clear();
    decoded.extend(raw.iter().map(|&b| cp437(b)));
    decoded
}

fn cp437(b: u8) -> char {
    match b.checked_sub(0x80) {
        Some(i) => CP437_HIGH[usize::from(i)],
        None => char::from(b),
    }
}

#[rustfmt::skip]
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

fn le16<const N: usize>(b: &[u8; N], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32<const N: usize>(b: &[u8; N], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64<const N: usize>(b: &[u8; N], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp437_upper_half() {
        assert_eq!(cp437(0x80), 'Ç');
        assert_eq!(cp437(0x82), 'é');
        assert_eq!(cp437(0x9E), '₧');
        assert_eq!(cp437(0xB0), '░');
        assert_eq!(cp437(0xE1), 'ß');
        assert_eq!(cp437(0xE6), 'µ');
        assert_eq!(cp437(0xFA), '·');
        assert_eq!(cp437(0xFF), '\u{a0}');
    }

    #[test]
    fn cp437_lower_half_is_ascii() {
        for b in 0..0x80u8 {
            assert_eq!(cp437(b), char::from(b));
        }
    }

    #[test]
    fn cp437_upper_half_is_distinct_non_ascii() {
        let mut chars: Vec<char> = CP437_HIGH.to_vec();
        assert!(chars.iter().all(|c| !c.is_ascii()));
        chars.sort_unstable();
        chars.dedup();
        assert_eq!(chars.len(), 128);
    }

    #[test]
    fn little_endian_readers() {
        let b = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(le16(&b, 0), 0x0201);
        assert_eq!(le16(&b, 8), 0x0A09);
        assert_eq!(le32(&b, 1), 0x0504_0302);
        assert_eq!(le64(&b, 2), 0x0A09_0807_0605_0403);
        assert_eq!(le32(&[0xFF; 4], 0), ZIP64_MARK);
    }

    #[test]
    fn last_eocd_skips_candidates_that_do_not_fit() {
        let mut buf = vec![0u8; 30];
        buf[..4].copy_from_slice(&EOCD_SIG);
        buf[26..30].copy_from_slice(&EOCD_SIG);
        assert_eq!(last_eocd(&buf).map(|(i, _)| i), Some(0));
        buf[20] = 9;
        assert_eq!(last_eocd(&buf), None);
        assert_eq!(last_eocd(&buf[..21]), None);
    }
}
