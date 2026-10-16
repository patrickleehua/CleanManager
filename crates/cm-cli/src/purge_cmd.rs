//! `purge` command group (PUR-01..05, CLI-10/11/12): project scan roots, read-only discovery of
//! build artifacts (`purge scan`, equivalent to `purge --dry-run`) and plan creation from the
//! stored candidate sets. Discovery scans each configured root, analyzes it with the built-in
//! catalog and stores an ordinary candidate set, so `candidates list`, `plan show/review/apply`,
//! `operations` and recovery work unchanged. Nothing here approves a plan or touches a target.

#[cfg(not(any(windows, target_os = "macos")))]
use std::process::ExitCode;

#[cfg(not(any(windows, target_os = "macos")))]
use cm_core::error::ErrorCode;

#[cfg(not(any(windows, target_os = "macos")))]
use crate::args::Parsed;
#[cfg(not(any(windows, target_os = "macos")))]
use crate::fail;

#[cfg(any(windows, target_os = "macos"))]
mod review;
#[cfg(any(windows, target_os = "macos"))]
pub(crate) use review::*;

#[cfg(not(any(windows, target_os = "macos")))]
fn unsupported(req: &str, p: &Parsed) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "purge is implemented on Windows and macOS only in this build", p.format)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn roots(req: &str, p: &Parsed, _: Option<bool>) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn scan(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
    unsupported(req, p)
}

#[cfg(any(windows, target_os = "macos"))]
pub use discovery::{plan_create, roots, scan};

/// The platform side of discovery: default roots, the settings file, path conversion and the evidence probe.
#[cfg(windows)]
mod plat {
    pub use cm_platform::windows::purge::{FsProbe, default_roots, load_settings, os, raw, save_settings};

    /// Whether two root paths name the same folder (Windows paths compare case-insensitively).
    pub fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
        a.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
    }
}

#[cfg(target_os = "macos")]
mod plat {
    pub use cm_platform::macos::purge::{FsProbe, default_roots, load_settings, os, raw, same_dir, save_settings};
}

#[cfg(any(windows, target_os = "macos"))]
mod discovery {
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
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::plat::{FsProbe, default_roots, load_settings, os, save_settings};
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
            let Some(path) = super::plat::raw(&abs) else { return fail(req, ErrorCode::InvalidArgument, "invalid path", p.format) };
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
                match configured.iter().find(|c| os(c).is_some_and(|c| super::plat::same_dir(&c, &abs))) {
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
            if let Err(e) = run_scan(&stores.index, &crate::scan_cmd::backend(), &request, &ScanOptions::default()) {
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

/// Seam for `plan apply`: the execution-time gates of discovery plans on macOS. Purge plans (PUR-03) ask Git again for
/// each artifact before it is acted on; installer plans (INC-04) re-read the attached disk images so an image attached
/// after planning is skipped, never removed. `None` for every other plan.
#[cfg(target_os = "macos")]
#[allow(clippy::type_complexity)]
pub(crate) fn mac_vcs_gate(plan: &cm_core::plan::Plan) -> Option<Box<dyn Fn(&cm_core::execute::ActionTarget<'_>) -> Result<(), String>>> {
    use cm_platform::macos::{installer, purge};
    if purge::is_purge_plan(plan) {
        Some(Box::new(purge::vcs_gate(plan)))
    } else if installer::is_installer_plan(plan) {
        Some(Box::new(installer::mounted_gate(plan)))
    } else {
        None
    }
}
