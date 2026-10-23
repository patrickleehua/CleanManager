//! What operations applied after a scan removed from its snapshot (the "removed since scan" overlay), shared by
//! `cleanmanager entries list` and the desktop's disk, treemap, search and candidate views, so both entries say the
//! same thing about an entry the scan still lists.
//!
//! A scan is a snapshot; nothing here re-reads the disk. The overlay is derived from the audit store only:
//!
//! - every action that **succeeded** at or after the scan finished (per item, `result_at`), of a stored `plan/v1`
//!   (cleanup, build artifacts, installers) or of a `plan/v2` file action (`fs_trash`, `fs_quarantine`, `fs_delete`:
//!   manual selections, old downloads, uninstall leftovers), names the planned path;
//! - a quarantine move whose quarantine row is `restored` (a recovery plan put it back) is dropped; one that is
//!   `purged` stays, no longer recoverable;
//! - each remaining path is resolved to the snapshot's entry by its components under the scan root (one read of a
//!   directory's children per directory touched); paths that are not in this snapshot are ignored, so a newer scan of
//!   the same root — finished after those operations — shows no overlay from them;
//! - sizes are the snapshot's stored sizes of the removed entries; an unknown size stays unknown (counted, never 0).
//!
//! The removed set is small next to a scan, and the overlay is computed once per scan and audit revision
//! ([`AuditStore::revision_token`](cm_store::AuditStore::revision_token)); callers cache it under that key.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use cm_core::path::RawPath;
use cm_core::plan::v2::{PLAN_V2_CONTRACT, PlanAction};
use cm_core::rules::Mode;
use cm_store::index::EntryItem;
use cm_store::{StoreError, Stores};

/// How an entry left its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovedMode {
    /// Moved to the Recycle Bin (macOS: the Trash).
    Trash,
    /// Moved into CleanManager's same-volume quarantine.
    Quarantine,
    /// Deleted permanently (a permanent-delete plan, an emptied folder, or a purged quarantine item).
    Permanent,
}

impl RemovedMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RemovedMode::Trash => "trash",
            RemovedMode::Quarantine => "quarantine",
            RemovedMode::Permanent => "permanent",
        }
    }
}

/// The removal of one object of the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovedMark {
    pub mode: RemovedMode,
    pub operation_id: String,
    /// When the action was recorded as succeeded.
    pub at: String,
    /// It can still be put back: from the bin (CleanManager cannot see the bin's later state), or from the quarantine
    /// while its item is held there.
    pub recoverable: bool,
}

/// The removed objects counted together: how many, the known part of their stored sizes, and how many of them have
/// a size that is unknown or only partly known (so the known part is "at least").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RemovedTotal {
    pub count: u64,
    pub known_bytes: u128,
    pub unknown_count: u64,
}

impl RemovedTotal {
    fn add(&mut self, item: &EntryItem) {
        self.count += 1;
        match item.logical_bytes {
            Some(b) => {
                self.known_bytes += u128::from(b.0);
                if item.unknown_descendants.unwrap_or(0) > 0 {
                    self.unknown_count += 1;
                }
            }
            None => self.unknown_count += 1,
        }
    }

    /// The known part as protocol text; `None` when nothing of it is known (unknown is never 0).
    pub fn known_text(&self) -> Option<String> {
        (self.count > 0 && (self.known_bytes > 0 || self.unknown_count < self.count)).then(|| self.known_bytes.to_string())
    }
}

/// What an entry's listing says about its removal: its own mark, or the mark of a removed folder above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRemoval<'a> {
    pub mark: &'a RemovedMark,
    /// Removed together with a folder above it (that folder carries the mark).
    pub inherited: bool,
}

/// See [`RemovedOverlay::touched`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touched<'a> {
    Removed(EntryRemoval<'a>),
    /// Removed objects beneath a directory that is itself still there.
    Inside(RemovedTotal),
}

