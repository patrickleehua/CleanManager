//! Manual trash services shared by `cleanmanager trash` and the desktop host (MTR-01..05), so a scanned object is
//! inspected, refused and planned the same way from both entries, and the bin is listed the same way.
//!
//! - [`inspect_entry`]: the core inspection (size, protected descendants, coverage gaps, links), the platform's own
//!   refusals (macOS: folders) and, for a link that resolves into a hard-deny location, the protection refusal naming
//!   that location (WFA-47/63). The target is resolved read-only, by identity, the way `protect check` does; nothing is
//!   opened through the link and links are never moved or followed.
//! - [`bin_refusal`]: an object on a volume without a usable bin is not movable (WFA-35); nothing is ever deleted
//!   instead and the mode is never switched.
//! - [`list_bins`] / [`pick`] / [`purge_plan`]: the current user's bin items, and a separate `manual_selection` plan of
//!   permanent deletes of some of them (MTR-03). The plan only freezes; its review needs the extra irreversibility
//!   confirmation, which only a terminal asks for.

use std::path::PathBuf;

use cm_core::analysis::{IndexView, MemView};
use cm_core::error::ErrorCode;
use cm_core::manual_trash::{BinItem, Inspection, ManualError, ManualInput, SelectionGuard, TrashRefusal, inspect};
use cm_core::model::ObjectKind;
use cm_core::path::RawPath;
use cm_core::plan::v2::PlanV2;
use cm_core::protect::HardDenySet;

/// What the platform calls its bin, in messages.
pub const BIN: &str = cm_core::manual_trash::BIN_NAME;

/// A link whose target is (inside) a hard-deny location (WFA-47/63).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedLink {
    /// The resolved target, for people.
    pub target: String,
    /// Why the target is protected (`is the hard-deny location …`, `is inside …`, `is refused by the path check …`).
    pub why: String,
}

impl ProtectedLink {
    /// The protection refusal (`policy_denied` / `protected`) naming the target.
    pub fn refusal(&self) -> TrashRefusal {
        TrashRefusal {
            code: "policy_denied",
            reason: "protected",
            message: format!(
                "this link resolves into a protected system location: {} {}; links are never moved or followed. {}",
                self.target,
                self.why,
                cm_core::manual_trash::VIEW_ONLY_EXPLANATION
            ),
        }
    }
}

/// Where a link on `path` leads when that is a hard-deny location (resolved read-only, by identity, the way `protect
/// check` does); `None` for anything else.
pub fn protected_link(path: &RawPath, hard: &HardDenySet) -> Option<ProtectedLink> {
    let chain = cm_platform::protection::identity_chain(path);
    chain.links.iter().filter(|l| l.on_object).filter_map(|l| l.target.as_ref().ok()).find_map(|t| {
        let why = hard
            .root_at(&t.object)
            .map(|r| format!("is the hard-deny location {} ({})", r.display, r.class.as_str()))
            .or_else(|| t.ancestors.iter().find_map(|a| hard.covering(a)).map(|r| format!("is inside the hard-deny location {} ({})", r.display, r.class.as_str())))
            .or_else(|| cm_core::protect::prefilter(&t.path).err().map(|e| format!("is refused by the path check ({e})")))?;
        Some(ProtectedLink { target: t.path.display(), why })
    })
}

/// WFA-47/63: a link is never moved or followed, and when it resolves into a hard-deny location the refusal is the
/// protection refusal (`policy_denied` / `protected`) naming that target.
pub fn link_protection(path: &RawPath, hard: &HardDenySet) -> Option<TrashRefusal> {
    protected_link(path, hard).map(|l| l.refusal())
}

/// Objects this platform's trash action cannot take beyond the core refusals: on macOS a folder (the Trash action moves
/// single verified files); none on Windows.
#[cfg(target_os = "macos")]
pub fn platform_refusal(ins: &Inspection) -> Option<TrashRefusal> {
    cm_platform::macos::actions::trash::folder_refusal(ins)
}

/// Objects this platform's trash action cannot take beyond the core refusals: none on Windows (folders move whole).
#[cfg(windows)]
pub fn platform_refusal(_: &Inspection) -> Option<TrashRefusal> {
    None
}

/// Whether objects of this kind may meet [`platform_refusal`] (so planning inspects them first).
pub fn platform_may_refuse(kind: ObjectKind) -> bool {
    cfg!(target_os = "macos") && kind == ObjectKind::Directory
}

