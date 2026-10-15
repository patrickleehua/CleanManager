//! `purge` command group (PUR-01..05, CLI-10/11/12): project scan roots, read-only discovery of
//! build artifacts (`purge scan`, equivalent to `purge --dry-run`) and plan creation from the
//! stored candidate sets. Discovery scans each configured root, analyzes it with the built-in
//! catalog and stores an ordinary candidate set, so `candidates list`, `plan show/review/apply`,
//! `operations` and recovery work unchanged. Nothing here approves a plan or touches a target.

use std::process::ExitCode;

use cm_core::analysis::{Candidate, CandidateSet, CandidateState};
use cm_core::error::ErrorCode;
use cm_core::path::RawPath;
use cm_core::protocol::record;
use cm_core::rules::{Mode, Tier};
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, open_stores, store_error};
use crate::{emit, fail};

/// Per-store record of the latest discovery run of a command group: which candidate set came
/// from which root, so `plan create` can find a candidate without a set id.
pub(crate) const LAST_RUN_PURGE: &str = "purge-last-run.json";

pub(crate) fn last_run_write(dir: &std::path::Path, file: &str, sets: &[(RawPath, String, String)]) {
    let v = json!({"schema_version": 1, "sets": sets.iter().map(|(root, scan, set)| json!({"root": root, "scan_id": scan, "set_id": set})).collect::<Vec<_>>()});
    let tmp = dir.join(format!("{file}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, v.to_string()).is_ok() && std::fs::rename(&tmp, dir.join(file)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub(crate) fn last_run_sets(dir: &std::path::Path, file: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(file)) else { return vec![] };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v["sets"].as_array().map(|a| a.iter().filter_map(|s| s["set_id"].as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

pub(crate) fn plannable(c: &Candidate) -> bool {
    c.state == CandidateState::Allowed && c.tier != Tier::Blocked && !c.allowed_modes.is_empty()
}

pub(crate) fn selected_by_default(c: &Candidate) -> bool {
    plannable(c) && c.tier == Tier::DefaultSelected
}

/// Protocol `preview_item.skip_reason` from analysis reasons (first match by severity).
pub(crate) fn skip_reason(c: &Candidate) -> Option<&'static str> {
    let has = |p: &str| c.reasons.iter().any(|r| r.starts_with(p));
    if c.state == CandidateState::Blocked {
        return Some(if has("hard_deny") {
            "hard_denied"
        } else if has("vcs_tracked") || has("vcs_metadata_inside") {
            "vcs_tracked"
        } else if has("mounted") {
            "mounted"
        } else {
            "protected"
        });
    }
    if c.state != CandidateState::Allowed {
        return Some(if has("vcs_unknown") {
            "vcs_unknown"
        } else if has("mounted_image") {
            "mounted"
        } else if has("in_use:") {
            "in_use"
        } else if has("mounted_state_unknown") || has("in_use_unknown") {
            "probe_unknown"
        } else {
            "partial_evidence"
        });
    }
    has("recent_project").then_some("recent")
}

pub(crate) struct PreviewRow<'a> {
    pub c: &'a Candidate,
    pub tier_reason: String,
    pub rebuild_class: &'static str,
    pub source_label: Option<String>,
    pub notes: Vec<String>,
}

pub(crate) fn preview_item(r: &PreviewRow<'_>) -> Value {
    let c = r.c;
    let rule = c.rules.first();
    let mut v = json!({
        "object_id": c.candidate_id,
        "entry_id": c.entry_id.to_string(),
        "path": c.path,
        "category": c.category,
        "rule_id": rule.map(|r| r.rule_id.clone()),
        "rule_version": rule.map(|r| r.version.clone()),
        "tier": c.tier,
        "tier_reason": r.tier_reason,
        "rebuild_class": r.rebuild_class,
        "registry_status": "covered",
        "selected_by_default": selected_by_default(c),
        "skip_reason": skip_reason(c),
        "probes": [],
        "logical_bytes": (c.logical.unknown_count == 0).then(|| c.logical.known.0.to_string()),
        "reclaimable_estimate_bytes": null,
        "impact": c.impact,
        "notes": r.notes,
    });
    if let Some(s) = &r.source_label {
        v["source_label"] = json!(s);
    }
    v
}

pub(crate) fn preview_record(req: &str, command: &str, scan: Option<(&str, u32)>, non_interactive: bool, rows: &[PreviewRow<'_>]) -> Value {
    let selected: Vec<&Candidate> = rows.iter().map(|r| r.c).filter(|c| selected_by_default(c)).collect();
    let summary = json!({
        "selected_count": selected.len(),
        "selected_known_bytes": selected.iter().fold(0u64, |a, c| a.saturating_add(c.logical.known.0)).to_string(),
        "selected_unknown_size_count": selected.iter().map(|c| c.logical.unknown_count).sum::<u64>(),
        "review_only_count": rows.iter().filter(|r| r.c.tier == Tier::ReviewOnly).count(),
        "blocked_count": rows.iter().filter(|r| r.c.tier == Tier::Blocked).count(),
        "skipped_count": rows.iter().filter(|r| skip_reason(r.c).is_some()).count(),
    });
    record("selection_preview", req, fields(json!({
        "command": command,
        "scan_id": scan.map(|s| s.0),
        "scan_revision": scan.map(|s| s.1),
        "dry_run": true,
        "non_interactive": non_interactive,
        "items": rows.iter().map(preview_item).collect::<Vec<_>>(),
        "next_cursor": null,
        "page_size": rows.len().clamp(1, 500),
        "summary": summary,
        "convertible_to_plan": rows.iter().any(|r| plannable(r.c)),
    })))
}

pub(crate) fn non_interactive(p: &Parsed) -> bool {
    p.flag("non-interactive") || !crate::plan_cmd::interactive()
}

/// `--mode` for write commands: recoverable trash by default; permanent delete only when named.
pub(crate) fn mode(p: &Parsed) -> Result<Mode, String> {
    match p.option("mode") {
        None | Some("trash") => Ok(Mode::Trash),
        Some("quarantine") => Ok(Mode::Quarantine),
        Some("delete") => Ok(Mode::Delete),
        Some(other) => Err(format!("unknown mode {other}; use trash (default), quarantine or delete")),
    }
}

/// A selection made by a group-specific rule (e.g. "every installer from this source").
pub(crate) type Pick<'a> = (&'a dyn Fn(&Candidate) -> bool, &'a str);

/// Create a plan from one of the group's stored candidate sets. `is_group_set` rejects sets of
/// other commands or of an older catalog. Non-interactive default selection only ever takes
/// default-selected candidates; explicit ids (or an interactive `pick`) may widen it, and the
/// plan still needs approval.
pub(crate) fn plan_from_sets(req: &str, p: &Parsed, last_run: &str, is_group_set: &dyn Fn(&CandidateSet) -> bool, pick: Option<Pick<'_>>) -> ExitCode {
    let mode = match mode(p) {
        Ok(m) => m,
        Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
    };
    let stores = match open_stores(req, p) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let ids: Vec<String> = match p.option("set") {
        Some(id) => vec![id.to_owned()],
        None => last_run_sets(&stores.dir, last_run),
    };
    let mut sets = Vec::new();
    for id in &ids {
        match stores.index.candidate_set(id).and_then(|v| serde_json::from_value::<CandidateSet>(v).map_err(|e| cm_store::StoreError::IndexCorrupt(e.to_string()))) {
            Ok(s) if is_group_set(&s) => sets.push(s),
            Ok(_) if p.option("set").is_some() => return fail(req, ErrorCode::InvalidArgument, "the set was not produced by this command group with the current catalog", p.format),
            Ok(_) => {}
            Err(e) if p.option("set").is_some() => return store_error(req, &e, p.format),
            Err(_) => {}
        }
    }
    if sets.is_empty() {
        return fail(req, ErrorCode::InvalidArgument, "no candidate set: run the discovery (scan) first or pass --set ID", p.format);
    }
    let (set, selected): (&CandidateSet, Vec<String>) = match (p.option("candidates"), p.flag("all-default-selected"), pick) {
        (None, false, Some((rule, what))) => {
            let with: Vec<&CandidateSet> = sets.iter().filter(|s| s.candidates.iter().any(|c| plannable(c) && rule(c))).collect();
            match with.as_slice() {
                [one] => (*one, one.candidates.iter().filter(|c| plannable(c) && rule(c)).map(|c| c.candidate_id.clone()).collect()),
                [] => return fail(req, ErrorCode::InvalidArgument, &format!("no plannable candidate {what}"), p.format),
                _ => return fail(req, ErrorCode::InvalidArgument, &format!("candidates {what} come from several locations; pass --set ID"), p.format),
            }
        }
        (Some(list), false, None) => {
            let wanted: Vec<String> = list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
            let owners: Vec<&CandidateSet> = sets.iter().filter(|s| wanted.iter().any(|w| s.candidates.iter().any(|c| &c.candidate_id == w))).collect();
            match owners.as_slice() {
                [one] => (*one, wanted),
                [] => return fail(req, ErrorCode::InvalidArgument, "none of the candidates is in the latest discovery; pass --set ID", p.format),
                _ => return fail(req, ErrorCode::InvalidArgument, "the candidates come from several roots; create one plan per root with --set ID", p.format),
            }
        }
        (None, true, None) => {
            let with: Vec<&CandidateSet> = sets.iter().filter(|s| s.candidates.iter().any(selected_by_default)).collect();
            match with.as_slice() {
                [one] => (*one, one.candidates.iter().filter(|c| selected_by_default(c)).map(|c| c.candidate_id.clone()).collect()),
                [] => return fail(req, ErrorCode::InvalidArgument, "nothing is selected by default (review-only items need explicit --candidates)", p.format),
                many => {
                    let list: Vec<&str> = many.iter().map(|s| s.candidate_set_id.as_str()).collect();
                    return fail(req, ErrorCode::InvalidArgument, &format!("several roots have default selections; pass --set with one of {}", list.join(", ")), p.format);
                }
            }
        }
        _ => return fail(req, ErrorCode::InvalidArgument, "pass exactly one of --candidates ID,ID or --all-default-selected (installer: or --source LABEL)", p.format),
    };
    let view = match stores.index.view(&set.scan_id) {
        Ok(v) => v,
        Err(e) => return store_error(req, &e, p.format),
    };
    let status = match stores.index.scan_status(&set.scan_id) {
        Ok(s) => s,
        Err(e) => return store_error(req, &e, p.format),
    };
    let policies = stores.audit.policies().unwrap_or_default();
    let pdigest = cm_core::c14n::digest(&serde_json::to_value(&policies).unwrap()).expect("plain JSON");
    let plan_id = format!("p_{}", &req[4..]);
    let input = cm_core::plan::PlanInput {
        plan_id: &plan_id,
        scope_id: &status.scope_id,
        created_unix: crate::plan_cmd::now_unix(),
        observed_at: status.finished_at.as_deref().unwrap_or(&status.started_at),
        policy_digest: &pdigest,
    };
    let plan = match cm_core::plan::build_plan(&view, set, &selected, mode, &input) {
        Ok(p) => p,
        Err(e) => return fail(req, ErrorCode::PolicyDenied, &e.to_string(), p.format),
    };
    if let Err(e) = stores.audit.save_plan(&plan) {
        return store_error(req, &e, p.format);
    }
    emit(&crate::plan_cmd::plan_record(req, &stores, &plan), p.format);
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn unsupported(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "purge is implemented on Windows only in this build", p.format)
}

#[cfg(not(windows))]
pub fn roots(req: &str, p: &Parsed, _: Option<bool>) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(windows))]
pub fn scan(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(windows))]
pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(windows)]
pub use win::{plan_create, roots, scan};