/// The overlay of one scan at one audit revision.
#[derive(Debug, Clone, Default)]
pub struct RemovedOverlay {
    pub scan_id: String,
    /// The audit revision it was computed at (the cache key).
    pub audit_revision: String,
    /// Removed entries of the snapshot, by entry id.
    marks: HashMap<u64, RemovedMark>,
    /// The same entries by path key (descendants inherit the mark of a removed folder).
    by_path: HashMap<Vec<u16>, u64>,
    /// For each directory, the top-most removed entries strictly beneath it.
    beneath: HashMap<u64, RemovedTotal>,
    /// The top-most removed entries of the whole snapshot ("removed since scan").
    pub total: RemovedTotal,
    /// The most recent removal.
    pub latest_at: Option<String>,
    /// A digest of the marks: changes only when what the overlay says changes.
    pub signature: String,
}

impl RemovedOverlay {
    fn empty(scan_id: &str, audit_revision: String) -> Self {
        RemovedOverlay { scan_id: scan_id.into(), audit_revision, signature: "0".into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The mark of the entry itself (not inherited).
    pub fn mark(&self, entry_id: u64) -> Option<&RemovedMark> {
        self.marks.get(&entry_id)
    }

    /// The removal that applies to a path of this snapshot: its own mark, or that of the nearest removed folder above.
    pub fn of_path(&self, path: &RawPath) -> Option<EntryRemoval<'_>> {
        if self.marks.is_empty() {
            return None;
        }
        let key = path_key(path);
        if let Some(m) = self.by_path.get(&key).and_then(|id| self.marks.get(id)) {
            return Some(EntryRemoval { mark: m, inherited: false });
        }
        let sep = sep_of(path);
        // Nearest ancestor first.
        key.iter().enumerate().rev().filter(|(_, u)| **u == sep).find_map(|(i, _)| {
            self.by_path.get(&key[..i]).and_then(|id| self.marks.get(id)).map(|m| EntryRemoval { mark: m, inherited: true })
        })
    }

    /// The removal that applies to a listed entry.
    pub fn of_entry(&self, item: &EntryItem) -> Option<EntryRemoval<'_>> {
        if let Some(m) = item.entry_id.parse::<u64>().ok().and_then(|id| self.marks.get(&id)) {
            return Some(EntryRemoval { mark: m, inherited: false });
        }
        self.of_path(&item.path)
    }

    /// The top-most removed entries strictly beneath a directory (`None` when there are none).
    pub fn beneath(&self, entry_id: u64) -> Option<RemovedTotal> {
        self.beneath.get(&entry_id).copied()
    }

    /// How a later operation touched the snapshot's entry `entry_id` at `path`: it removed the entry or a folder above it,
    /// or something beneath it (then the snapshot no longer describes the folder). Candidate views leave such
    /// candidates out and plan creation refuses them, so a second plan never repeats what an earlier one removed.
    pub fn touched(&self, entry_id: u64, path: &RawPath) -> Option<Touched<'_>> {
        if let Some(r) = self.marks.get(&entry_id).map(|m| EntryRemoval { mark: m, inherited: false }).or_else(|| self.of_path(path)) {
            return Some(Touched::Removed(r));
        }
        self.beneath(entry_id).map(Touched::Inside)
    }

    /// Every removed entry with its mark, by entry id.
    pub fn marks(&self) -> impl Iterator<Item = (u64, &RemovedMark)> {
        self.marks.iter().map(|(id, m)| (*id, m))
    }
}

fn sep_of(p: &RawPath) -> u16 {
    match p {
        RawPath::Windows(_) => u16::from(b'\\'),
        RawPath::Unix(_) => u16::from(b'/'),
    }
}

/// A comparable form of a path: Windows units with `/` read as `\` and ASCII letters folded (the file system ignores
/// case), macOS bytes as they are; without a trailing separator.
fn path_key(p: &RawPath) -> Vec<u16> {
    let mut k: Vec<u16> = match p {
        RawPath::Windows(units) => units
            .iter()
            .map(|&u| if u == u16::from(b'/') { u16::from(b'\\') } else if (u16::from(b'A')..=u16::from(b'Z')).contains(&u) { u + 32 } else { u })
            .collect(),
        RawPath::Unix(bytes) => bytes.iter().map(|&b| u16::from(b)).collect(),
    };
    let sep = sep_of(p);
    while k.len() > 1 && k.last() == Some(&sep) {
        k.pop();
    }
    k
}

