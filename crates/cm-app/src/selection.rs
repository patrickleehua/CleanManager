//! Plan-time checks of a candidate selection shared by the CLI (`clean`, `purge plan create`, `installer plan
//! create`) and the desktop host (catalog plans), so both entries refuse and list the same rows (CLI-11, WFA-50,
//! WFA-58, NEW-09).
//!
//! - [`refuse_unplannable`] runs first: a candidate that can never be planned now (attached disk image, in use,
//!   protected, mode not allowed) is refused for that reason, never reported as changed because its live state keeps
//!   moving (WFA-58: a mounted image).
//! - [`split_stale`] then re-checks each remaining candidate live, read-only and by metadata only: gone (`missing`),
//!   a file whose size or whole-second modification time changed, or — for a folder — any file the scan observed
//!   under it that is gone or changed (`changed`). Those rows are listed, not planned; the plan would otherwise freeze
//!   them with their scanned sizes. Links and mount points inside were never traversed and are not followed here.
//!   Everything is verified again per item by the executor before any action.

use std::path::PathBuf;

use cm_core::analysis::{Candidate, CandidateSet, IndexView};
use cm_core::model::ObjectKind;
use cm_core::path::RawPath;
use cm_core::plan::PlanError;
use cm_core::rules::Mode;

/// A selected object whose live state no longer matches the scan observation (CLI-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub object_id: String,
    /// `missing` or `changed`.
    pub reason: &'static str,
    pub detail: String,
}

/// The local path of a recorded path on this platform.
fn live_path(p: &RawPath) -> Option<PathBuf> {
    #[cfg(windows)]
    let os = cm_platform::windows::purge::os(p);
    #[cfg(target_os = "macos")]
    let os = cm_platform::macos::purge::os(p);
    os
}

/// How a file's live metadata differs from its scan observation (size, then whole-second modification time, as the
/// executor compares them), if it does.
fn file_drift(m: &std::fs::Metadata, logical: Option<u64>, modified_unix: Option<i64>) -> Option<String> {
    if !m.is_file() {
        return Some("is no longer a regular file".into());
    }
    if let Some(b) = logical
        && m.len() != b
    {
        return Some(format!("is {} bytes now, {b} when scanned", m.len()));
    }
    let now = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    match (modified_unix, now) {
        (Some(then), Some(now)) if then != now => Some("was modified after the scan".into()),
        _ => None,
    }
}

