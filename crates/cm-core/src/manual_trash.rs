//! Manual selection to the platform trash (parity MTR-01..05, task group 6).
//!
//! - A `manual_selection` plan is built only from entries of a published scan (entry ids, never raw paths). Its
//!   only actions are `fs_trash` of objects at their original place, or — as a separate plan — `fs_delete` of
//!   objects already in the platform recycle bin. It is never policy-matchable: an interactive review of the exact
//!   plan is the only authorization (MTR-01).
//! - Hard-deny locations, default protection classes and the user protection list make an object view-only, and so
//!   do they for its ancestors and, for a directory, any known descendant (reported one by one). A directory whose
//!   subtree was not read completely (unread ranges, links or mount points not traversed, unknown sizes) is refused
//!   as a whole: the user can pick its known descendants instead (MTR-02).
//! - A directory goes to the trash as one object: the plan binds every scanned member (identity, kind and CM-OBS-v1
//!   fingerprint) and the platform moves it only if the live subtree is exactly that, then re-checks it in the bin.
//! - Cloud placeholders are refused: removing one also removes the cloud copy, and nothing here reads or hydrates it.

use serde::Serialize;

use crate::analysis::{IndexView, class_of_name};
use crate::bytes::Bytes;
use crate::model::{CloudState, IdentityQuality, ObjectKind};
use crate::path::RawPath;
use crate::plan::v2::{ActionMeta, FsAction, HeaderInput, MemberMeta, PlanAction, PlanKind, PlanTarget, PlanV2, SubtreeBinding, TargetLocation, members_digest};
use crate::plan::{ItemKind, WireIdentity, observe};
use crate::protect::{HardDenySet, ObjectIdentity, UserProtection, prefilter};

/// How to put back what this tool moved to the Recycle Bin (it never restores from the bin itself).
pub const PUT_BACK_GUIDANCE: &str = "To put it back: open the Recycle Bin, select the item and choose Restore (or drag it out).";
/// Guidance when a move could not be confirmed.
pub const UNKNOWN_GUIDANCE: &str =
    "Check the Recycle Bin and the original folder: if the item is in the Recycle Bin, restore it there if it was not the one you meant; nothing was permanently deleted by this action.";
/// Why system and protected locations offer no trash action (MTR-02).
pub const VIEW_ONLY_EXPLANATION: &str =
    "System folders, protected categories and your protection list are view-only: CleanManager can show, copy the path of, or diagnose them, never move them.";

/// Cap on per-object detail lists (counts are always complete).
const DETAIL_CAP: usize = 50;

/// Protection layers available when the selection is checked. `user: Err` (unreadable list) refuses everything.
#[derive(Debug, Clone, Copy)]
pub struct SelectionGuard<'a> {
    pub hard: Option<&'a HardDenySet>,
    pub user: Result<&'a UserProtection, &'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub path: RawPath,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrashRefusal {
    /// Protocol error code.
    pub code: &'static str,
    /// Machine reason: `protected`, `protected_descendants`, `incomplete_subtree`, `cloud_placeholder`,
    /// `link_or_reparse`, `scan_root`, `hard_links`, `weak_identity`, `in_recycle_bin`, `unknown_size`,
    /// `protection_unavailable`.
    pub reason: &'static str,
    pub message: String,
}

/// Read-only facts about one entry and whether it may be moved to the trash (MTR-02/05). Built from the index only:
/// nothing is opened, read or hydrated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inspection {
    pub entry_id: u64,
    pub path: RawPath,
    pub kind: ObjectKind,
    pub cloud: CloudState,
    /// Known logical bytes of the object or subtree (`None` if any size is unknown).
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub files: u64,
    pub directories: u64,
    pub protected: Vec<Hit>,
    pub protected_count: u64,
    pub gaps: Vec<Hit>,
    pub gap_count: u64,
    pub refusal: Option<TrashRefusal>,
}

fn refusal(code: &'static str, reason: &'static str, message: impl Into<String>) -> TrashRefusal {
    TrashRefusal { code, reason, message: message.into() }
}

fn id_of(view: &dyn IndexView, e: u64) -> ObjectIdentity {
    let f = view.info(e);
    ObjectIdentity::new(f.volume_id.clone(), f.file_id.clone())
}

pub fn in_recycle_bin(path: &RawPath) -> bool {
    path.lossy().to_lowercase().replace('/', "\\").split('\\').any(|c| c == "$recycle.bin" || c == ".trash" || c == ".trashes")
}