/// Read-only inspection of one scanned entry for a move to the bin: the core refusals, then the platform's, and a link
/// into a protected location refused as protected (naming it, also returned) instead of as a plain link.
pub fn inspect_entry(view: &dyn IndexView, entry: u64, guard: &SelectionGuard<'_>, hard: &HardDenySet) -> (Inspection, Option<ProtectedLink>) {
    let mut ins = inspect(view, entry, guard);
    if ins.refusal.is_none() {
        ins.refusal = platform_refusal(&ins);
    }
    let mut link = None;
    if ins.refusal.as_ref().is_some_and(|r| r.reason == "link_or_reparse")
        && let Some(l) = protected_link(&ins.path, hard)
    {
        ins.refusal = Some(l.refusal());
        link = Some(l);
    }
    (ins, link)
}

/// The paths whose holders (processes with them open) an inspection reports: the object itself, or a folder's files
/// (at most 1000, links and mount points not entered). `None` for a cloud placeholder: it is never opened or downloaded.
pub fn holder_paths(ins: &Inspection, view: &dyn IndexView) -> Option<Vec<RawPath>> {
    if ins.cloud == cm_core::model::CloudState::Placeholder || ins.refusal.as_ref().is_some_and(|r| r.reason == "cloud_placeholder") {
        return None;
    }
    if ins.kind != ObjectKind::Directory {
        return Some(vec![ins.path.clone()]);
    }
    let mut paths = Vec::new();
    let mut stack = vec![ins.entry_id];
    while let Some(e) = stack.pop() {
        let f = view.info(e);
        if f.boundary {
            continue;
        }
        match f.kind {
            ObjectKind::Directory => stack.extend_from_slice(view.children(e)),
            ObjectKind::File if paths.len() < 1000 => paths.push(view.path(e)),
            _ => {}
        }
    }
    Some(paths)
}

/// WFA-35: `path` sits on a volume without a usable bin, so it cannot be moved there: an `unsupported` refusal with
/// `reason` (the entry's vocabulary: `trash_unavailable` in the CLI, `recycle_bin_unavailable` in the desktop), the
/// volume, why, and the next step. `None` when the bin is usable.
pub fn bin_refusal(path: &RawPath, reason: &'static str) -> Option<TrashRefusal> {
    use cm_platform::trash_readiness::{message, next_step, unready};
    let u = unready(std::slice::from_ref(path)).into_iter().next()?;
    let one = std::slice::from_ref(&u);
    Some(TrashRefusal { code: "unsupported", reason, message: format!("{} {}", message(one), next_step(one, None)) })
}

/// Before planning: the platform refusals of the selected entries (macOS folders), so nothing is planned that the
/// platform's trash action cannot take.
pub fn platform_plan_check(view: &dyn IndexView, selected: &[u64], guard: &SelectionGuard<'_>) -> Result<(), ManualError> {
    for &e in selected.iter().filter(|&&e| platform_may_refuse(view.info(e).kind)) {
        let ins = inspect(view, e, guard);
        if ins.refusal.is_none()
            && let Some(r) = platform_refusal(&ins)
        {
            return Err(ManualError::Refused { entry_id: e.to_string(), path: ins.path.display(), refusal: Box::new(r), protected: vec![], gaps: vec![] });
        }
    }
    Ok(())
}

