//! Lossless platform paths (CLI-04). Windows paths keep their UTF-16 code units (unpaired
//! surrogates included); macOS paths keep raw bytes. The display string is derived for people
//! and is never parsed back into an execution path.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::capability::Platform;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RawPath {
    /// UTF-16LE code units exactly as returned by the Windows API.
    Windows(Vec<u16>),
    /// Raw bytes exactly as returned by the macOS API.
    Unix(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path contains NUL")]
    Nul,
    #[error("raw_encoding does not match platform")]
    EncodingMismatch,
    #[error("raw_base64 is not valid base64")]
    BadBase64,
    #[error("UTF-16 raw path has an odd byte length")]
    OddUtf16Length,
}

impl RawPath {
    pub fn windows(units: Vec<u16>) -> Result<Self, PathError> {
        if units.is_empty() {
            return Err(PathError::Empty);
        }
        if units.contains(&0) {
            return Err(PathError::Nul);
        }
        Ok(RawPath::Windows(units))
    }

    pub fn unix(bytes: Vec<u8>) -> Result<Self, PathError> {
        if bytes.is_empty() {
            return Err(PathError::Empty);
        }
        if bytes.contains(&0) {
            return Err(PathError::Nul);
        }
        Ok(RawPath::Unix(bytes))
    }

    pub fn platform(&self) -> Platform {
        match self {
            RawPath::Windows(_) => Platform::Windows,
            RawPath::Unix(_) => Platform::Macos,
        }
    }

    pub fn raw_encoding(&self) -> &'static str {
        match self {
            RawPath::Windows(_) => "windows_utf16le_base64",
            RawPath::Unix(_) => "unix_bytes_base64",
        }
    }

    pub fn raw_bytes(&self) -> Vec<u8> {
        match self {
            RawPath::Windows(units) => units.iter().flat_map(|u| u.to_le_bytes()).collect(),
            RawPath::Unix(bytes) => bytes.clone(),
        }
    }

    pub fn raw_base64(&self) -> String {
        STANDARD.encode(self.raw_bytes())
    }

    /// Lossy text without escaping, for literal name matching only (never for display or
    /// execution: invalid sequences become U+FFFD and cannot match a literal pattern).
    pub fn lossy(&self) -> String {
        match self {
            RawPath::Windows(units) => String::from_utf16_lossy(units),
            RawPath::Unix(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        }
    }

    /// Append one child name with the platform separator, without re-parsing any text.
    pub fn join(&self, name: &RawPath) -> Result<RawPath, PathError> {
        match (self, name) {
            (RawPath::Windows(p), RawPath::Windows(n)) => {
                let mut units = p.clone();
                if units.last() != Some(&(b'\\' as u16)) {
                    units.push(b'\\' as u16);
                }
                units.extend_from_slice(n);
                Ok(RawPath::Windows(units))
            }
            (RawPath::Unix(p), RawPath::Unix(n)) => {
                let mut bytes = p.clone();
                if bytes.last() != Some(&b'/') {
                    bytes.push(b'/');
                }
                bytes.extend_from_slice(n);
                Ok(RawPath::Unix(bytes))
            }
            _ => Err(PathError::EncodingMismatch),
        }
    }

    /// Human-readable text. Invalid sequences become U+FFFD and control characters are shown
    /// as escapes so a file name cannot rewrite the surrounding UI or log line.
    pub fn display(&self) -> String {
        let lossy = match self {
            RawPath::Windows(units) => String::from_utf16_lossy(units),
            RawPath::Unix(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        };
        lossy
            .chars()
            .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
                if c.is_control() { Box::new(c.escape_unicode()) } else { Box::new(std::iter::once(c)) }
            })
            .collect()
    }

    fn from_wire(platform: Platform, encoding: &str, b64: &str) -> Result<Self, PathError> {
        let bytes = STANDARD.decode(b64).map_err(|_| PathError::BadBase64)?;
        match (platform, encoding) {
            (Platform::Windows, "windows_utf16le_base64") => {
                if bytes.len() % 2 != 0 {
                    return Err(PathError::OddUtf16Length);
                }
                RawPath::windows(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
            }
            (Platform::Macos, "unix_bytes_base64") => RawPath::unix(bytes),
            _ => Err(PathError::EncodingMismatch),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePath {
    platform: Platform,
    display: String,
    raw_encoding: String,
    raw_base64: String,
}

impl Serialize for RawPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        WirePath {
            platform: self.platform(),
            display: self.display(),
            raw_encoding: self.raw_encoding().into(),
            raw_base64: self.raw_base64(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for RawPath {
    /// The received `display` is ignored: it may be redacted and is never an execution input.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = WirePath::deserialize(d)?;
        RawPath::from_wire(w.platform, &w.raw_encoding, &w.raw_base64).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(p: &RawPath) -> RawPath {
        serde_json::from_str(&serde_json::to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn windows_utf16_units_survive_including_unpaired_surrogates() {
        let mut units: Vec<u16> = "C:\\数据\\a b".encode_utf16().collect();
        units.push(0xD800); // unpaired high surrogate: legal in NTFS names, not valid Unicode
        let p = RawPath::windows(units.clone()).unwrap();
        assert_eq!(round_trip(&p), RawPath::Windows(units));
        assert!(p.display().contains('\u{FFFD}'), "display is lossy, raw is not");
    }

    #[test]
    fn unix_raw_bytes_survive_invalid_utf8() {
        let bytes = b"/Users/x/\xff\xfe-\xe4\xb8\xad".to_vec();
        let p = RawPath::unix(bytes.clone()).unwrap();
        assert_eq!(round_trip(&p), RawPath::Unix(bytes));
    }

    #[test]
    fn control_characters_are_escaped_only_in_display() {
        let p = RawPath::unix(b"/tmp/evil\x1b[2Jname\nnext".to_vec()).unwrap();
        let shown = p.display();
        assert!(!shown.contains('\u{1b}') && !shown.contains('\n'));
        assert_eq!(round_trip(&p).raw_bytes(), b"/tmp/evil\x1b[2Jname\nnext");
    }

    #[test]
    fn nul_empty_odd_length_and_mismatched_encodings_are_rejected() {
        assert_eq!(RawPath::unix(b"/a\0b".to_vec()), Err(PathError::Nul));
        assert_eq!(RawPath::windows(vec![]), Err(PathError::Empty));
        let odd = r#"{"platform":"windows","display":"x","raw_encoding":"windows_utf16le_base64","raw_base64":"QUJD"}"#;
        assert!(serde_json::from_str::<RawPath>(odd).is_err());
        let mismatch = r#"{"platform":"windows","display":"x","raw_encoding":"unix_bytes_base64","raw_base64":"L3RtcA=="}"#;
        assert!(serde_json::from_str::<RawPath>(mismatch).is_err());
        let nul16 = r#"{"platform":"windows","display":"x","raw_encoding":"windows_utf16le_base64","raw_base64":"QQAAAA=="}"#;
        assert!(serde_json::from_str::<RawPath>(nul16).is_err());
    }

    #[test]
    fn received_display_never_changes_the_target() {
        let honest = RawPath::unix(b"/Users/x/cache".to_vec()).unwrap();
        let mut wire: serde_json::Value = serde_json::to_value(&honest).unwrap();
        wire["display"] = serde_json::Value::from("/System");
        assert_eq!(serde_json::from_value::<RawPath>(wire).unwrap(), honest);
    }
}
