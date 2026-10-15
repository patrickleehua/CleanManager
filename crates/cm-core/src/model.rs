//! Scan-side domain model (SCN-01/02/03/06, IDX-04): scopes, identities, file objects vs
//! directory entries, measured values with provenance, and scan lifecycle/completeness.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::bytes::{Bytes, KnownTotal};
use crate::capability::Platform;
use crate::error::CoreError;
use crate::path::RawPath;

/// UTC RFC 3339 timestamp ending in `Z`, kept as text to avoid lossy time conversions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileIdKind {
    /// NTFS file reference number (64-bit, includes the sequence number).
    NtfsFrn64,
    /// 128-bit FILE_ID_INFO (ReFS and newer APIs); never truncated to 64 bits.
    FileId128,
    /// APFS/HFS+ file ID.
    Inode64,
}

/// A file identity as observed on one volume instance. File IDs can be reused, so identity
/// alone never proves a name still sits under an approved parent. On the wire it uses the
/// protocol `identity` shape with a kind-prefixed `file_id` string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    pub volume_id: String,
    pub file_id_kind: FileIdKind,
    /// Lower-case hexadecimal of the full native ID.
    pub file_id_hex: String,
    /// Generation/sequence when the platform exposes one; never invented.
    pub generation: Option<String>,
    pub quality: IdentityQuality,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityQuality {
    /// Stable volume + full native file ID obtained from the object itself.
    Strong,
    /// Path-derived or otherwise incomplete; usable for analysis, never as write evidence.
    Weak,
}

impl Identity {
    /// Weak identities are never upgraded into execution evidence.
    pub fn execution_grade(&self) -> bool {
        self.quality == IdentityQuality::Strong
    }

    /// Protocol `file_id` text: kind-prefixed so 64- and 128-bit IDs never collide.
    pub fn wire_file_id(&self) -> String {
        let kind = match self.file_id_kind {
            FileIdKind::NtfsFrn64 => "ntfs64",
            FileIdKind::FileId128 => "fileid128",
            FileIdKind::Inode64 => "inode64",
        };
        format!("{kind}:{}", self.file_id_hex)
    }

