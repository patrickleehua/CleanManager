//! Cross-language golden vectors (MVP task 2.5): the Python generator writes the expected
//! canonical bytes and digests; Rust must reproduce them exactly.

use std::path::PathBuf;

use cm_core::c14n::{C14nError, canonical_bytes, digest, strict_parse};
use cm_core::obs::ObservationRecord;
use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn canonical_bytes_match_python_golden_cases() {
    let golden: Value = serde_json::from_str(&read("examples/c14n/golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 5);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let value = strict_parse(case["input"].as_str().unwrap()).unwrap();
        let canon = String::from_utf8(canonical_bytes(&value).unwrap()).unwrap();
        assert_eq!(canon, case["canonical"].as_str().unwrap(), "{name}");
        assert_eq!(digest(&value).unwrap(), case["sha256"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn golden_rejections_are_rejected_for_the_stated_reason() {
    let golden: Value = serde_json::from_str(&read("examples/c14n/golden.json")).unwrap();
    for case in golden["rejected"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let outcome = strict_parse(input).and_then(|v| canonical_bytes(&v).map(|_| ()));
        let reason = match outcome {
            Err(C14nError::DuplicateKey(_)) => "duplicate_key",
            Err(C14nError::Float(_)) => "float",
            Err(C14nError::Malformed(_)) => "malformed",
            Err(C14nError::NonAsciiKey(_)) => "non_ascii_key",
            Ok(()) => panic!("{} was accepted", case["name"]),
        };
        assert_eq!(reason, case["reason"].as_str().unwrap(), "{}", case["name"]);
    }
}

#[test]
fn observation_fingerprint_matches_and_round_trips_strictly() {
    let doc: Value = serde_json::from_str(&read("examples/c14n/observation-v1.json")).unwrap();
    let record: ObservationRecord = serde_json::from_value(doc["record"].clone()).expect("strict typed parse");
    record.check_sources().unwrap();
    assert_eq!(record.fingerprint().unwrap(), doc["fingerprint_sha256"].as_str().unwrap());
    assert_eq!(serde_json::to_value(&record).unwrap(), doc["record"], "no field is added, dropped or renamed");

    let mut extra = doc["record"].clone();
    extra["content_sha256"] = Value::from("0".repeat(64));
    assert!(serde_json::from_value::<ObservationRecord>(extra).is_err(), "unknown fields are rejected");

    let mut changed = record.clone();
    changed.logical_bytes = Some(cm_core::bytes::Bytes(1));
    assert_ne!(changed.fingerprint().unwrap(), record.fingerprint().unwrap());
}

#[test]
fn every_example_plan_digest_is_reproduced() {
    let mut checked = 0;
    for entry in std::fs::read_dir(repo().join("examples/contracts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let loose: Value = serde_json::from_str(&text).unwrap();
        if loose["kind"] == "plan" {
            // Only canonicalized inputs must pass the strict parser (no floats, no duplicate keys).
            let value = strict_parse(&text).unwrap();
            assert_eq!(digest(&value["body"]).unwrap(), value["plan_digest"].as_str().unwrap(), "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 9, "expected plan/v1, plan/v2 and recovery/v1 examples, found {checked}");
}
