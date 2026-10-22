//! macOS uninstall plans (parity 9.1 macOS half, UNI-01..11; design D7): the same two separately reviewed plans and
//! the same batch rules as on Windows, on the macOS routes.
//!
//! - Stage 1: per application an `app_quit` and one `tool_adapter`: `macos.trash-app-bundle` (the bundle to the Trash,
//!   bound to its identity, `Contents/Info.plist` hash and signature publisher; never deleted) or
//!   `homebrew.uninstall-cask` / `homebrew.uninstall-formula` (the trusted `brew`, fixed argv, never `--zap`). App
//!   Store apps, bundles this user cannot change and protected classes are refused per application (UNI-10/11).
//! - Stage 2: leftovers under `~/Library` of every application whose own removal succeeded, from the macOS record
//!   frozen in the stage-1 plan (bundle ID, Team ID). Files go to the Trash as `fs_trash` actions bound to a fresh scan
//!   with the same protection and coverage checks as a manual selection. The macOS Trash action moves single verified
//!   files, so a leftover folder is planned as its files (the emptied folders stay); folders with more than
//!   [`MAX_FILES_PER_OBJECT`] files are refused with the Finder route. Privacy-protected Containers, shared Group
//!   Containers and login items are reported only. There are no registry steps on macOS.
//! - AI command-line tools (UNI-07) take the same two stages: stage 1 runs `npm uninstall --global` for the package
//!   (`npm.uninstall-global`), stage 2 offers their data folder with sessions and settings never preselected.
//! - Orphans (UNI-08): [`create_orphans_plan`] puts the orphaned leftovers a person named into their own reviewed
//!   `uninstall` plan of `fs_trash` actions, through the same file path as stage 2; nothing is ever preselected.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use cm_core::host::HostRef;
use cm_core::manual_trash::{ManualInput, SelectionGuard, build_trash_plan};
use cm_core::model::ObjectKind;
use cm_core::path::RawPath;
use cm_core::plan::v2::{ExecutableRef, ToolAdapterAction};
use cm_core::uninstall::{ADAPTER_VERSION, Binding, Group, Leftover, UNINSTALLER_TIMEOUT_SECONDS, UninstallRecord, adapter_digest};
use cm_platform::macos::uninstall::{UninstallEnv, discover, installed, records, route, verify_program};

use super::*;

/// Most files one leftover folder may be planned as (the macOS Trash action moves single files).
pub const MAX_FILES_PER_OBJECT: usize = 5000;

/// A stage-1 plan with its (non-binding) leftover estimate by group.
pub struct StageOne {
    pub plan: PlanV2,
    pub record: UninstallRecord,
    pub estimate: BTreeMap<Group, usize>,
}

/// One application accepted into a batch: its record, its actions and its (non-binding) leftover estimate.
pub struct BatchApp {
    pub record: UninstallRecord,
    pub action_ids: Vec<String>,
    pub estimate: BTreeMap<Group, usize>,
}

/// One application refused exactly as a single uninstall of it would be (UNI-10/11); the others are unaffected.
pub struct BatchRefusal {
    pub app_instance_id: String,
    pub name: Option<String>,
    pub refused: Refused,
}

/// A stage-1 plan for several applications (UNI-11) and the applications that were refused.
pub struct StageOneBatch {
    pub plan: PlanV2,
    pub apps: Vec<BatchApp>,
    pub refused: Vec<BatchRefusal>,
}

/// Stage 1 for one instance; `end_running` is the separately reviewed variant that may end processes after a person
/// confirms it at apply time.
pub fn create_uninstall_plan(stores: &Stores, env: &UninstallEnv, instance_id: &str, end_running: bool, plan_id: &str, now: i64, policy_digest: &str) -> Result<StageOne, Refused> {
    let mut b = create_uninstall_batch(stores, env, &[instance_id.to_owned()], end_running, plan_id, now, policy_digest)?;
    let app = b.apps.remove(0);
    Ok(StageOne { plan: b.plan, record: app.record, estimate: app.estimate })
}