    fn parse_wire_file_id(s: &str) -> Result<(FileIdKind, String), String> {
        let (kind, hex) = s.split_once(':').ok_or("file_id must be kind:hex")?;
        let (kind, digits) = match kind {
            "ntfs64" => (FileIdKind::NtfsFrn64, 16),
            "fileid128" => (FileIdKind::FileId128, 32),
            "inode64" => (FileIdKind::Inode64, 16),
            other => return Err(format!("unknown file_id kind {other}")),
        };
        if hex.len() != digits || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("file_id needs {digits} lower-case hex digits"));
        }
        Ok((kind, hex.to_owned()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIdentity {
    volume_id: String,
    file_id: String,
    generation: Option<String>,
    quality: IdentityQuality,
    observed_at: Timestamp,
}

impl Serialize for Identity {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        WireIdentity {
            volume_id: self.volume_id.clone(),
            file_id: self.wire_file_id(),
            generation: self.generation.clone(),
            quality: self.quality,
            observed_at: self.observed_at.clone(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = WireIdentity::deserialize(d)?;
        let (file_id_kind, file_id_hex) = Identity::parse_wire_file_id(&w.file_id).map_err(serde::de::Error::custom)?;
        Ok(Identity {
            volume_id: w.volume_id,
            file_id_kind,
            file_id_hex,
            generation: w.generation,
            quality: w.quality,
            observed_at: w.observed_at,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    File,
    Directory,
    Symlink,
    /// Windows reparse point other than a plain symlink (junction, mount point, cloud, ...).
    Reparse,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    DirectoryEnumeration,
    HandleQuery,
    BulkAttributes,
    Stat,
    VolumeQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    NotScanned,
    PermissionDenied,
    Unsupported,
    CloudBoundary,
    SharedBlocks,
    Error,
}

/// A value with provenance. `Unknown` is never coerced to zero (SCN-03/SCN-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Measured<T> {
    Known { value: T, source: Source },
    Unknown { reason: UnknownReason },
}

impl<T: Copy> Measured<T> {
    pub fn known(&self) -> Option<T> {
        match self {
            Measured::Known { value, .. } => Some(*value),
            Measured::Unknown { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudState {
    NotCloud,
    /// Fully present locally.
    Hydrated,
    /// Placeholder whose content lives remotely; reading it may trigger a download.
    Placeholder,
    Unknown,
}

pub type ObjectId = u64;
pub type EntryId = u64;

/// One file-system object (inode / file record). Several directory entries may point at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileObject {
    pub object_id: ObjectId,
    pub identity: Identity,
    pub kind: ObjectKind,
    pub logical: Measured<Bytes>,
    pub allocated: Measured<Bytes>,
    pub link_count: Option<u32>,
    pub cloud: CloudState,
}

/// A name inside a directory. The root entry has no parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub entry_id: EntryId,
    pub parent_entry_id: Option<EntryId>,
    pub name: RawPath,
    pub object_id: ObjectId,
}

/// What the user asked to scan. Links and mount points are boundaries, never followed (SCN-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub scope_id: String,
    pub platform: Platform,
    pub root: RawPath,
    pub root_volume_id: Option<String>,
}

impl Scope {
    pub fn new(scope_id: &str, root: RawPath, root_volume_id: Option<String>) -> Self {
        Scope { scope_id: scope_id.to_owned(), platform: root.platform(), root, root_volume_id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryKind {
    Symlink,
    Junction,
    MountPoint,
    OtherVolume,
    CloudProvider,
    /// CleanManager's own data directory (index, audit log): recorded, never traversed, so a
    /// scan never observes its own writes. Reported as an issue, not hidden.
    OwnStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    PermissionDenied,
    CloudBoundary,
    VolumeChanged,
    Unsupported,
    Cancelled,
    Error,
}

/// A subtree that was not read. Its contents are unknown, not empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedRange {
    pub path: RawPath,
    pub reason: SkipReason,
}

/// A link or mount boundary that was recorded but not traversed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boundary {
    pub path: RawPath,
    pub kind: BoundaryKind,
}

/// Live states (scan_status) are distinct from terminal states (scan_result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Created,
    Running,
    Stopping,
    Completed,
    Partial,
    Cancelled,
    Failed,
    /// The hosting process exited before a terminal state was recorded (CLI-09).
    Interrupted,
}

impl ScanState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, ScanState::Created | ScanState::Running | ScanState::Stopping)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    VerifiedScope,
    PartialScope,
}

/// Terminal facts of one scan. Constructed only through `ScanOutcome::finish`, which refuses
/// to call a scan complete while any range is unread (SCN-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanOutcome {
    pub state: ScanState,
    pub completeness: Completeness,
    pub skipped: Vec<SkippedRange>,
    pub boundaries: Vec<Boundary>,
}

impl ScanOutcome {
    /// `requested` is what the backend reached; the result is downgraded when ranges are missing.
    pub fn finish(requested: ScanState, skipped: Vec<SkippedRange>, boundaries: Vec<Boundary>) -> Result<Self, CoreError> {
        if !requested.is_terminal() {
            return Err(CoreError::InvalidState("scan outcome needs a terminal state"));
        }
        let state = match requested {
            ScanState::Completed if !skipped.is_empty() => ScanState::Partial,
            other => other,
        };
        let completeness = if state == ScanState::Completed { Completeness::VerifiedScope } else { Completeness::PartialScope };
        Ok(ScanOutcome { state, completeness, skipped, boundaries })
    }
}

/// Space for a set of directory entries with hard links counted once (IDX-04 / SCN-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SpaceSummary {
    pub logical: KnownTotal,
    pub allocated: KnownTotal,
    /// Number of distinct objects that more than one selected entry pointed at.
    pub shared_objects: u64,
}

impl SpaceSummary {
    pub fn of<'a>(
        entries: impl IntoIterator<Item = &'a DirectoryEntry>,
        object: impl Fn(ObjectId) -> Option<&'a FileObject>,
    ) -> Result<Self, CoreError> {
        let mut seen = BTreeSet::new();
        let mut summary = SpaceSummary::default();
        for entry in entries {
            if !seen.insert(entry.object_id) {
                summary.shared_objects += 1;
                continue;
            }
            match object(entry.object_id) {
                Some(o) => {
                    summary.logical.add(o.logical.known())?;
                    summary.allocated.add(o.allocated.known())?;
                }
                None => {
                    summary.logical.add(None)?;
                    summary.allocated.add(None)?;
                }
            }
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(q: IdentityQuality) -> Identity {
        Identity {
            volume_id: "vol".into(),
            file_id_kind: FileIdKind::NtfsFrn64,
            file_id_hex: "0001000000000a2b".into(),
            generation: None,
            quality: q,
            observed_at: Timestamp("2026-10-15T00:00:00Z".into()),
        }
    }

    fn object(id: ObjectId, logical: Measured<Bytes>) -> FileObject {
        FileObject {
            object_id: id,
            identity: ident(IdentityQuality::Strong),
            kind: ObjectKind::File,
            logical,
            allocated: Measured::Unknown { reason: UnknownReason::Unsupported },
            link_count: Some(2),
            cloud: CloudState::NotCloud,
        }
    }

    fn entry(entry_id: EntryId, object_id: ObjectId) -> DirectoryEntry {
        DirectoryEntry { entry_id, parent_entry_id: Some(0), name: RawPath::unix(b"n".to_vec()).unwrap(), object_id }
    }

    #[test]
    fn weak_identity_is_never_execution_grade() {
        assert!(ident(IdentityQuality::Strong).execution_grade());
        assert!(!ident(IdentityQuality::Weak).execution_grade());
        assert_eq!(ident(IdentityQuality::Strong).wire_file_id(), "ntfs64:0001000000000a2b");
    }

    #[test]
    fn completed_with_skipped_ranges_becomes_partial() {
        let skipped = vec![SkippedRange { path: RawPath::unix(b"/x/denied".to_vec()).unwrap(), reason: SkipReason::PermissionDenied }];
        let out = ScanOutcome::finish(ScanState::Completed, skipped, vec![]).unwrap();
        assert_eq!((out.state, out.completeness), (ScanState::Partial, Completeness::PartialScope));
        let clean = ScanOutcome::finish(ScanState::Completed, vec![], vec![]).unwrap();
        assert_eq!(clean.completeness, Completeness::VerifiedScope);
        assert!(ScanOutcome::finish(ScanState::Running, vec![], vec![]).is_err());
    }

    #[test]
    fn boundaries_do_not_make_a_scan_partial() {
        let b = vec![Boundary { path: RawPath::unix(b"/x/link".to_vec()).unwrap(), kind: BoundaryKind::Symlink }];
        let out = ScanOutcome::finish(ScanState::Completed, vec![], b).unwrap();
        assert_eq!(out.state, ScanState::Completed, "recorded, untraversed links are part of the verified scope");
    }

    #[test]
    fn hardlinks_count_once_and_unknowns_stay_unknown() {
        let known = object(1, Measured::Known { value: Bytes(100), source: Source::HandleQuery });
        let unknown = object(2, Measured::Unknown { reason: UnknownReason::PermissionDenied });
        let objects = [known, unknown];
        let entries = [entry(10, 1), entry(11, 1), entry(12, 2)];
        let s = SpaceSummary::of(entries.iter(), |id| objects.iter().find(|o| o.object_id == id)).unwrap();
        assert_eq!(s.logical.known, Bytes(100));
        assert_eq!(s.logical.unknown_count, 1);
        assert_eq!(s.logical.exact(), None);
        assert_eq!(s.shared_objects, 1);
        assert_eq!(s.allocated.unknown_count, 2);
    }

    #[test]
    fn measured_serializes_unknown_with_reason_not_zero() {
        let m: Measured<Bytes> = Measured::Unknown { reason: UnknownReason::CloudBoundary };
        assert_eq!(serde_json::to_value(m).unwrap(), serde_json::json!({"state": "unknown", "reason": "cloud_boundary"}));
    }
}