/// Why the object itself (or an ancestor) is protected, if it is.
fn own_protection(view: &dyn IndexView, entry: u64, guard: &SelectionGuard<'_>) -> Option<String> {
    let path = view.path(entry);
    if let Err(r) = prefilter(&path) {
        return Some(format!("hard-deny: {r}"));
    }
    if let Some(hard) = guard.hard {
        if let Some(r) = hard.root_at(&id_of(view, entry)) {
            return Some(format!("hard-deny location {} ({})", r.display, r.class.as_str()));
        }
        if let Some(r) = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| hard.covering(&id_of(view, a))) {
            return Some(format!("inside the hard-deny location {} ({})", r.display, r.class.as_str()));
        }
    }
    if let Some(c) = std::iter::successors(Some(entry), |p| view.parent(*p)).find_map(|e| crate::analysis::protection_by_name(view, e)) {
        return Some(format!("default protection class {}", c.as_str()));
    }
    match guard.user {
        Ok(user) => user.match_path(&path, false).map(|e| format!("protected by user entry {}", e.id)),
        Err(_) => None,
    }
}

/// Protection of one descendant (its ancestors are inside the selection and checked separately).
fn descendant_protection(view: &dyn IndexView, e: u64, guard: &SelectionGuard<'_>) -> Option<String> {
    let f = view.info(e);
    let name = view.name(e).lossy().to_lowercase();
    let has_git = f.kind == ObjectKind::Directory && view.children(e).iter().any(|c| view.name(*c).lossy().eq_ignore_ascii_case(".git"));
    if let Some(c) = class_of_name(&name, f.kind == ObjectKind::Directory, has_git) {
        return Some(format!("default protection class {}", c.as_str()));
    }
    if let Some(r) = guard.hard.and_then(|h| h.root_at(&id_of(view, e))) {
        return Some(format!("hard-deny location {} ({})", r.display, r.class.as_str()));
    }
    let path = view.path(e);
    if let Err(r) = prefilter(&path) {
        return Some(format!("hard-deny: {r}"));
    }
    guard.user.ok().and_then(|u| u.match_path(&path, false)).map(|u| format!("protected by user entry {}", u.id))
}