/// Stage 1 for several instances in one plan (UNI-11): each checked exactly as on its own; refused ones are reported
/// and left out; only when every one is refused is the whole request refused.
pub fn create_uninstall_batch(
    stores: &Stores, env: &UninstallEnv, instance_ids: &[String], end_running: bool, plan_id: &str, now: i64, policy_digest: &str,
) -> Result<StageOneBatch, Refused> {
    if instance_ids.is_empty() {
        return Err(Refused::new("invalid_argument", "name at least one application instance"));
    }
    if instance_ids.len() > MAX_BATCH {
        return Err(Refused::new("invalid_argument", format!("at most {MAX_BATCH} applications in one uninstall plan")));
    }
    for (i, id) in instance_ids.iter().enumerate() {
        if instance_ids[..i].contains(id) {
            return Err(Refused::new("invalid_argument", format!("{id} is named twice")));
        }
    }
    let (all, _) = records(env);
    let mut items = Vec::new();
    let mut meta = Vec::new();
    let mut apps = Vec::new();
    let mut refused = Vec::new();
    for id in instance_ids {
        match app_actions(&all, env, id, end_running, apps.len()) {
            Ok((rec, identity, acts)) => {
                let action_ids = acts.iter().map(|(a, _)| a.action_id().to_owned()).collect();
                for (a, m) in acts {
                    items.push(a);
                    meta.push(m);
                }
                // Estimate only (D7): what is there now, by group; never part of the plan, sizes not read.
                let others: Vec<_> = installed(&all).into_iter().filter(|a| a.instance_id != rec.instance_id).collect();
                let mut estimate = BTreeMap::new();
                for l in discover(env, &identity, rec.mac.as_ref(), &others, false) {
                    *estimate.entry(l.group).or_insert(0) += 1;
                }
                apps.push(BatchApp { record: rec, action_ids, estimate });
            }
            Err(r) => refused.push(BatchRefusal { app_instance_id: id.clone(), name: all.iter().find(|x| &x.instance_id == id).map(|x| x.name.clone()), refused: r }),
        }
    }
    if apps.is_empty() {
        return Err(batch_refusal(refused.into_iter().map(|b| (b.name.unwrap_or(b.app_instance_id), b.refused)).collect()));
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Uninstall, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
    };
    let plan = cm_core::plan::v2::build(&header, items, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok(StageOneBatch { plan, apps, refused })
}

/// The two stage-1 actions of one application (the `n`-th of its plan), after its route was bound.
#[allow(clippy::type_complexity)]
fn app_actions(all: &[UninstallRecord], env: &UninstallEnv, instance_id: &str, end_running: bool, n: usize) -> Result<(UninstallRecord, AppIdentity, Vec<(PlanAction, ActionMeta)>), Refused> {
    let Some(rec) = all.iter().find(|r| r.instance_id == instance_id).cloned() else {
        return Err(Refused::new("invalid_argument", format!("no installed application {instance_id}; run `cleanmanager apps uninstall list`")));
    };
    let choice = route(env, &rec).map_err(|r| Refused { code: r.code.into(), message: r.message, settings_uri: r.settings_uri.map(str::to_owned) })?;
    let (sha, signer) = verify_program(&choice).map_err(|(c, m)| Refused::new(c, m))?;
    let identity = AppIdentity::of(&rec);
    let app_meta = UninstallMeta::App { identity: identity.clone(), record: Box::new(rec.clone()), adapter: choice.clone(), signer };
    let program = RawPath::unix(choice.program.as_bytes().to_vec()).map_err(|e| Refused::new("unsupported", e.to_string()))?;
    let items = vec![
        PlanAction::AppQuit(AppQuitAction {
            action_id: batch_action_id(n, false), app_instance_id: rec.instance_id.clone(),
            app_identity: rec.install_location.clone().unwrap_or_else(|| rec.name.clone()),
            force: if end_running { QuitForce::AskUser } else { QuitForce::Never },
        }),
        PlanAction::ToolAdapter(ToolAdapterAction {
            action_id: batch_action_id(n, true), adapter_id: choice.kind.adapter_id().into(), adapter_version: ADAPTER_VERSION.into(),
            executable: ExecutableRef { path: program, sha256: sha }, argv: choice.argv.clone(), preview_digest: adapter_digest(&rec, &choice),
            timeout_seconds: UNINSTALLER_TIMEOUT_SECONDS,
        }),
    ];
    let meta = vec![
        ActionMeta { note: Some(format!("quit {} (quit request{})", rec.name, if end_running { "; may end it after you confirm" } else { "" })), uninstall: Some(app_meta.clone()), ..Default::default() },
        ActionMeta { note: Some(choice.display.clone()), uninstall: Some(app_meta), ..Default::default() },
    ];
    Ok((rec, identity, items.into_iter().zip(meta).collect()))
}

/// The record of `instance_id` as frozen in a stage-1 plan.
fn planned_record(plan: &PlanV2, instance_id: &str) -> Option<UninstallRecord> {
    plan.meta.iter().find_map(|m| match &m.uninstall {
        Some(UninstallMeta::App { record, .. }) if record.instance_id == instance_id => Some((**record).clone()),
        _ => None,
    })
}

/// The leftover review of one application of a stage-1 operation (UNI-04/05/09/11).
pub struct AppLeftovers {
    pub outcome: AppOutcome,
    pub identity: AppIdentity,
    /// Empty when its own removal did not succeed: nothing of an application that is still installed is a leftover.
    pub items: Vec<Leftover>,
    /// Read-only hints in other files (not collected on macOS in this build).
    pub external_references: Vec<String>,
}

/// The grouped, read-only leftover preview of a finished stage-1 operation, per application in plan order.
pub fn leftovers_by_app(stores: &Stores, env: &UninstallEnv, operation_id: &str) -> Result<Vec<AppLeftovers>, Refused> {
    let (plan, apps, report) = stage_one(stores, operation_id)?;
    let Some(report) = report else {
        return Err(Refused::new("invalid_argument", format!("operation {operation_id} has not finished; leftovers are reviewed after the removal ran")));
    };
    let outcomes = app_outcomes(&plan, Some(&report));
    let (all, _) = records(env);
    // A record exactly as planned of an app whose removal succeeded is a leftover of it (a bundle still listed); a
    // changed or new record of the same app is `app_reappeared`. An app of the batch that was not removed protects
    // what it shares.
    let gone: Vec<(&str, &str)> =
        apps.iter().zip(&outcomes).filter(|(_, o)| o.leftovers_reviewable).map(|(a, _)| (a.identity.instance_id.as_str(), a.record_digest.as_str())).collect();
    let live: Vec<UninstallRecord> = all.into_iter().filter(|r| !gone.iter().any(|(i, d)| r.instance_id == *i && r.digest() == *d)).collect();
    let still = installed(&live);
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (app, outcome) in apps.into_iter().zip(outcomes) {
        let items = if outcome.leftovers_reviewable {
            let rec = planned_record(&plan, &app.identity.instance_id);
            let mac = rec.as_ref().and_then(|r| r.mac.as_ref());
            discover(env, &app.identity, mac, &still, true).into_iter().filter(|l| seen.insert(l.object_id.clone())).collect()
        } else {
            vec![]
        };
        out.push(AppLeftovers { outcome, identity: app.identity, items, external_references: vec![] });
    }
    Ok(out)
}

/// Why an operation has nothing to review: every application's removal failed, was skipped or did not run.
pub fn none_reviewable(operation_id: &str, apps: &[AppLeftovers]) -> Option<Refused> {
    if apps.iter().any(|a| a.outcome.leftovers_reviewable) {
        return None;
    }
    let why: Vec<String> = apps.iter().map(|a| format!("{} ({}{})", a.outcome.name, a.outcome.status, a.outcome.code.as_deref().map(|c| format!(": {c}")).unwrap_or_default())).collect();
    Some(Refused::new(
        "invalid_argument",
        format!("the uninstall in operation {operation_id} did not complete for {}; its leftovers are not reviewed or removed while the application is still installed", why.join(", ")),
    ))
}

/// Stage 2: freeze default-selected leftovers plus explicitly named objects into a separately reviewed `uninstall`
/// plan of `fs_trash` actions; with a batch, of every application whose removal succeeded (UNI-11).
#[allow(clippy::too_many_arguments)]
pub fn create_leftovers_plan(
    stores: &Stores, env: &UninstallEnv, operation_id: &str, extra: &[String], exclude: &[String], host: &HostRef, plan_id: &str, now: i64, policy_digest: &str,
) -> Result<(PlanV2, Vec<Leftover>), Refused> {
    let by_app = leftovers_by_app(stores, env, operation_id)?;
    if let Some(r) = none_reviewable(operation_id, &by_app) {
        return Err(r);
    }
    let batch = by_app.len() > 1;
    let owner: BTreeMap<String, String> = by_app.iter().flat_map(|a| a.items.iter().map(|l| (l.object_id.clone(), a.identity.name.clone()))).collect();
    let all: Vec<Leftover> = by_app.into_iter().flat_map(|a| a.items).collect();
    for x in exclude {
        if !all.iter().any(|l| &l.object_id == x && l.selected) {
            return Err(Refused::new("invalid_argument", format!("{x} is not a preselected leftover of this operation (only caches/logs chosen by default can be excluded)")));
        }
    }
    for x in extra {
        match all.iter().find(|l| &l.object_id == x) {
            None => return Err(Refused::new("invalid_argument", format!("{x} is not a leftover of this operation (use object ids from `apps uninstall leftovers`)"))),
            Some(l) if l.tier == "kept" || l.tier == "blocked" => {
                let why = l.note.clone().unwrap_or_else(|| l.evidence.join("; "));
                return Err(Refused::new(if l.tier == "blocked" { "permission_required" } else { "policy_denied" }, format!("{} is {}: {why}", l.display, l.tier)));
            }
            Some(Leftover { binding: Binding::Manual { guidance }, display, .. }) => return Err(Refused::new("unsupported", format!("{display}: {guidance}"))),
            Some(_) => {}
        }
    }
    let chosen: Vec<Leftover> = all.iter().filter(|l| (l.selected && !exclude.contains(&l.object_id)) || extra.contains(&l.object_id)).cloned().collect();
    if chosen.is_empty() {
        return Err(Refused::new("invalid_argument", "nothing is selected: no cache or log leftovers were found; name objects with --items to include them"));
    }
    let hard = cm_platform::protection::hard_deny_set();
    let user = cm_platform::protection::load(&stores.dir);
    let problem = user.as_ref().err().map(ToString::to_string);
    let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
    let mut items = Vec::new();
    let mut meta = Vec::new();
    for (n, l) in chosen.iter().enumerate() {
        let app = |s: String| match owner.get(&l.object_id).filter(|_| batch) {
            Some(name) => format!("{name} · {s}"),
            None => s,
        };
        match &l.binding {
            Binding::Path { path } => {
                for (a, m) in file_actions(stores, env, host, plan_id, n, path, &guard, now, policy_digest)? {
                    items.push(a);
                    meta.push(ActionMeta { note: Some(app(format!("{}: {}", l.group.as_str(), l.display))), ..m });
                }
            }
            Binding::Registry { .. } | Binding::Startup { .. } => return Err(Refused::new("unsupported", format!("{}: registry and startup changes are not made on macOS", l.display))),
            Binding::Manual { .. } => {}
        }
    }
    if items.is_empty() {
        return Err(Refused::new("invalid_argument", "the selected leftovers are already empty or gone; nothing to plan"));
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Uninstall, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
    };
    let plan = cm_core::plan::v2::build(&header, items, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok((plan, chosen))
}

/// UNI-08: a separately reviewed `uninstall` plan moving orphaned leftovers a person named (`apps uninstall orphans`
/// object ids) to the Trash. Orphans are never preselected, so only named objects are planned, each found again in a
/// fresh discovery; kept, blocked and report-only ones are refused. Files go through the same `fs_trash` path as
/// leftovers (fresh scan, protection and coverage checks; folders as their files).
#[allow(clippy::too_many_arguments)]
pub fn create_orphans_plan(stores: &Stores, env: &UninstallEnv, items: &[String], host: &HostRef, plan_id: &str, now: i64, policy_digest: &str) -> Result<(PlanV2, Vec<Leftover>), Refused> {
    if items.is_empty() {
        return Err(Refused::new("invalid_argument", "orphaned leftovers are never preselected: name them with --items (object ids from `apps uninstall orphans`)"));
    }
    let all = cm_platform::macos::uninstall::orphans(env, false);
    let mut chosen: Vec<Leftover> = Vec::new();
    for x in items {
        match all.iter().find(|l| &l.object_id == x) {
            None => return Err(Refused::new("invalid_argument", format!("{x} is not an orphaned leftover now (use object ids from `apps uninstall orphans`)"))),
            Some(l) if l.tier == "kept" || l.tier == "blocked" => {
                let why = l.note.clone().unwrap_or_else(|| l.evidence.join("; "));
                return Err(Refused::new(if l.tier == "blocked" { "permission_required" } else { "policy_denied" }, format!("{} is {}: {why}", l.display, l.tier)));
            }
            Some(Leftover { binding: Binding::Manual { guidance }, display, .. }) => return Err(Refused::new("unsupported", format!("{display}: {guidance}"))),
            Some(l) if !chosen.iter().any(|c| c.object_id == l.object_id) => chosen.push(l.clone()),
            Some(_) => {}
        }
    }
    let hard = cm_platform::protection::hard_deny_set();
    let user = cm_platform::protection::load(&stores.dir);
    let problem = user.as_ref().err().map(ToString::to_string);
    let guard = SelectionGuard { hard: Some(&hard), user: user.as_ref().map_err(|_| problem.as_deref().unwrap_or("unavailable")) };
    let mut actions = Vec::new();
    let mut meta = Vec::new();
    for (n, l) in chosen.iter().enumerate() {
        if let Binding::Path { path } = &l.binding {
            for (a, m) in file_actions(stores, env, host, plan_id, n, path, &guard, now, policy_digest)? {
                actions.push(a);
                meta.push(ActionMeta { note: Some(format!("orphan · {}: {}", l.group.as_str(), l.display)), ..m });
            }
        }
    }
    if actions.is_empty() {
        return Err(Refused::new("invalid_argument", "the named orphaned leftovers are already empty or gone; nothing to plan"));
    }
    let header = HeaderInput {
        plan_id, plan_kind: PlanKind::Uninstall, created_unix: now, scope_id: None, scan_id: None, scan_revision: None, rule_set_digest: NO_RULES_DIGEST, policy_digest,
    };
    let plan = cm_core::plan::v2::build(&header, actions, meta).map_err(|e| Refused::new("internal_error", e.to_string()))?;
    stores.audit.save_plan_v2(&plan)?;
    Ok((plan, chosen))
}

fn raw(p: &Path) -> Result<RawPath, Refused> {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).map_err(|e| Refused::new("invalid_argument", e.to_string()))
}

/// `fs_trash` actions for one leftover, bound to a fresh scan and the same protection and coverage checks as a manual
/// selection (N3: the Trash, never a delete). A file is bound through a scan of its own folder (never the home folder
/// or `~/Library` itself); a folder is scanned itself and planned as the regular files below it.
#[allow(clippy::too_many_arguments)]
fn file_actions(
    stores: &Stores, env: &UninstallEnv, host: &HostRef, plan_id: &str, n: usize, path: &str, guard: &SelectionGuard<'_>, now: i64, policy_digest: &str,
) -> Result<Vec<(PlanAction, ActionMeta)>, Refused> {
    let p = Path::new(path);
    let meta = p.symlink_metadata().map_err(|e| Refused::new("conflict", format!("{path}: {e}")))?;
    if meta.file_type().is_symlink() {
        return Err(Refused::new("unsupported", format!("{path} is a symbolic link; it is not followed")));
    }
    let is_dir = meta.is_dir();
    let root = if is_dir { p } else { p.parent().ok_or_else(|| Refused::new("invalid_argument", "no parent folder"))? };
    let broad = [env.apps.home.clone(), env.library()];
    if broad.iter().flatten().any(|b| b.as_path() == root || (is_dir && b.as_path() == p)) {
        return Err(Refused::new("unsupported", format!("{path} sits directly in {}; remove it yourself after review", root.display())));
    }
    let raw_root = raw(root)?;
    if let Err(why) = cm_platform::macos::actions::trash::trash_ready(&raw_root) {
        return Err(Refused::new("unsupported", format!("trash_unavailable: {path}: {why}; there is no permanent-delete alternative")));
    }
    let scan_id = format!("{plan_id}_s{n}");
    let request = crate::scan::ScanRequest { scan_id: scan_id.clone(), scope_id: format!("scope_{plan_id}_{n}"), root: raw_root, host: host.clone() };
    let outcome = crate::scan::run_scan(&stores.index, &cm_platform::macos::posix::PosixBackend, &request, &cm_core::scan::ScanOptions::default())?;
    if outcome.state != cm_core::model::ScanState::Completed {
        return Err(Refused::new("conflict", format!("{path} could not be read completely ({}); it is not planned", outcome.state.as_str())));
    }
    let status = stores.index.scan_status(&scan_id)?;
    let view = stores.index.view(&scan_id)?;
    use cm_core::analysis::IndexView;
    let entries: Vec<u64> = if is_dir {
        // The macOS Trash action moves single verified files: plan the regular files below the folder.
        let mut files = Vec::new();
        let mut stack: Vec<u64> = view.children(view.root()).to_vec();
        while let Some(e) = stack.pop() {
            let info = view.info(e);
            match info.kind {
                ObjectKind::Directory if !info.boundary => stack.extend_from_slice(view.children(e)),
                ObjectKind::File if !info.boundary => files.push(e),
                _ => {}
            }
            if files.len() > MAX_FILES_PER_OBJECT {
                return Err(Refused::new(
                    "unsupported",
                    format!("{path} holds more than {MAX_FILES_PER_OBJECT} files; move the folder to the Trash in Finder (whole folders are not moved by this build)"),
                ));
            }
        }
        files.sort_by_key(|&e| view.path(e).display());
        files
    } else {
        let name = p.file_name().map(|f| f.as_bytes().to_vec()).unwrap_or_default();
        let found = view.children(view.root()).iter().copied().find(|&c| view.path(c).raw_bytes().ends_with(&[b"/".as_slice(), &name].concat()));
        vec![found.ok_or_else(|| Refused::new("conflict", format!("{path} is gone")))?]
    };
    if entries.is_empty() {
        return Ok(vec![]);
    }
    let observed_at = status.finished_at.clone().unwrap_or(status.started_at.clone());
    let input = ManualInput {
        plan_id, scope_id: Some(&status.scope_id), scan_id: Some(&scan_id), scan_revision: Some(status.revision), created_unix: now, observed_at: &observed_at, policy_digest,
    };
    let built = build_trash_plan(&view, &entries, guard, &input).map_err(|e| Refused {
        code: match e.code() {
            cm_core::error::ErrorCode::PolicyDenied => "policy_denied".into(),
            cm_core::error::ErrorCode::Unsupported => "unsupported".into(),
            cm_core::error::ErrorCode::PermissionRequired => "permission_required".into(),
            _ => "invalid_argument".into(),
        },
        message: format!("{path}: {e}"),
        settings_uri: None,
    })?;
    Ok(built
        .body
        .items
        .into_iter()
        .zip(built.meta)
        .enumerate()
        .map(|(i, (mut a, m))| {
            if let PlanAction::FsTrash(f) = &mut a {
                f.action_id = format!("f{n:04}{i:04}");
            }
            (a, m)
        })
        .collect())
}
