//! CM-OBS-v1: bounded metadata precondition record bound into plans by its fingerprint (D6).
//!
//! It records only metadata (never file content), keeps unavailable fields as explicit nulls,
//! and tags every field with its source. The fingerprint detects staleness; it never replaces
//! live identity and parent checks at execution time, and missing fields cannot be recovered
//! from the hash.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bytes::Bytes;
use crate::c14n::{C14nError, digest};
use crate::model::{CloudState, Identity, ObjectKind, Source};
use crate::path::RawPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    /// Windows FILETIME: 100 ns ticks since 1601-01-01 UTC.
    #[serde(rename = "windows_filetime_100ns")]
    WindowsFiletime100ns,
    /// Seconds and nanoseconds since the Unix epoch, combined as nanoseconds.
    UnixNanoseconds,
}

/// A native timestamp kept in its original unit; never reconstructed from display time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTime {
    pub unit: TimeUnit,
    /// Decimal string so the full native range survives JSON.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRecord {
    pub contract: ObsContract,
    pub scope_root: Identity,
    pub volume_id: String,
    pub entity: Identity,
    /// Parent chain from the immediate parent up to (and including) the scope root.
    pub parent_chain: Vec<Identity>,
    pub path: RawPath,
    pub kind: ObjectKind,
    pub logical_bytes: Option<Bytes>,
    pub allocated_bytes: Option<Bytes>,
    pub modified: Option<NativeTime>,
    pub change_marker: Option<NativeTime>,
    pub link_count: Option<u32>,
    pub reparse_tag: Option<u32>,
    pub cloud: CloudState,
    /// Source per field; `None` when the field was not available.
    pub field_sources: BTreeMap<String, Option<Source>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsContract {
    #[serde(rename = "obs/v1")]
    V1,
}

/// Fields whose provenance must be stated (value or null) in every record.
pub const SOURCED_FIELDS: [&str; 7] =
    ["logical_bytes", "allocated_bytes", "modified", "change_marker", "link_count", "reparse_tag", "cloud"];

impl ObservationRecord {
    /// `observed_fingerprint_sha256` = SHA-256 of the CM-C14N-v1 bytes of this record.
    pub fn fingerprint(&self) -> Result<String, C14nError> {
        let value = serde_json::to_value(self).expect("observation serializes");
        digest(&value)
    }

    /// A present value needs a source and an absent one must not claim a source.
    pub fn check_sources(&self) -> Result<(), String> {
        let present = [
            ("logical_bytes", self.logical_bytes.is_some()),
            ("allocated_bytes", self.allocated_bytes.is_some()),
            ("modified", self.modified.is_some()),
            ("change_marker", self.change_marker.is_some()),
            ("link_count", self.link_count.is_some()),
            ("reparse_tag", self.reparse_tag.is_some()),
            ("cloud", self.cloud != CloudState::Unknown),
        ];
        if self.field_sources.len() != SOURCED_FIELDS.len() {
            return Err("field_sources must list exactly the sourced fields".into());
        }
        for (name, has_value) in present {
            match self.field_sources.get(name) {
                None => return Err(format!("field_sources lacks {name}")),
                Some(src) if src.is_some() != has_value => return Err(format!("source/value mismatch for {name}")),
                Some(_) => {}
            }
        }
        Ok(())
    }
}