#[cfg(windows)]
mod win {
    use std::path::Path;
    use std::process::ExitCode;

    use cm_app::scan::{ScanRequest, run_scan};
    use cm_core::analysis::{AnalysisProtection, UserList};
    use cm_core::error::ErrorCode;
    use cm_core::host::HostProbe;
    use cm_core::path::RawPath;
    use cm_core::protocol::record;
    use cm_core::purge::{ArtifactClass, MAX_DEPTH_LIMIT, PurgeInput, PurgeSettings, analyze_purge, artifact_for_rule};
    use cm_core::scan::ScanOptions;
    use cm_platform::host::OsHostProbe;
    use cm_platform::windows::purge::{FsProbe, default_roots, load_settings, os, save_settings};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{LAST_RUN_PURGE, PreviewRow, last_run_write, non_interactive, plan_from_sets, preview_record};
    use crate::args::Parsed;
    use crate::commands::{fields, open_stores, store_error};
    use crate::{emit, fail};

    fn settings(req: &str, p: &Parsed, dir: &Path) -> Result<PurgeSettings, ExitCode> {
        load_settings(dir).map_err(|e| fail(req, ErrorCode::InternalError, &format!("{e}; fix or remove {}", cm_core::purge::SETTINGS_FILE), p.format))
    }