/// A plan refusal of a link that resolves into a protected location becomes the protection refusal naming it
/// (WFA-47/63); every other error is returned unchanged.
pub fn name_protected_link(view: &MemView, err: ManualError, hard: &HardDenySet) -> ManualError {
    match err {
        ManualError::Refused { entry_id, path, refusal, protected, gaps } if refusal.reason == "link_or_reparse" => {
            let link = entry_id.parse::<u64>().ok().filter(|e| view.facts.contains_key(e)).map(|e| view.path(e));
            match link.and_then(|l| link_protection(&l, hard)) {
                Some(r) => ManualError::Refused { entry_id, path, refusal: Box::new(r), protected: vec![], gaps: vec![] },
                None => ManualError::Refused { entry_id, path, refusal, protected, gaps },
            }
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------------------------
// The bin (MTR-03)

/// One bin folder of this user (Windows: `X:\$Recycle.Bin\<SID>`; macOS: `~/.Trash` or `/Volumes/*/.Trashes/<uid>`) as
/// an index-like view plus its top-level items.
#[derive(Debug)]
pub struct Bin {
    pub dir: PathBuf,
    pub view: MemView,
    pub items: Vec<BinItem>,
    pub observed_at: String,
}

impl Bin {
    /// Where the recycled object sits, for people.
    pub fn location(&self, item: &BinItem) -> String {
        self.view.path(item.data_entry).display()
    }
}

/// A bin folder that could not be listed next to the others (`bin_unreadable`, or `permission_required` on macOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinIssue {
    pub code: &'static str,
    pub scope_display: String,
    pub message: String,
}

/// The bin could not be listed at all (macOS: the home Trash blocked by privacy protection or permissions); never an
/// empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinListError {
    pub code: ErrorCode,
    pub message: String,
    pub required_action: Option<&'static str>,
}

/// Every bin of this user and the folders that could not be read.
#[cfg(windows)]
pub fn list_bins() -> Result<(Vec<Bin>, Vec<BinIssue>), BinListError> {
    let (listings, failed) = cm_platform::windows::actions::recycle_bin::list_user_bins();
    let issues = failed.into_iter().map(|(d, e)| BinIssue { code: "bin_unreadable", scope_display: d.display().to_string(), message: e }).collect();
    Ok((listings.into_iter().map(|l| Bin { dir: l.bin_dir, view: l.view, items: l.items, observed_at: l.observed_at }).collect(), issues))
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};

    use cm_core::error::ErrorCode;
    use cm_platform::macos::actions::trash::{ListError, TrashListing, list_trash_dir, volume_trash_dirs};

    use super::{Bin, BinIssue, BinListError};

    /// Next step when privacy protection (TCC) or permissions block the Trash listing.
    const GRANT_ACCESS: &str = "Give the program that runs cleanmanager (for example Terminal) Full Disk Access in System Settings › Privacy & Security › Full Disk Access, then retry; or open the Trash in Finder";
    /// Next step while macOS is still asking whether the program may open the Trash.
    const ANSWER_PROMPT: &str = "Answer the macOS privacy prompt (\"would like to access…\") for the program that runs cleanmanager, then list the Trash again; or open the Trash in Finder";
    /// Next step when the folder's own permissions block it.
    const FIX_PERMISSIONS: &str = "The Trash folder should belong to you and be readable by you; check its permissions, or open the Trash in Finder";

    /// The home Trash: `$CM_RESOLVE_HOME/.Trash` when that test redirect is set (then no volume is listed), otherwise
    /// `$HOME/.Trash`.
    fn home_trash() -> (Option<PathBuf>, bool) {
        if let Some(h) = std::env::var_os("CM_RESOLVE_HOME").filter(|v| !v.is_empty()) {
            return (Some(PathBuf::from(h).join(".Trash")), true);
        }
        (std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".Trash")), false)
    }

    fn issue(dir: &Path, e: &ListError) -> BinIssue {
        let code = match e {
            ListError::PermissionRequired { .. } | ListError::PromptPending(_) => "permission_required",
            ListError::Unreadable(_) => "bin_unreadable",
        };
        let message = match e {
            ListError::PermissionRequired { .. } if e.privacy_blocked() => format!("{} (blocked by privacy protection or another policy layer; permission_cause=undetermined). {GRANT_ACCESS}", e.message()),
            ListError::PermissionRequired { .. } => format!("{} (its permissions do not allow listing it). {FIX_PERMISSIONS}", e.message()),
            ListError::PromptPending(_) => format!("{} {ANSWER_PROMPT}", e.message()),
            ListError::Unreadable(_) => e.message().to_owned(),
        };
        BinIssue { code, scope_display: dir.display().to_string(), message }
    }

    fn bin(l: TrashListing) -> Bin {
        Bin { dir: l.trash_dir, view: l.view, items: l.items, observed_at: l.observed_at }
    }

    /// Every Trash of this user. The home Trash failing to list is an error (`permission_required` when blocked), never
    /// an empty list; a volume's Trash failing is an issue next to the others.
    pub fn list_bins() -> Result<(Vec<Bin>, Vec<BinIssue>), BinListError> {
        let (home, redirected) = home_trash();
        let mut bins = Vec::new();
        let mut issues = Vec::new();
        if let Some(dir) = home {
            match list_trash_dir(&dir) {
                Ok(l) => bins.push(bin(l)),
                // No Trash folder yet: truly nothing in it.
                Err(ListError::Unreadable(_)) if std::fs::symlink_metadata(&dir).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
                Err(e) => {
                    let (code, required_action) = match &e {
                        ListError::PermissionRequired { .. } if e.privacy_blocked() => (ErrorCode::PermissionRequired, Some(GRANT_ACCESS)),
                        ListError::PermissionRequired { .. } => (ErrorCode::PermissionRequired, Some(FIX_PERMISSIONS)),
                        ListError::PromptPending(_) => (ErrorCode::PermissionRequired, Some(ANSWER_PROMPT)),
                        ListError::Unreadable(_) => (ErrorCode::IoError, None),
                    };
                    return Err(BinListError { code, message: issue(&dir, &e).message, required_action });
                }
            }
        }
        if !redirected {
            let (dirs, failed) = volume_trash_dirs();
            issues.extend(failed.iter().map(|(d, e)| issue(d, e)));
            for d in dirs {
                match list_trash_dir(&d) {
                    Ok(l) => bins.push(bin(l)),
                    Err(e) => issues.push(issue(&d, &e)),
                }
            }
        }
        Ok((bins, issues))
    }
}

