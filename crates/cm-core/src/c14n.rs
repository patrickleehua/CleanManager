//! CM-C14N-v1: canonical JSON for plan bodies and observation fingerprints (PLN-02, D6).
//!
//! Rules: objects with ASCII keys sorted by byte value, no insignificant whitespace, UTF-8,
//! arrays kept in order, integers only (no floats/exponents), duplicate keys rejected at parse
//! time. String escaping matches Python `json.dumps(..., ensure_ascii=False)` so the
//! generator, validator and Rust produce identical bytes.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum C14nError {
    #[error("malformed JSON: {0}")]
    Malformed(String),
    #[error("duplicate object key: {0}")]
    DuplicateKey(String),
    #[error("non-integer number: {0}")]
    Float(String),
    #[error("object key is not ASCII: {0}")]
    NonAsciiKey(String),
}

/// A JSON value that rejects duplicate keys and non-integer numbers while parsing.
struct Strict(Value);

impl<'de> de::Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Strict;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
        Ok(Strict(Value::Bool(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
        Ok(Strict(Value::Number(v.into())))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
        Ok(Strict(Value::Number(v.into())))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
        Err(E::custom(format!("float:{v}")))
    }
    fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
        Ok(Strict(Value::String(v.to_owned())))
    }
    fn visit_string<E>(self, v: String) -> Result<Strict, E> {
        Ok(Strict(Value::String(v)))
    }
    fn visit_unit<E>(self) -> Result<Strict, E> {
        Ok(Strict(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Strict(Value::Array(out)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let Strict(v) = map.next_value()?;
            if out.insert(key.clone(), v).is_some() {
                return Err(de::Error::custom(format!("duplicate:{key}")));
            }
        }
        Ok(Strict(Value::Object(out)))
    }
}

/// Parse JSON text, rejecting duplicate keys and floats (they would make digests ambiguous).
pub fn strict_parse(text: &str) -> Result<Value, C14nError> {
    let mut de = serde_json::Deserializer::from_str(text);
    let parsed = de::Deserialize::deserialize(&mut de).and_then(|Strict(v)| de.end().map(|_| v));
    parsed.map_err(|e| {
        let msg = e.to_string();
        if let Some(rest) = msg.strip_prefix("duplicate:") {
            C14nError::DuplicateKey(rest.split(" at line").next().unwrap_or(rest).to_owned())
        } else if let Some(rest) = msg.strip_prefix("float:") {
            C14nError::Float(rest.split(" at line").next().unwrap_or(rest).to_owned())
        } else {
            C14nError::Malformed(msg)
        }
    })
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_number(out: &mut String, n: &Number) -> Result<(), C14nError> {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else {
        return Err(C14nError::Float(n.to_string()));
    }
    Ok(())
}

fn write_value(out: &mut String, v: &Value) -> Result<(), C14nError> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, n)?,
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (k, val) in map {
                if !k.is_ascii() {
                    return Err(C14nError::NonAsciiKey(k.clone()));
                }
                sorted.insert(k.as_str(), val);
            }
            out.push('{');
            for (i, (k, val)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_value(out, val)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Canonical UTF-8 bytes of a value.
pub fn canonical_bytes(v: &Value) -> Result<Vec<u8>, C14nError> {
    let mut out = String::new();
    write_value(&mut out, v)?;
    Ok(out.into_bytes())
}

/// Lower-case hex SHA-256 of the canonical bytes.
pub fn digest(v: &Value) -> Result<String, C14nError> {
    let bytes = canonical_bytes(v)?;
    Ok(Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_keys_and_removes_whitespace() {
        let v = strict_parse(r#"{ "b": [1, {"z": null, "a": true}], "a": "x" }"#).unwrap();
        assert_eq!(canonical_bytes(&v).unwrap(), br#"{"a":"x","b":[1,{"a":true,"z":null}]}"#);
    }

    #[test]
    fn escapes_like_python_json_dumps() {
        let v = Value::String("q\"\\/\n\t\u{1}\u{7f}中\u{2028}😀".into());
        assert_eq!(String::from_utf8(canonical_bytes(&v).unwrap()).unwrap(), "\"q\\\"\\\\/\\n\\t\\u0001\u{7f}中\u{2028}😀\"");
    }

    #[test]
    fn rejects_duplicates_floats_exponents_and_non_ascii_keys() {
        assert!(matches!(strict_parse(r#"{"a":1,"a":2}"#), Err(C14nError::DuplicateKey(_))));
        assert!(matches!(strict_parse(r#"{"a":1.5}"#), Err(C14nError::Float(_))));
        assert!(matches!(strict_parse(r#"{"a":1e3}"#), Err(C14nError::Float(_))));
        assert!(matches!(strict_parse(r#"{"a":1} x"#), Err(C14nError::Malformed(_))));
        let v = strict_parse(r#"{"名":1}"#).unwrap();
        assert!(matches!(canonical_bytes(&v), Err(C14nError::NonAsciiKey(_))));
    }

    #[test]
    fn same_body_same_digest_any_field_change_differs() {
        let a = strict_parse(r#"{"mode":"trash","items":[{"action_id":"a1"}]}"#).unwrap();
        let b = strict_parse(r#"{"items":[{"action_id":"a1"}],"mode":"trash"}"#).unwrap();
        let c = strict_parse(r#"{"items":[{"action_id":"a1"}],"mode":"delete"}"#).unwrap();
        assert_eq!(digest(&a).unwrap(), digest(&b).unwrap());
        assert_ne!(digest(&a).unwrap(), digest(&c).unwrap());
    }
}