    fn roots_record(req: &str, s: &PurgeSettings, defaults: &[RawPath]) -> Value {
        let roots: Vec<Value> = s
            .roots(defaults)
            .into_iter()
            .map(|(path, source)| json!({"path": path, "source": source, "exists": os(&path).is_some_and(|p| p.is_dir())}))
            .collect();
        record("purge_roots", req, fields(json!({"roots": roots})))
    }

    /// `purge roots [list]` (None), `purge roots add --path P` (Some(true)), `remove` (Some(false)).
    pub fn roots(req: &str, p: &Parsed, add: Option<bool>) -> ExitCode {
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let mut s = match settings(req, p, &stores.dir) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let defaults = default_roots();
        if let Some(add) = add {
            let Some(raw) = p.raw_option("path") else { return fail(req, ErrorCode::InvalidArgument, "needs --path PATH", p.format) };
            let abs = std::path::absolute(raw).unwrap_or_else(|_| raw.into());
            let Some(path) = cm_platform::windows::purge::raw(&abs) else { return fail(req, ErrorCode::InvalidArgument, "invalid path", p.format) };
            if add {
                if !abs.is_dir() {
                    return fail(req, ErrorCode::InvalidArgument, "not an existing directory", p.format);
                }
                if let Err(r) = cm_core::protect::prefilter(&path) {
                    return fail(req, ErrorCode::PolicyDenied, &format!("not a project root: {r}"), p.format);
                }
                if !s.add_root(path, &defaults) {
                    return fail(req, ErrorCode::InvalidArgument, "already a configured root", p.format);
                }
            } else if !s.remove_root(&path, &defaults) {
                return fail(req, ErrorCode::InvalidArgument, "not a configured root", p.format);
            }
            if let Err(e) = save_settings(&stores.dir, &s) {
                return fail(req, ErrorCode::IoError, &format!("cannot save purge settings: {e}"), p.format);
            }
        }
        emit(&roots_record(req, &s, &defaults), p.format);
        ExitCode::SUCCESS
    }