/// Inspect one entry: size, coverage, protection and whether a trash plan may include it.
pub fn inspect(view: &dyn IndexView, entry: u64, guard: &SelectionGuard<'_>) -> Inspection {
    let f = view.info(entry);
    let path = view.path(entry);
    let mut out = Inspection {
        entry_id: entry, path: path.clone(), kind: f.kind, cloud: f.cloud, logical_bytes: Some(0), allocated_bytes: Some(0), files: 0, directories: 0,
        protected: Vec::new(), protected_count: 0, gaps: Vec::new(), gap_count: 0, refusal: None,
    };
    let push = |list: &mut Vec<Hit>, count: &mut u64, path: RawPath, reason: String| {
        *count += 1;
        if list.len() < DETAIL_CAP {
            list.push(Hit { path, reason });
        }
    };
    if let Some(why) = own_protection(view, entry, guard) {
        push(&mut out.protected, &mut out.protected_count, path.clone(), why);
    }
    // Walk the subtree (index only). Links and mount points are boundaries: recorded, never traversed.
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![entry];
    let mut placeholders = 0u64;
    let mut multi_linked = 0u64;
    let mut weak = 0u64;
    while let Some(e) = stack.pop() {
        let ef = view.info(e);
        if ef.quality != IdentityQuality::Strong {
            weak += 1;
        }
        if ef.cloud == CloudState::Placeholder {
            placeholders += 1;
        }
        if e != entry && let Some(why) = descendant_protection(view, e, guard) {
            push(&mut out.protected, &mut out.protected_count, view.path(e), why);
        }
        if ef.skipped_here > 0 {
            push(&mut out.gaps, &mut out.gap_count, view.path(e), format!("{} unread range(s) below (permission or error)", ef.skipped_here));
        }
        if ef.boundary || matches!(ef.kind, ObjectKind::Symlink | ObjectKind::Reparse | ObjectKind::Other) {
            if e != entry {
                push(&mut out.gaps, &mut out.gap_count, view.path(e), "link, mount point or special object not traversed".into());
            }
            continue;
        }
        match ef.kind {
            ObjectKind::Directory => {
                out.directories += u64::from(e != entry);
                stack.extend_from_slice(view.children(e));
            }
            _ => {
                if !seen.insert(ef.object_id) {
                    continue;
                }
                out.files += 1;
                if ef.link_count.is_some_and(|n| n > 1) {
                    multi_linked += 1;
                }
                match ef.logical {
                    Some(b) => out.logical_bytes = out.logical_bytes.map(|t| t.saturating_add(b)),
                    None => {
                        out.logical_bytes = None;
                        push(&mut out.gaps, &mut out.gap_count, view.path(e), "size unknown".into());
                    }
                }
                out.allocated_bytes = out.allocated_bytes.zip(ef.allocated).map(|(t, b)| t.saturating_add(b));
            }
        }
    }
    let is_dir = f.kind == ObjectKind::Directory;
    // Whole-tree check of the user list (e.g. an exact entry on something below a link that was not traversed).
    if out.protected_count == 0 && is_dir && let Some(e) = guard.user.ok().and_then(|u| u.match_path(&path, true)) {
        push(&mut out.protected, &mut out.protected_count, path.clone(), format!("contains an object protected by user entry {}", e.id));
    }
    out.refusal = if let Err(problem) = guard.user {
        Some(refusal("protection_unavailable", "protection_unavailable", format!("protection list unavailable ({problem}); nothing can be moved until it is repaired")))
    } else if entry == view.root() {
        Some(refusal("invalid_argument", "scan_root", "the scan root itself cannot be selected; scan its parent and select it there"))
    } else if in_recycle_bin(&path) {
        Some(refusal("invalid_argument", "in_recycle_bin", "already in the recycle bin; use `trash purge` to delete it permanently"))
    } else if f.boundary || !matches!(f.kind, ObjectKind::File | ObjectKind::Directory) {
        Some(refusal("unsupported", "link_or_reparse", "links, junctions, mount points and special objects are never moved or followed"))
    } else if let Some(first) = out.protected.first() {
        let (reason, msg) = if out.protected.first().is_some_and(|h| h.path == path) {
            ("protected", format!("{}. {VIEW_ONLY_EXPLANATION}", first.reason))
        } else {
            ("protected_descendants", format!("{} protected object(s) inside, first: {} ({}). Select unprotected descendants instead.", out.protected_count, first.path.display(), first.reason))
        };
        Some(refusal("policy_denied", reason, msg))
    } else if placeholders > 0 {
        Some(refusal("unsupported", "cloud_placeholder", "cloud placeholder(s): removing one also removes the cloud copy; use the sync client (nothing was downloaded)"))
    } else if is_dir && out.gap_count > 0 {
        let first = &out.gaps[0];
        Some(refusal("policy_denied", "incomplete_subtree", format!("the folder was not read completely ({} gap(s), first: {} — {}); select its known descendants instead", out.gap_count, first.path.display(), first.reason)))
    } else if out.logical_bytes.is_none() {
        Some(refusal("policy_denied", "unknown_size", "size unknown; rescan before moving it"))
    } else if multi_linked > 0 {
        Some(refusal("unsupported", "hard_links", "file(s) with several hard links: moving one name would not free the data"))
    } else if weak > 0 {
        Some(refusal("unsupported", "weak_identity", "identity could not be established strongly enough to verify the move"))
    } else {
        None
    };
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManualError {
    #[error("select at least one entry")]
    Empty,
    #[error("entry {0} is not in this scan")]
    UnknownEntry(String),
    #[error("entry {inner} is inside the also-selected entry {outer}; select one of them")]
    Overlap { inner: String, outer: String },
    #[error("{path}: {}", .refusal.message)]
    Refused { entry_id: String, path: String, refusal: Box<TrashRefusal>, protected: Vec<Hit>, gaps: Vec<Hit> },
    #[error("{0}")]
    Build(String),
}

impl ManualError {
    /// Protocol error code.
    pub fn code(&self) -> crate::error::ErrorCode {
        use crate::error::ErrorCode;
        match self {
            ManualError::Refused { refusal, .. } => match refusal.code {
                "policy_denied" => ErrorCode::PolicyDenied,
                "unsupported" => ErrorCode::Unsupported,
                "protection_unavailable" => ErrorCode::ProtectionUnavailable,
                "permission_required" => ErrorCode::PermissionRequired,
                _ => ErrorCode::InvalidArgument,
            },
            ManualError::Build(_) => ErrorCode::InternalError,
            _ => ErrorCode::InvalidArgument,
        }
    }
}

/// Header inputs of a manual plan.
pub struct ManualInput<'a> {
    pub plan_id: &'a str,
    pub scope_id: Option<&'a str>,
    pub scan_id: Option<&'a str>,
    pub scan_revision: Option<u32>,
    pub created_unix: i64,
    /// When the bound observations were taken (scan finish time, or the bin listing time).
    pub observed_at: &'a str,
    pub policy_digest: &'a str,
}

