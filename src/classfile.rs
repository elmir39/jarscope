use std::fmt;

const MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassHeader {
    pub minor: u16,
    pub major: u16,
}

impl ClassHeader {
    // Preview features exist only since Java 12 (major 56).
    pub fn is_preview(&self) -> bool {
        self.minor == 0xFFFF && self.major >= 56
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    Truncated,
    BadMagic,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HeaderError::Truncated => "truncated class header",
            HeaderError::BadMagic => "bad magic",
        })
    }
}

impl std::error::Error for HeaderError {}

pub fn parse_header(bytes: &[u8]) -> Result<ClassHeader, HeaderError> {
    match bytes {
        [a, b, c, d, mi0, mi1, ma0, ma1, ..] => {
            if [*a, *b, *c, *d] != MAGIC {
                return Err(HeaderError::BadMagic);
            }
            Ok(ClassHeader {
                minor: u16::from_be_bytes([*mi0, *mi1]),
                major: u16::from_be_bytes([*ma0, *ma1]),
            })
        }
        _ => Err(HeaderError::Truncated),
    }
}

pub fn java_version(major: u16) -> Option<String> {
    match major {
        0..=44 => None,
        45 => Some("1.0/1.1".to_string()),
        46..=48 => Some(format!("1.{}", major - 44)),
        _ => Some((major - 44).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(minor: u16, major: u16) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&minor.to_be_bytes());
        v.extend_from_slice(&major.to_be_bytes());
        v
    }

    #[test]
    fn parses_java8_header() {
        let bytes = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x34];
        let header = parse_header(&bytes).expect("valid header");
        assert_eq!(
            header,
            ClassHeader {
                minor: 0,
                major: 52
            }
        );
        assert_eq!(java_version(header.major).as_deref(), Some("8"));
    }

    #[test]
    fn future_versions_map_to_release_numbers() {
        assert_eq!(java_version(69).as_deref(), Some("25"));
        assert_eq!(java_version(70).as_deref(), Some("26"));
    }

    #[test]
    fn legacy_versions_map_to_1_x_names() {
        assert_eq!(java_version(45).as_deref(), Some("1.0/1.1"));
        assert_eq!(java_version(46).as_deref(), Some("1.2"));
        assert_eq!(java_version(47).as_deref(), Some("1.3"));
        assert_eq!(java_version(48).as_deref(), Some("1.4"));
        assert_eq!(java_version(49).as_deref(), Some("5"));
    }

    #[test]
    fn major_below_45_is_unknown() {
        assert_eq!(java_version(44), None);
        assert_eq!(java_version(0), None);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let bytes = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x34];
        assert_eq!(parse_header(&bytes), Err(HeaderError::BadMagic));
    }

    #[test]
    fn short_input_is_truncated() {
        assert_eq!(parse_header(&[]), Err(HeaderError::Truncated));
        assert_eq!(parse_header(&MAGIC), Err(HeaderError::Truncated));
        assert_eq!(
            parse_header(&header_bytes(0, 52)[..7]),
            Err(HeaderError::Truncated)
        );
    }

    #[test]
    fn seven_garbage_bytes_are_truncated_not_bad_magic() {
        assert_eq!(parse_header(&[0xFF; 7]), Err(HeaderError::Truncated));
    }

    #[test]
    fn only_first_eight_bytes_are_parsed() {
        let mut bytes = header_bytes(0, 61);
        bytes.extend_from_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(
            parse_header(&bytes),
            Ok(ClassHeader {
                minor: 0,
                major: 61
            })
        );
    }

    #[test]
    fn error_messages() {
        assert_eq!(HeaderError::Truncated.to_string(), "truncated class header");
        assert_eq!(HeaderError::BadMagic.to_string(), "bad magic");
    }

    #[test]
    fn preview_requires_ffff_minor_and_major_56_or_later() {
        let preview = ClassHeader {
            minor: 0xFFFF,
            major: 69,
        };
        let minor_zero = ClassHeader {
            minor: 0,
            major: 69,
        };
        let before_java12 = ClassHeader {
            minor: 0xFFFF,
            major: 52,
        };
        assert!(preview.is_preview());
        assert!(!minor_zero.is_preview());
        assert!(!before_java12.is_preview());
    }
}
