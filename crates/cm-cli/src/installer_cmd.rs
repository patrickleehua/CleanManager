//! `installer` command group (INC-01..05, CLI-10/11/12): declared locations, read-only discovery
//! (`installer scan`, equivalent to `installer --dry-run`) and plan creation. Every installer is
//! review-only: plans come from explicit candidate ids or, in an interactive terminal, from one
//! source label. Plans are the shared identity-bound file plans (`plan review/apply`).

#[cfg(not(windows))]
use std::process::ExitCode;

#[cfg(not(windows))]
use crate::args::Parsed;

#[cfg(windows)]
pub(crate) const LAST_RUN_INSTALLER: &str = "installer-last-run.json";

#[cfg(not(windows))]
fn unsupported(req: &str, p: &Parsed) -> ExitCode {
    crate::fail(req, cm_core::error::ErrorCode::Unsupported, "installer cleanup is implemented on Windows only in this build", p.format)
}

#[cfg(not(windows))]
pub fn locations(req: &str, p: &Parsed) -> ExitCode {
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
pub use win::{locations, plan_create, scan};

#[cfg(windows)]
mod win {
    use std::process::ExitCode;

    use cm_app::scan::{ScanRequest, run_scan};
    use cm_core::analysis::{AnalysisProtection, Candidate, CandidateSet, CandidateState, UserList};
    use cm_core::error::ErrorCode;
    use cm_core::host::HostProbe;
    use cm_core::installer::{DEFAULT_MAX_DEPTH, InstallerInput, MAX_DEPTH_LIMIT, RULE_PREFIX, analyze_installers, is_current_digest};
    use cm_core::path::RawPath;
    use cm_core::protocol::record;
    use cm_core::scan::ScanOptions;
    use cm_platform::host::OsHostProbe;
    use cm_platform::windows::installer::{WinInstallerProbe, declared_locations, installed_apps};
    use cm_platform::windows::purge::os;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::LAST_RUN_INSTALLER;
    use crate::args::Parsed;
    use crate::commands::{fields, open_stores, store_error};
    use crate::purge_cmd::{PreviewRow, last_run_write, non_interactive, plan_from_sets, preview_record};
    use crate::{emit, fail};

    /// `CM_INSTALLER_LOCATIONS=label=PATH;label=PATH` replaces the declared locations (validation
    /// fixtures only; every preview item then carries a note saying so).
    fn override_locations() -> Option<Vec<(RawPath, &'static str)>> {
        let v = std::env::var("CM_INSTALLER_LOCATIONS").ok()?;
        let labels = [cm_platform::windows::installer::SOURCE_DOWNLOADS, cm_platform::windows::installer::SOURCE_DESKTOP, cm_platform::windows::installer::SOURCE_BROWSER];
        Some(
            v.split(';')
                .filter_map(|e| e.split_once('='))
                .filter_map(|(l, p)| labels.iter().find(|x| **x == l).and_then(|l| cm_platform::windows::purge::raw(std::path::Path::new(p)).map(|r| (r, *l))))
                .collect(),
        )
    }

    fn selected_locations(p: &Parsed) -> Result<Vec<(RawPath, &'static str)>, String> {
        let all = override_locations().unwrap_or_else(declared_locations);
        match p.option("source") {
            None => Ok(all),
            Some(label) => {
                let picked: Vec<_> = all.into_iter().filter(|(_, l)| *l == label).collect();
                if picked.is_empty() { Err(format!("no declared location has source {label} (see `installer locations`)")) } else { Ok(picked) }
            }
        }
    }

    /// `installer locations`: where discovery looks, with source labels (read-only).
    pub fn locations(req: &str, p: &Parsed) -> ExitCode {
        let items: Vec<_> = override_locations()
            .unwrap_or_else(declared_locations)
            .into_iter()
            .map(|(path, label)| json!({"path": path, "source_label": label, "exists": os(&path).is_some_and(|p| p.is_dir())}))
            .collect();
        emit(&record("installer_locations", req, fields(json!({"locations": items, "max_depth": DEFAULT_MAX_DEPTH}))), p.format);
        ExitCode::SUCCESS
    }

    fn tier_reason(c: &Candidate) -> String {
        let blocking = c.reasons.iter().find(|r| ["hard_deny", "user_protected", "user_protection_unavailable", "mounted", "in_use"].iter().any(|p| r.starts_with(p)));
        match (blocking, c.state) {
            (Some(b), _) => b.clone(),
            (None, CandidateState::Allowed) => "installers are review-only: choose them one by one or by source".into(),
            (None, _) => "evidence incomplete".into(),
        }
    }

    /// `installer scan` / `installer --dry-run`: scan the declared locations (bounded depth),
    /// classify by content, store the sets and return one preview.
    pub fn scan(req: &str, p: &Parsed) -> ExitCode {
        let max_depth = match p.option("max-depth").map(|v| v.parse::<u32>().ok().filter(|n| (1..=MAX_DEPTH_LIMIT).contains(n))) {
            None => DEFAULT_MAX_DEPTH,
            Some(Some(n)) => n,
            Some(None) => return fail(req, ErrorCode::InvalidArgument, &format!("--max-depth must be 1..={MAX_DEPTH_LIMIT}"), p.format),
        };
        let locations = match selected_locations(p) {
            Ok(l) => l,
            Err(e) => return fail(req, ErrorCode::InvalidArgument, &e, p.format),
        };
        let stores = match open_stores(req, p) {
            Ok(s) => s,
            Err(c) => return c,
        };
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
        let apps = installed_apps();
        let host = OsHostProbe.current("cli");
        let mut analyses = Vec::new();
        let mut runs = Vec::new();
        for (i, (root, label)) in locations.iter().enumerate() {
            let Some(path) = os(root) else { continue };
            if stores.ensure_outside(&path).is_err() {
                continue;
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
            let input = InstallerInput { scan_id: &scan_id, revision: status.revision, source_label: label, max_depth, apps: &apps };
            let a = analyze_installers(&view, &input, &prot, &WinInstallerProbe);
            if let Err(e) = stores.index.save_candidates(&a.set) {
                return store_error(req, &e, p.format);
            }
            runs.push((root.clone(), scan_id.clone(), a.set.candidate_set_id.clone()));
            analyses.push((scan_id, status.revision, a));
        }
        last_run_write(&stores.dir, LAST_RUN_INSTALLER, &runs);
        let rows: Vec<PreviewRow<'_>> = analyses
            .iter()
            .flat_map(|(_, _, a)| a.set.candidates.iter().zip(&a.facts).map(move |(c, f)| (a.set.candidate_set_id.as_str(), c, f)))
            .map(|(set_id, c, f)| {
                let mut notes = vec![format!("recognized: {}", f.evidence), format!("candidate set: {set_id}")];
                notes.push(match &f.signer {
                    Some(s) => format!("signed by {s}"),
                    None if f.kind.signable() => "no verified Authenticode signer".into(),
                    None => "images carry no Authenticode signer".into(),
                });
                if let Some(h) = &f.installed_hint {
                    notes.push(h.clone());
                }
                if override_locations().is_some() {
                    notes.push("location from CM_INSTALLER_LOCATIONS (validation fixture), not a declared user folder".into());
                }
                if f.mounted == Some(true) {
                    notes.push("attached: eject it in File Explorer (right-click > Eject) or with Dismount-DiskImage, then scan again".into());
                }
                PreviewRow { c, tier_reason: tier_reason(c), rebuild_class: "network_redownload", source_label: Some(f.source_label.clone()), notes }
            })
            .collect();
        let single = (analyses.len() == 1).then(|| (analyses[0].0.as_str(), analyses[0].1));
        emit(&preview_record(req, "installer.preview", single, non_interactive(p), &rows), p.format);
        ExitCode::SUCCESS
    }

    /// `installer plan create`: explicit `--candidates`, or `--source LABEL` in an interactive
    /// terminal (a person choosing a whole source); never an automatic selection.
    pub fn plan_create(req: &str, p: &Parsed) -> ExitCode {
        let is_installer_set = |s: &CandidateSet| is_current_digest(&s.rule_set_digest) && s.candidates.iter().all(|c| c.rules.iter().all(|r| r.rule_id.starts_with(RULE_PREFIX)));
        let Some(label) = p.option("source").map(str::to_owned) else {
            return plan_from_sets(req, p, LAST_RUN_INSTALLER, &is_installer_set, None);
        };
        if non_interactive(p) {
            return fail(req, ErrorCode::InvalidArgument, "selecting by source needs an interactive terminal; machine callers pass explicit --candidates", p.format);
        }
        let tag = format!("source:{label}");
        let from_source = move |c: &Candidate| c.reasons.contains(&tag);
        let what = format!("from source {label}");
        plan_from_sets(req, p, LAST_RUN_INSTALLER, &is_installer_set, Some((&from_source, &what)))
    }
}