fn wire(view: &dyn IndexView, e: u64, at: &str) -> WireIdentity {
    let f = view.info(e);
    WireIdentity {
        volume_id: f.volume_id.clone(),
        file_id: f.file_id.clone(),
        generation: None,
        quality: if f.quality == IdentityQuality::Strong { "strong".into() } else { "weak".into() },
        observed_at: at.into(),
    }
}

fn target(view: &dyn IndexView, e: u64, entry_id: String, logical: u64, allocated: Option<u64>, at: &str) -> Result<(PlanTarget, crate::obs::ObservationRecord), ManualError> {
    let obs = observe(view, e, at);
    let fingerprint = obs.fingerprint().map_err(|err| ManualError::Build(err.to_string()))?;
    let kind = if view.info(e).kind == ObjectKind::Directory { ItemKind::Directory } else { ItemKind::File };
    let t = PlanTarget {
        entry_id, candidate_id: None, path: view.path(e), identity: wire(view, e, at), observed_fingerprint_sha256: fingerprint,
        logical_bytes: Bytes(logical), allocated_bytes: allocated.map(Bytes), kind,
    };
    Ok((t, obs))
}

fn header<'a>(input: &'a ManualInput<'a>) -> HeaderInput<'a> {
    HeaderInput {
        plan_id: input.plan_id, plan_kind: PlanKind::ManualSelection, created_unix: input.created_unix, scope_id: input.scope_id,
        scan_id: input.scan_id, scan_revision: input.scan_revision, rule_set_digest: crate::plan::v2::NO_RULES_DIGEST, policy_digest: input.policy_digest,
    }
}

/// Freeze selected scan entries into a `manual_selection` plan of `fs_trash` actions (one per selected object).
pub fn build_trash_plan(view: &dyn IndexView, entries: &[u64], guard: &SelectionGuard<'_>, input: &ManualInput<'_>) -> Result<PlanV2, ManualError> {
    if entries.is_empty() {
        return Err(ManualError::Empty);
    }
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for (i, &e) in entries.iter().enumerate() {
        for &other in &entries[..i] {
            let inside = |a: u64, b: u64| std::iter::successors(view.parent(a), |p| view.parent(*p)).any(|p| p == b);
            if other == e {
                return Err(ManualError::Overlap { inner: e.to_string(), outer: other.to_string() });
            }
            if inside(e, other) || inside(other, e) {
                let (inner, outer) = if inside(e, other) { (e, other) } else { (other, e) };
                return Err(ManualError::Overlap { inner: inner.to_string(), outer: outer.to_string() });
            }
        }
        let ins = inspect(view, e, guard);
        if let Some(r) = ins.refusal {
            return Err(ManualError::Refused { entry_id: e.to_string(), path: ins.path.display(), refusal: Box::new(r), protected: ins.protected, gaps: ins.gaps });
        }
        let logical = ins.logical_bytes.unwrap_or(0);
        let (t, obs) = target(view, e, e.to_string(), logical, ins.allocated_bytes, input.observed_at)?;
        let mut members = Vec::new();
        if ins.kind == ObjectKind::Directory {
            let mut stack: Vec<u64> = view.children(e).to_vec();
            while let Some(m) = stack.pop() {
                let mf = view.info(m);
                let directory = mf.kind == ObjectKind::Directory;
                if directory {
                    stack.extend_from_slice(view.children(m));
                }
                members.push(MemberMeta { path: view.path(m), identity: wire(view, m, input.observed_at), observation: observe(view, m, input.observed_at), directory });
            }
            members.sort_by_key(|m| m.path.display());
        }
        let subtree = (ins.kind == ObjectKind::Directory).then(|| -> Result<SubtreeBinding, ManualError> {
            Ok(SubtreeBinding { items: members.len() as u64, logical_bytes: Bytes(logical), members_sha256: members_digest(&members).map_err(|err| ManualError::Build(err.to_string()))? })
        }).transpose()?;
        items.push(PlanAction::FsTrash(FsAction {
            action_id: format!("t{i:07}"), target: t, target_location: TargetLocation::Original, must_not_follow_links: true, subtree, trash_entry_id: None,
        }));
        meta.push(ActionMeta { observation: Some(obs), members, ..Default::default() });
    }
    crate::plan::v2::build(&header(input), items, meta).map_err(|err| ManualError::Build(err.to_string()))
}