/// Live, read-only check of one candidate against its scan observation (`view`: the scan it comes from); `None` when
/// it still matches (or cannot be read, which the executor decides).
pub fn stale_reason(c: &Candidate, view: &dyn IndexView) -> Option<(&'static str, String)> {
    let top = match std::fs::symlink_metadata(live_path(&c.path)?) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(("missing", format!("{} no longer exists", c.path.display()))),
        Err(_) => return None,
        Ok(m) => m,
    };
    let f = view.info(c.entry_id);
    if c.kind != ObjectKind::Directory {
        let logical = if c.logical.unknown_count == 0 { Some(c.logical.known.0) } else { f.logical };
        return file_drift(&top, logical, f.modified_unix).map(|why| ("changed", format!("{} {why}", c.path.display())));
    }
    if !top.is_dir() || top.file_type().is_symlink() {
        return Some(("changed", format!("{} is no longer a folder", c.path.display())));
    }
    let mut stack = vec![c.entry_id];
    while let Some(e) = stack.pop() {
        let ef = view.info(e);
        if ef.boundary {
            continue;
        }
        match ef.kind {
            ObjectKind::Directory => stack.extend_from_slice(view.children(e)),
            ObjectKind::File => {
                let path = view.path(e);
                let Some(os) = live_path(&path) else { continue };
                let why = match std::fs::symlink_metadata(&os) {
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some("no longer exists".to_string()),
                    Err(_) => None,
                    Ok(m) => file_drift(&m, ef.logical, ef.modified_unix),
                };
                if let Some(why) = why {
                    return Some(("changed", format!("{} changed since the scan: {} {why}", c.path.display(), path.display())));
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a selection into plannable ids and stale rows, keeping order. `view`: the scan the candidates come from.
pub fn split_stale(candidates: &[Candidate], view: &dyn IndexView, selected: Vec<String>) -> (Vec<String>, Vec<Stale>) {
    let mut keep = Vec::new();
    let mut stale = Vec::new();
    for id in selected {
        match candidates.iter().find(|c| c.candidate_id == id).and_then(|c| stale_reason(c, view)) {
            Some((reason, detail)) => stale.push(Stale { object_id: id, reason, detail }),
            None => keep.push(id),
        }
    }
    (keep, stale)
}

/// `id: reason, id: reason` for a refusal message.
pub fn stale_summary(stale: &[Stale]) -> String {
    stale.iter().map(|s| format!("{}: {}", s.object_id, s.reason)).collect::<Vec<_>>().join(", ")
}

/// Why `selected` cannot be planned in `mode` before anything is read live (WFA-58), as the core would refuse it at
/// plan creation; for a candidate that is not plannable now, the message adds its analysis reasons (an attached
/// image names how to eject it). `Ok` when every id is in the set and plannable.
pub fn refuse_unplannable(set: &CandidateSet, selected: &[String], mode: Mode) -> Result<(), String> {
    match cm_core::plan::check_selection(set, selected, mode) {
        Ok(_) => Ok(()),
        Err(PlanError::NotPlannable(id, why)) => {
            let reasons = set.candidates.iter().find(|c| c.candidate_id == id).map(|c| c.reasons.join("; ")).unwrap_or_default();
            Err(if reasons.is_empty() { PlanError::NotPlannable(id, why).to_string() } else { format!("{} ({reasons})", PlanError::NotPlannable(id, why)) })
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, state: &str, reasons: &[&str], path: RawPath, kind: &str, logical: u64) -> Candidate {
        serde_json::from_value(serde_json::json!({
            "candidate_id": id, "entry_id": 2, "path": path, "kind": kind, "rules": [{"rule_id": "installer.iso", "version": "1.0.0"}],
            "category": "installers", "state": state, "tier": "review_only", "reasons": reasons, "protection": null,
            "logical": {"known": logical.to_string(), "unknown_count": 0}, "allowed_modes": ["trash"], "probes": [], "impact": "", "owner_id": null,
        }))
        .unwrap()
    }

    fn raw(p: &std::path::Path) -> RawPath {
        cm_platform::paths::raw_path(p.as_os_str()).unwrap()
    }

    fn set(candidates: Vec<Candidate>) -> CandidateSet {
        CandidateSet { candidate_set_id: "cs".into(), scan_id: "s".into(), revision: 1, rule_set_digest: "d".into(), protection_version: "v".into(), candidates, inapplicable: vec![] }
    }

    /// WFA-58: an attached image is refused for what it is (with how to eject it) before its changing live state could
    /// be reported as a change since discovery; a missing object is listed missing, not refused.
    #[test]
    fn an_unplannable_candidate_is_refused_before_any_live_check() {
        let dir = std::env::temp_dir().join(format!("cm-app-selection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("disk.vhdx");
        std::fs::write(&image, vec![0u8; 64]).unwrap();
        // Discovered at 10 bytes: live it is 64, so a live check alone would call it changed.
        let mounted = candidate("c_img", "pending_evidence", &["mounted_image: eject it first (File Explorer > Eject)"], raw(&image), "file", 10);
        let gone = candidate("c_gone", "allowed", &[], raw(&dir.join("gone.msi")), "file", 5);
        let s = set(vec![mounted, gone]);
        let err = refuse_unplannable(&s, &["c_img".into()], Mode::Trash).unwrap_err();
        assert!(err.starts_with("candidate c_img cannot be planned: pending_evidence: ") && err.contains("mounted_image: eject it first"), "{err}");
        assert!(refuse_unplannable(&s, &["c_gone".into()], Mode::Trash).is_ok());
        assert_eq!(refuse_unplannable(&s, &["c_gone".into()], Mode::Delete).unwrap_err(), "mode delete is not allowed for candidate c_gone");
        let view = cm_core::analysis::MemView::default();
        let (keep, stale) = split_stale(&s.candidates, &view, vec!["c_gone".into()]);
        assert!(keep.is_empty());
        assert_eq!((stale[0].reason, stale_summary(&stale)), ("missing", "c_gone: missing".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