/// The names below `root` that lead to `path` (empty for the root itself); `None` when `path` is not under `root`.
fn components_below(root: &RawPath, path: &RawPath) -> Option<Vec<Vec<u16>>> {
    if root.platform() != path.platform() {
        return None;
    }
    let (r, p) = (path_key(root), path_key(path));
    let sep = sep_of(path);
    if p.len() < r.len() || p[..r.len()] != r[..] {
        return None;
    }
    let rest = &p[r.len()..];
    if rest.is_empty() {
        return Some(vec![]);
    }
    // The root may end with its separator (`C:\`); otherwise the next unit must be one.
    let rest = if r.last() == Some(&sep) { rest } else { rest.strip_prefix(&[sep])? };
    Some(rest.split(|u| *u == sep).filter(|s| !s.is_empty()).map(<[u16]>::to_vec).collect())
}

/// One removal fact before it is resolved to the snapshot.
struct Fact {
    path: RawPath,
    mark: RemovedMark,
}

/// The planned target of one action of a stored plan: its path, and what a success of it did to that path.
struct Target {
    path: RawPath,
    plan_mode: RemovedMode,
}

fn mode_of(m: Mode) -> RemovedMode {
    match m {
        Mode::Trash => RemovedMode::Trash,
        Mode::Quarantine => RemovedMode::Quarantine,
        Mode::Delete => RemovedMode::Permanent,
    }
}

/// The file targets of a stored plan by action id; empty for plans that remove no scanned path (recovery, startup…).
fn plan_targets(stores: &Stores, plan_id: &str) -> Result<HashMap<String, Target>, StoreError> {
    let contract = match stores.audit.plan_contract(plan_id) {
        Ok(c) => c,
        Err(StoreError::NotFound(_)) => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let mut out = HashMap::new();
    if contract == PLAN_V2_CONTRACT {
        let plan = stores.audit.load_plan_v2(plan_id)?;
        for a in &plan.body.items {
            let mode = match a {
                PlanAction::FsTrash(_) => RemovedMode::Trash,
                PlanAction::FsQuarantine(_) => RemovedMode::Quarantine,
                PlanAction::FsDelete(_) => RemovedMode::Permanent,
                _ => continue,
            };
            let f = a.fs().expect("file action");
            out.insert(f.action_id.clone(), Target { path: f.target.path.clone(), plan_mode: mode });
        }
    } else if contract == "plan/v1" {
        let plan = stores.audit.load_plan(plan_id)?;
        let mode = mode_of(plan.body.mode);
        // An emptied folder of a bin or quarantine plan (`not_applicable` recovery) goes only when everything planned
        // inside went the same way: it reads as its plan's mode, since putting its contents back brings it back.
        for i in &plan.body.items {
            out.insert(i.action_id.clone(), Target { path: i.path.clone(), plan_mode: mode });
        }
    }
    Ok(out)
}

/// What the recorded result says about where the object went (`recovery` of `action_result`), over the plan's mode.
fn result_mode(result_json: &str, plan_mode: RemovedMode) -> RemovedMode {
    let recovery = serde_json::from_str::<serde_json::Value>(result_json).ok().and_then(|v| v["recovery"].as_str().map(str::to_owned));
    match recovery.as_deref() {
        Some("trash") => RemovedMode::Trash,
        Some("quarantine") => RemovedMode::Quarantine,
        Some("unavailable") => RemovedMode::Permanent,
        _ => plan_mode,
    }
}

/// The removal facts recorded since `since`, oldest first; a path keeps its first (earliest) removal.
fn facts_since(stores: &Stores, since: &str) -> Result<Vec<Fact>, StoreError> {
    let rows = stores.audit.succeeded_since(since)?;
    let mut plans: HashMap<String, HashMap<String, Target>> = HashMap::new();
    let mut seen: HashMap<Vec<u16>, ()> = HashMap::new();
    let mut out = Vec::new();
    for row in rows {
        if !plans.contains_key(&row.plan_id) {
            let targets = plan_targets(stores, &row.plan_id)?;
            plans.insert(row.plan_id.clone(), targets);
        }
        let Some(target) = plans[&row.plan_id].get(&row.action_id) else { continue };
        let mut mode = result_mode(&row.result_json, target.plan_mode);
        let mut recoverable = mode != RemovedMode::Permanent;
        match row.quarantine_state.as_deref() {
            // Put back by a recovery plan: nothing is removed any more.
            Some("restored") => continue,
            Some("purged") => {
                mode = RemovedMode::Permanent;
                recoverable = false;
            }
            Some(_) => mode = RemovedMode::Quarantine,
            None if mode == RemovedMode::Quarantine => recoverable = false,
            None => {}
        }
        if seen.insert(path_key(&target.path), ()).is_some() {
            continue;
        }
        out.push(Fact { path: target.path.clone(), mark: RemovedMark { mode, operation_id: row.operation_id, at: row.result_at, recoverable } });
    }
    Ok(out)
}

/// Resolves paths to the snapshot's entries, reading each directory's children once.
struct Resolver<'a> {
    stores: &'a Stores,
    scan_id: &'a str,
    root: (u64, RawPath),
    children: HashMap<u64, HashMap<Vec<u16>, u64>>,
}

