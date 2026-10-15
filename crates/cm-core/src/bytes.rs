//! Exact byte counts (CLI-04). JSON carries u64 values as decimal strings so JavaScript and
//! model clients never round them; free-space deltas are signed decimal strings.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

use crate::error::CoreError;

/// An exact, non-negative byte count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bytes(pub u64);

/// A signed byte difference whose magnitude fits in u64 (observed free-space delta).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SignedBytes(i128);

fn parse_digits(s: &str) -> Result<u64, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("byte count must be decimal digits, got {s:?}"));
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err(format!("byte count must not have leading zeros: {s:?}"));
    }
    s.parse::<u64>().map_err(|_| format!("byte count exceeds u64: {s}"))
}

impl Bytes {
    pub fn parse(s: &str) -> Result<Self, String> {
        parse_digits(s).map(Bytes)
    }

    pub fn checked_add(self, other: Bytes) -> Result<Bytes, CoreError> {
        self.0.checked_add(other.0).map(Bytes).ok_or(CoreError::ByteOverflow)
    }
}

impl SignedBytes {
    pub fn new(value: i128) -> Result<Self, CoreError> {
        if value.unsigned_abs() > u128::from(u64::MAX) { Err(CoreError::ByteOverflow) } else { Ok(SignedBytes(value)) }
    }

    /// `after - before` of two volume free-space observations; may be negative (REC-05).
    pub fn delta(before: Bytes, after: Bytes) -> Self {
        SignedBytes(i128::from(after.0) - i128::from(before.0))
    }

    pub fn get(self) -> i128 {
        self.0
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let (neg, digits) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let magnitude = parse_digits(digits)?;
        if neg && magnitude == 0 {
            return Err("negative zero is not a valid delta".into());
        }
        let v = i128::from(magnitude);
        Ok(SignedBytes(if neg { -v } else { v }))
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for SignedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl Serialize for SignedBytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

struct DecimalVisitor<T>(fn(&str) -> Result<T, String>);

impl<T> Visitor<'_> for DecimalVisitor<T> {
    type Value = T;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a decimal string (JSON numbers are rejected to avoid precision loss)")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.0)(v).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(DecimalVisitor(Bytes::parse))
    }
}

impl<'de> Deserialize<'de> for SignedBytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(DecimalVisitor(SignedBytes::parse))
    }
}

/// Sum of values where some may be unknown. Unknown values are counted, never treated as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnownTotal {
    pub known: Bytes,
    pub unknown_count: u64,
}

impl KnownTotal {
    pub fn add(&mut self, value: Option<Bytes>) -> Result<(), CoreError> {
        match value {
            Some(b) => self.known = self.known.checked_add(b)?,
            None => self.unknown_count += 1,
        }
        Ok(())
    }

    /// An exact total exists only when nothing is unknown.
    pub fn exact(&self) -> Option<Bytes> {
        (self.unknown_count == 0).then_some(self.known)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u64_bounds_round_trip_as_strings() {
        for v in [0, 1, (1u64 << 53) + 1, i64::MAX as u64 + 1, u64::MAX] {
            let json = serde_json::to_string(&Bytes(v)).unwrap();
            assert_eq!(json, format!("\"{v}\""));
            assert_eq!(serde_json::from_str::<Bytes>(&json).unwrap(), Bytes(v));
        }
    }

    #[test]
    fn rejects_numbers_overflow_and_malformed_strings() {
        assert!(serde_json::from_str::<Bytes>("104857600").is_err(), "JSON numbers are rejected");
        assert!(serde_json::from_str::<Bytes>("\"18446744073709551616\"").is_err());
        for bad in ["\"\"", "\"-1\"", "\"01\"", "\"1.0\"", "\"1e3\"", "\" 1\""] {
            assert!(serde_json::from_str::<Bytes>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn signed_delta_allows_negative_but_not_negative_zero() {
        let d = SignedBytes::delta(Bytes(100), Bytes(40));
        assert_eq!(serde_json::to_string(&d).unwrap(), "\"-60\"");
        assert_eq!(SignedBytes::delta(Bytes(0), Bytes(u64::MAX)).get(), i128::from(u64::MAX));
        assert!(SignedBytes::parse("-0").is_err());
        assert_eq!(SignedBytes::parse("-18446744073709551615").unwrap().get(), -i128::from(u64::MAX));
        assert!(SignedBytes::parse("-18446744073709551616").is_err());
    }

    #[test]
    fn totals_keep_unknowns_separate_and_detect_overflow() {
        let mut t = KnownTotal::default();
        t.add(Some(Bytes(5))).unwrap();
        t.add(None).unwrap();
        assert_eq!((t.known, t.unknown_count, t.exact()), (Bytes(5), 1, None));
        let mut big = KnownTotal::default();
        big.add(Some(Bytes(u64::MAX))).unwrap();
        assert!(big.add(Some(Bytes(1))).is_err());
    }
}