    fn number(p: &Parsed, key: &str, range: std::ops::RangeInclusive<u32>, default: u32) -> Result<u32, String> {
        match p.option(key) {
            None => Ok(default),
            Some(v) => v.parse::<u32>().ok().filter(|n| range.contains(n)).ok_or_else(|| format!("--{key} must be {}..={}", range.start(), range.end())),
        }
    }

    fn tier_reason(c: &cm_core::analysis::Candidate, class: Option<ArtifactClass>) -> String {
        let blocking = c.reasons.iter().find(|r| {
            ["hard_deny", "user_protected", "secret_file", "protected:", "vcs_tracked", "vcs_metadata_inside", "cloud_placeholder", "vcs_unknown", "unread_or_unknown_size", "user_protection_unavailable"]
                .iter()
                .any(|p| r.starts_with(p))
        });
        if let Some(b) = blocking {
            return b.clone();
        }
        match (class, c.tier) {
            (Some(ArtifactClass::Dependency), _) => "dependencies: review only, reinstalling needs the network".into(),
            (_, cm_core::rules::Tier::ReviewOnly) if c.reasons.iter().any(|r| r.starts_with("recent_project")) => "recent project: listed, not preselected".into(),
            (_, cm_core::rules::Tier::ReviewOnly) => "build output that may hold hand-written files: review it".into(),
            _ => "build output next to its project marker; rebuilt locally".into(),
        }
    }