#[cfg(target_os = "macos")]
pub use mac::list_bins;

/// Why a purge selection names no listed item, or items of several bins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickError {
    /// None of the ids is in a bin now (emptied, restored, or never listed).
    NotListed,
    /// This id is not in the same bin as the first one found: one plan per bin (volume).
    OtherBin(String),
}

/// The bin holding the selected items and those items in selection order. One plan per bin (volume): every id must be
/// in the bin of the first one found.
pub fn pick<'a>(bins: &'a [Bin], wanted: &[String]) -> Result<(&'a Bin, Vec<&'a BinItem>), PickError> {
    let bin = bins.iter().find(|b| b.items.iter().any(|i| wanted.contains(&i.trash_item_id))).ok_or(PickError::NotListed)?;
    let mut chosen = Vec::new();
    for id in wanted {
        match bin.items.iter().find(|i| &i.trash_item_id == id) {
            Some(i) => chosen.push(i),
            None => return Err(PickError::OtherBin(id.clone())),
        }
    }
    Ok((bin, chosen))
}

/// Freeze recycled items of one bin into a separate `manual_selection` plan of permanent deletes (MTR-03); it
/// authorizes nothing.
pub fn purge_plan(bin: &Bin, items: &[&BinItem], plan_id: &str, policy_digest: &str, created_unix: i64) -> Result<PlanV2, ManualError> {
    let input = ManualInput { plan_id, scope_id: None, scan_id: None, scan_revision: None, created_unix, observed_at: &bin.observed_at, policy_digest };
    cm_core::manual_trash::build_purge_plan(&bin.view, items, &input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> BinItem {
        BinItem { trash_item_id: id.into(), original_path: None, deleted_at: None, kind: ObjectKind::File, logical_bytes: Some(1), data_entry: 1, info_entry: None, refusal: None }
    }

    fn bin(dir: &str, ids: &[&str]) -> Bin {
        Bin { dir: dir.into(), view: MemView::default(), items: ids.iter().map(|i| item(i)).collect(), observed_at: "2026-10-23T00:00:00Z".into() }
    }

    #[test]
    fn a_purge_selection_comes_from_one_bin() {
        let bins = [bin("C", &["tb_a", "tb_b"]), bin("D", &["tb_c"])];
        let (b, items) = pick(&bins, &["tb_b".into(), "tb_a".into()]).unwrap();
        assert_eq!((b.dir.to_str(), items.iter().map(|i| i.trash_item_id.as_str()).collect::<Vec<_>>()), (Some("C"), vec!["tb_b", "tb_a"]));
        assert_eq!(pick(&bins, &["tb_a".into(), "tb_c".into()]).unwrap_err(), PickError::OtherBin("tb_c".into()));
        assert_eq!(pick(&bins, &["tb_gone".into()]).unwrap_err(), PickError::NotListed);
    }

    #[test]
    fn only_macos_refuses_folders_before_planning() {
        assert!(!platform_may_refuse(ObjectKind::File));
        assert_eq!(platform_may_refuse(ObjectKind::Directory), cfg!(target_os = "macos"));
    }
}