/// One object in the platform recycle bin, as listed for a purge selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BinItem {
    pub trash_item_id: String,
    /// Where it was deleted from (from the bin's own record; display only).
    pub original_path: Option<String>,
    pub deleted_at: Option<String>,
    pub kind: ObjectKind,
    pub logical_bytes: Option<u64>,
    /// The recycled object (`$R…`) in the bin view.
    #[serde(skip)]
    pub data_entry: u64,
    /// The bin's record of it (`$I…`), deleted after the object.
    #[serde(skip)]
    pub info_entry: Option<u64>,
    /// `None` when it can be purged by this tool; otherwise why not.
    pub refusal: Option<String>,
}

/// Freeze recycled objects into a separate `manual_selection` plan of `fs_delete` actions on objects already in the bin
/// (MTR-03). For each item, contents come first (files, then directories deepest first), then the object, then the bin's
/// record; the executor stops an item's sequence at its first action that does not succeed.
pub fn build_purge_plan(view: &dyn IndexView, items: &[&BinItem], input: &ManualInput<'_>) -> Result<PlanV2, ManualError> {
    if items.is_empty() {
        return Err(ManualError::Empty);
    }
    let mut actions = Vec::new();
    let mut meta = Vec::new();
    for item in items {
        if let Some(why) = &item.refusal {
            let r = refusal("unsupported", "link_or_reparse", why.clone());
            return Err(ManualError::Refused { entry_id: item.trash_item_id.clone(), path: view.path(item.data_entry).display(), refusal: Box::new(r), protected: vec![], gaps: vec![] });
        }
        let mut order: Vec<u64> = Vec::new();
        let mut dirs: Vec<(usize, u64)> = Vec::new();
        let mut stack = vec![(item.data_entry, 0usize)];
        while let Some((e, depth)) = stack.pop() {
            let f = view.info(e);
            if f.boundary || !matches!(f.kind, ObjectKind::File | ObjectKind::Directory) {
                let r = refusal("unsupported", "link_or_reparse", "contains a link or special object; empty it with Explorer instead");
                return Err(ManualError::Refused { entry_id: item.trash_item_id.clone(), path: view.path(e).display(), refusal: Box::new(r), protected: vec![], gaps: vec![] });
            }
            if f.kind == ObjectKind::Directory {
                dirs.push((depth, e));
                stack.extend(view.children(e).iter().map(|c| (*c, depth + 1)));
            } else {
                order.push(e);
            }
        }
        order.sort_unstable();
        dirs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        order.extend(dirs.into_iter().map(|(_, d)| d));
        order.extend(item.info_entry);
        for e in order {
            let f = view.info(e);
            let directory = f.kind == ObjectKind::Directory;
            let logical = if directory { Some(0) } else { f.logical };
            let Some(logical) = logical else {
                let r = refusal("policy_denied", "unknown_size", "a size in the recycled item is unknown; list the recycle bin again");
                return Err(ManualError::Refused { entry_id: item.trash_item_id.clone(), path: view.path(e).display(), refusal: Box::new(r), protected: vec![], gaps: vec![] });
            };
            let n = actions.len();
            let (t, obs) = target(view, e, format!("{}/{e}", item.trash_item_id), logical, f.allocated, input.observed_at)?;
            actions.push(PlanAction::FsDelete(FsAction {
                action_id: format!("d{n:07}"), target: t, target_location: TargetLocation::PlatformTrash, must_not_follow_links: true, subtree: None,
                trash_entry_id: Some(item.trash_item_id.clone()),
            }));
            meta.push(ActionMeta { observation: Some(obs), note: item.original_path.clone(), ..Default::default() });
        }
    }
    crate::plan::v2::build(&header(input), actions, meta).map_err(|err| ManualError::Build(err.to_string()))
}

#[cfg(test)]
mod tests;