    /// `purge scan` / `purge --dry-run`: scan each configured root, analyze, store the sets and
    /// return one preview. Nothing outside this tool's store is written.
    pub fn scan(req: &str, p: &Parsed) -> ExitCode {
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let s = match settings(req, p, &stores.dir) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let (recent_days, max_depth) = match (number(p, "recent-days", 0..=365, s.recent_days), number(p, "max-depth", 1..=MAX_DEPTH_LIMIT, s.max_depth)) {
            (Ok(r), Ok(m)) => (r, m),
            (Err(e), _) | (_, Err(e)) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
        };
        let configured: Vec<RawPath> = s.roots(&default_roots()).into_iter().map(|(r, _)| r).collect();
        let roots: Vec<RawPath> = match p.raw_option("root") {
            Some(raw) => {
                let abs = std::path::absolute(raw).unwrap_or_else(|_| raw.into());
                match configured.iter().find(|c| os(c).is_some_and(|c| c.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(abs.to_string_lossy().trim_end_matches('\\')))) {
                    Some(r) => vec![r.clone()],
                    None => return fail(req, ErrorCode::InvalidArgument, "not a configured purge root; add it with `purge roots add --path PATH`", p.format),
                }
            }
            None => configured,
        };
        if roots.is_empty() {
            return fail(req, ErrorCode::InvalidArgument, "no purge roots configured; add one with `purge roots add --path PATH`", p.format);
        }
        let hard = cm_platform::protection::hard_deny_set();
        let user = cm_platform::protection::load(&stores.dir);
        let problem = user.as_ref().err().map(ToString::to_string);
        let prot = AnalysisProtection {
            hard_deny: Some(&hard),
            user: match (&user, &problem) {
                (Ok(list), _) => UserList::Loaded(list),
                (Err(_), why) => UserList::Unavailable(why.as_deref().unwrap_or_default()),
            },
            worktrees: &[],
        };
        let now = crate::plan_cmd::now_unix();
        let host = OsHostProbe.current("cli");
        let mut analyses = Vec::new();
        let mut runs = Vec::new();
        for (i, root) in roots.iter().enumerate() {
            let Some(path) = os(root).filter(|p| p.is_dir()) else { continue };
            if stores.ensure_outside(&path).is_err() {
                continue; // never scan a root that contains this tool's store
            }
            let digest: String = Sha256::digest(root.raw_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
            let scan_id = format!("s_{}_{i}", &req[4..]);
            let request = ScanRequest { scan_id: scan_id.clone(), scope_id: format!("scope_{digest}"), root: root.clone(), host: host.clone() };
            if let Err(e) = run_scan(&stores.index, &cm_platform::windows::directory::DirectoryBackend, &request, &ScanOptions::default()) {
                return store_error(req, &e, p.format);
            }
            let (status, view) = match (stores.index.scan_status(&scan_id), stores.index.view(&scan_id)) {
                (Ok(s), Ok(v)) => (s, v),
                (Err(e), _) | (_, Err(e)) => return store_error(req, &e, p.format),
            };
            let input = PurgeInput { scan_id: &scan_id, revision: status.revision, now_unix: now, recent_days, max_depth };
            let a = analyze_purge(&view, &input, &prot, &FsProbe::default());
            if let Err(e) = stores.index.save_candidates(&a.set) {
                return store_error(req, &e, p.format);
            }
            runs.push((root.clone(), scan_id.clone(), a.set.candidate_set_id.clone()));
            analyses.push((scan_id, status.revision, a));
        }
        last_run_write(&stores.dir, LAST_RUN_PURGE, &runs);
        let rows: Vec<PreviewRow<'_>> = analyses
            .iter()
            .flat_map(|(_, _, a)| a.set.candidates.iter().zip(&a.facts).map(move |(c, f)| (a.set.candidate_set_id.as_str(), c, f)))
            .map(|(set_id, c, f)| {
                let artifact = c.rules.first().and_then(|r| artifact_for_rule(&r.rule_id));
                let mut notes = vec![format!("project: {} ({})", f.project.display(), f.markers.join(", ")), format!("candidate set: {set_id}")];
                if let Some(a) = artifact {
                    notes.push(format!("to get it back: {}", a.restore));
                }
                if f.recent {
                    notes.push(format!("project modified within {recent_days} days"));
                }
                PreviewRow {
                    c,
                    tier_reason: tier_reason(c, artifact.map(|a| a.class)),
                    rebuild_class: match f.rebuild_class {
                        cm_core::rules::RebuildClass::LocalRebuild => "local_rebuild",
                        cm_core::rules::RebuildClass::NetworkRedownload => "network_redownload",
                        cm_core::rules::RebuildClass::NotRebuildable => "not_rebuildable",
                    },
                    source_label: None,
                    notes,
                }
            })
            .collect();
        let single = (analyses.len() == 1).then(|| (analyses[0].0.as_str(), analyses[0].1));
        emit(&preview_record(req, "purge.preview", single, non_interactive(p), &rows), p.format);
        ExitCode::SUCCESS
    }

    /// `purge plan create`: a fixed cleanup plan (default mode trash) from a purge candidate set.
    pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
        let is_purge = |s: &cm_core::analysis::CandidateSet| {
            cm_core::purge::is_current_digest(&s.rule_set_digest) && s.candidates.iter().all(|c| c.rules.iter().all(|r| r.rule_id.starts_with(cm_core::purge::RULE_PREFIX)))
        };
        plan_from_sets(req, p, LAST_RUN_PURGE, &is_purge, None)
    }
}