impl Resolver<'_> {
    /// The entry at `path` and its ancestors (root first, the entry itself excluded).
    fn resolve(&mut self, path: &RawPath) -> Result<Option<(u64, Vec<u64>)>, StoreError> {
        let Some(names) = components_below(&self.root.1, path) else { return Ok(None) };
        let mut cur = self.root.0;
        let mut chain = Vec::with_capacity(names.len());
        for name in names {
            if !self.children.contains_key(&cur) {
                let kids = self.stores.index.child_names(self.scan_id, cur)?;
                let map = kids.into_iter().map(|(id, n)| (path_key(&n), id)).collect();
                self.children.insert(cur, map);
            }
            let Some(&next) = self.children[&cur].get(&name) else { return Ok(None) };
            chain.push(cur);
            cur = next;
        }
        Ok(Some((cur, chain)))
    }
}

fn fnv(acc: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *acc ^= u64::from(*b);
        *acc = acc.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The overlay of `scan_id` at the audit store's current revision. A scan that has not finished, or has published
/// nothing, has an empty overlay.
pub fn removed_overlay(stores: &Stores, scan_id: &str) -> Result<RemovedOverlay, StoreError> {
    let audit_revision = stores.audit.revision_token()?;
    let status = stores.index.scan_status(scan_id)?;
    let mut out = RemovedOverlay::empty(scan_id, audit_revision);
    use cm_core::model::ScanState;
    if !status.published || matches!(status.state, ScanState::Created | ScanState::Running | ScanState::Stopping) {
        return Ok(out);
    }
    // The snapshot holds what the scan saw until it finished; anything removed from then on is not in it any more.
    let since = status.finished_at.clone().unwrap_or_else(|| status.heartbeat_at.clone());
    let facts = facts_since(stores, &since)?;
    if facts.is_empty() {
        return Ok(out);
    }
    let Some(root) = stores.index.root_entry(scan_id)? else { return Ok(out) };
    let mut resolver = Resolver { stores, scan_id, root, children: HashMap::new() };
    let mut resolved: BTreeMap<u64, (Vec<u64>, RemovedMark)> = BTreeMap::new();
    for f in facts {
        if let Some((id, chain)) = resolver.resolve(&f.path)? {
            resolved.entry(id).or_insert((chain, f.mark));
        }
    }
    let mut sig: u64 = 0xcbf2_9ce4_8422_2325;
    for (id, (chain, mark)) in &resolved {
        let item = stores.index.get_entry(scan_id, *id)?;
        fnv(&mut sig, format!("{id}|{}|{}|{}|{};", mark.mode.as_str(), mark.operation_id, mark.recoverable, mark.at).as_bytes());
        if out.latest_at.as_deref().is_none_or(|l| l < mark.at.as_str()) {
            out.latest_at = Some(mark.at.clone());
        }
        out.by_path.insert(path_key(&item.path), *id);
        // Only the top-most removed entries count in totals: what is inside a removed folder went with it.
        if chain.iter().any(|a| resolved.contains_key(a)) {
            continue;
        }
        out.total.add(&item);
        for a in chain {
            out.beneath.entry(*a).or_default().add(&item);
        }
    }
    out.marks = resolved.into_iter().map(|(id, (_, m))| (id, m)).collect();
    out.signature = format!("{:016x}.{}", sig, out.marks.len());
    Ok(out)
}

#[cfg(test)]
mod tests;
