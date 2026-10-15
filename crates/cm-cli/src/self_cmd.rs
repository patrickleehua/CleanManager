//! `self update`, `self install` and `self uninstall` (LIF-01/02, CLI-01; parity 17.1/17.2, MVP 14.1).
//!
//! - `self update --check` verifies the signed release manifest and reports (`release_info`);
//!   `self update` additionally downloads, verifies and swaps the program (`self_update_result`).
//!   Package-manager copies get the package manager's command and are never touched.
//! - `self install --dir PATH [--add-to-path]` previews a copy of this release into a chosen
//!   directory, with every other `cleanmanager` on PATH reported; it installs only after a person
//!   types `install` in a terminal.
//! - `self uninstall` shows the reviewed removal plan (`self_remove_preview`); it removes anything
//!   only after a person types `uninstall` in a terminal, and only if the plan is unchanged.
//!
//! All three take the exclusive lifecycle lock while they change files; every other write command
//! holds it shared ([`write_guard`]).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, store_dir};
use crate::instance_lock::{self, LifecycleLock, LockError};
use crate::release::{self, ReleaseManifest, display};
use crate::update_self::{self, Detected, Status};
use crate::{Format, emit, fail};

/// Shared lifecycle hold for every write command except `self ...` (they hold it exclusively).
pub fn write_guard(req: &str, p: &Parsed) -> Result<Option<LifecycleLock>, ExitCode> {
    let words: Vec<&str> = p.words.iter().map(String::as_str).collect();
    if words.first() == Some(&"self") || !crate::args::spec_for(&words).is_some_and(|s| s.writes) {
        return Ok(None);
    }
    let Some(dir) = store_dir(p) else { return Ok(None) };
    match instance_lock::shared(&dir) {
        Ok(g) => Ok(g),
        Err(e @ LockError::Busy(_)) => Err(fail(req, ErrorCode::Conflict, &e.to_string(), p.format)),
        // An unusable lock file must not block ordinary work; the update side still refuses to run concurrently.
        Err(LockError::Io(..)) => Ok(None),
    }
}

/// The running program, without the `\\?\` prefix canonicalization adds on Windows.
fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let canon = std::fs::canonicalize(&exe).unwrap_or(exe);
    let s = canon.display().to_string();
    Some(match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => canon,
    })
}

struct Copy {
    exe: PathBuf,
    dir: PathBuf,
    release: Option<ReleaseManifest>,
    release_problem: Option<String>,
    detected: Detected,
}

fn this_copy() -> Option<Copy> {
    let exe = current_exe()?;
    let dir = exe.parent()?.to_path_buf();
    let (release, release_problem) = match ReleaseManifest::load(&dir) {
        Ok(r) => (r, None),
        Err(e) => (None, Some(e)),
    };
    let detected = update_self::detect(&exe, release.as_ref(), &update_self::os_env);
    Some(Copy { exe, dir, release, release_problem, detected })
}

// ---------------------------------------------------------------------------------------------
// self update

fn release_info(req: &str, d: &Detected, o: &update_self::Outcome) -> Value {
    record("release_info", req, fields(json!({
        "current_version": cm_core::ENGINE_VERSION,
        "latest_version": o.to_version,
        "release_id": o.release_id,
        "install_method": d.method.as_str(),
        "status": o.status.as_str(),
        "reason": o.reason,
        "checked_at": cm_core::time::now_utc(),
        "manager_command": d.update_command,
    })))
}

fn update_result(req: &str, d: &Detected, o: &update_self::Outcome) -> Value {
    record("self_update_result", req, fields(json!({
        "status": o.status.as_str(),
        "from_version": cm_core::ENGINE_VERSION,
        "to_version": o.to_version,
        "signature_verified": o.signature_verified,
        "checksum_verified": o.checksum_verified,
        "reason": o.reason,
        "install_method": d.method.as_str(),
        "manager_command": d.update_command,
        "backup_path": o.backup.as_deref().map(display),
    })))
}

fn exit_for(status: Status, config_problem: bool) -> ExitCode {
    let code = match status {
        Status::Updated | Status::UpToDate | Status::UpdateAvailable | Status::UsePackageManager => 0,
        Status::CannotCheck if config_problem => 7,
        Status::CannotCheck | Status::RolledBack | Status::RollbackIncomplete => 8,
        Status::VerificationFailed | Status::Incompatible => 3,
        Status::Locked => 6,
    };
    ExitCode::from(code)
}

fn simple(status: Status, reason: String) -> update_self::Outcome {
    update_self::Outcome { status, to_version: None, release_id: None, signature_verified: false, checksum_verified: false, reason: Some(reason), backup: None }
}

pub fn self_update(req: &str, p: &Parsed) -> ExitCode {
    let check = p.flag("check");
    let emit_outcome = |d: &Detected, o: &update_self::Outcome, config: bool| {
        emit(&if check { release_info(req, d, o) } else { update_result(req, d, o) }, p.format);
        exit_for(o.status, config)
    };
    let Some(copy) = this_copy() else { return fail(req, ErrorCode::InternalError, "cannot locate the running program", p.format) };
    let d = &copy.detected;
    if d.method.managed() {
        let o = simple(Status::UsePackageManager, format!(
            "this copy is managed by {}; update it with `{}` (CleanManager never replaces files a package manager owns)",
            d.method.as_str(),
            d.update_command.clone().unwrap_or_default()
        ));
        return emit_outcome(d, &o, false);
    }
    let Some(old) = copy.release.clone() else {
        let why = copy.release_problem.clone().unwrap_or_else(|| {
            "this copy is not a release layout (no cleanmanager-release.json next to the program: a development build or a loose binary); only release installs update themselves".into()
        });
        return emit_outcome(d, &simple(Status::CannotCheck, why), true);
    };
    let location = match p.raw_option("manifest").map(|s| s.to_string_lossy().into_owned()).or_else(|| update_self::default_channel().map(str::to_owned)) {
        Some(l) => l,
        None => return emit_outcome(d, &simple(Status::CannotCheck, "no update channel is configured in this build; pass --manifest URL|PATH of a signed release manifest".into()), true),
    };
    let keys = update_self::trusted_keys();
    if keys.is_empty() {
        return emit_outcome(d, &simple(Status::CannotCheck, "this build has no embedded update signing key, so no update can be verified and none is installed".into()), true);
    }
    let store = store_dir(p);
    let (index_v, audit_v) = store.as_deref().map(cm_store::migrate::store_versions).unwrap_or((None, None));
    let parse = |v: Option<String>| v.and_then(|s| s.parse::<u32>().ok());
    let smoke = update_self::smoke_test;
    let ctx = update_self::Context {
        install_dir: &copy.dir,
        current_version: cm_core::ENGINE_VERSION,
        target: &release::target_id(),
        manifest_location: &location,
        keys: &keys,
        transport: &update_self::SystemTransport,
        store_audit_schema: parse(audit_v),
        store_index_schema: parse(index_v),
        smoke: &smoke,
        fs: &update_self::RealFs,
    };
    if check {
        let o = update_self::check(&ctx);
        return emit_outcome(d, &o, false);
    }
    // One update/install at a time, and never under a running write (LIF-01).
    let locks = store.as_deref().map(instance_lock::exclusive).transpose().and_then(|s| instance_lock::exclusive(&copy.dir).map(|i| (s, i)));
    let _held = match locks {
        Ok(h) => h,
        Err(e) => return emit_outcome(d, &simple(Status::Locked, e.to_string()), false),
    };
    let o = update_self::apply(&ctx, &old);
    emit_outcome(d, &o, false)
}

// ---------------------------------------------------------------------------------------------
// self install

fn install_record(req: &str, status: &str, plan: &crate::install_self::InstallPlan, path_entry: &str, reason: Option<String>) -> Value {
    record("self_install_result", req, fields(json!({
        "status": status,
        "version": plan.release.version,
        "install_dir": display(&plan.target_dir),
        "files": plan.release.files.len(),
        "path_conflicts": plan.hits.iter().map(|h| json!({"path": display(&h.file), "relation": h.relation})).collect::<Vec<_>>(),
        "path_entry": path_entry,
        "reason": reason,
        "requires_interactive_confirmation": true,
    })))
}

pub fn self_install(req: &str, p: &Parsed) -> ExitCode {
    let Some(target) = p.raw_option("dir").map(PathBuf::from) else {
        return fail(req, ErrorCode::InvalidArgument, "--dir PATH is required: the directory to install into (new or empty)", p.format);
    };
    let Some(copy) = this_copy() else { return fail(req, ErrorCode::InternalError, "cannot locate the running program", p.format) };
    if copy.detected.method.managed() {
        return fail(req, ErrorCode::Unsupported, &format!(
            "this copy is managed by {}; install other copies with that package manager",
            copy.detected.method.as_str()
        ), p.format);
    }
    let user_path = crate::install_self::os_user_path();
    let path_var = std::env::var_os("PATH");
    let plan = match crate::install_self::plan(&copy.dir, &target, p.flag("add-to-path"), path_var.as_deref(), user_path.as_deref()) {
        Ok(pl) => pl,
        Err(e) => {
            let code = match e {
                crate::install_self::InstallError::Invalid(_) => ErrorCode::InvalidArgument,
                crate::install_self::InstallError::Conflict(_) => ErrorCode::Conflict,
                crate::install_self::InstallError::Io(_) => ErrorCode::IoError,
            };
            return fail(req, code, &e.to_string(), p.format);
        }
    };
    let different = plan.hits.iter().filter(|h| h.relation == "different").count();
    let reason = (different > 0).then(|| format!(
        "{different} other cleanmanager program(s) on PATH are left untouched; typing `cleanmanager` keeps running the first one on PATH — run this copy by its full path, or rename/remove the other one yourself"
    ));
    if p.format == Format::Json || p.non_interactive() || !crate::plan_cmd::interactive() {
        emit(&install_record(req, "preview", &plan, plan.path_entry, reason), p.format);
        return ExitCode::SUCCESS;
    }
    emit(&install_record(req, "preview", &plan, plan.path_entry, reason.clone()), Format::Text);
    match crate::plan_cmd::prompt(crate::i18n::t("prompt.self_install")) {
        Some(a) if a == "install" => {}
        _ => return fail(req, ErrorCode::Cancelled, "not installed; nothing was written", p.format),
    }
    let _held = match store_dir(p).map(|s| instance_lock::exclusive(&s)).transpose() {
        Ok(h) => h,
        Err(e) => return fail(req, ErrorCode::Conflict, &e.to_string(), p.format),
    };
    match crate::install_self::execute(&plan, user_path.as_deref()) {
        Ok(path_entry) => {
            emit(&install_record(req, "installed", &plan, path_entry, reason), p.format);
            ExitCode::SUCCESS
        }
        Err(e) => fail(req, ErrorCode::IoError, &e.to_string(), p.format),
    }
}

// ---------------------------------------------------------------------------------------------
// self uninstall

/// The desktop tray's startup value (RES-07), if it exists: its display.
fn resident_startup() -> Option<String> {
    #[cfg(windows)]
    {
        use cm_core::resident::StartupStore;
        let rv = cm_platform::windows::resident::RunValue::real();
        rv.read().ok().flatten().map(|cmd| format!("{} = {cmd}", rv.display()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn remove_resident_startup() -> Result<bool, String> {
    #[cfg(windows)]
    {
        use cm_core::resident::StartupStore;
        cm_platform::windows::resident::RunValue::real().remove()
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

fn credential_targets(store: Option<&Path>) -> Vec<String> {
    let Some(text) = store.and_then(|s| std::fs::read_to_string(s.join("diag-providers.json")).ok()) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(id) = o.get("provider_id").and_then(Value::as_str) {
                    out.push(id.to_owned());
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut ids = Vec::new();
    walk(&v, &mut ids);
    ids.sort();
    ids.dedup();
    let creds = cm_platform::credentials::os_store();
    ids.into_iter()
        .filter_map(|id| cm_platform::credentials::target_for(&id).ok())
        .filter(|t| creds.exists(t).unwrap_or(false))
        .collect()
}

fn remove_record(req: &str, status: &str, plan: &crate::uninstall_self::Plan, d: &Detected, done: &[crate::uninstall_self::Done]) -> Value {
    let outcome = |id: &str| done.iter().find(|x| x.item_id == id);
    record("self_remove_preview", req, fields(json!({
        "status": status,
        "remove": plan.items.iter().map(|i| json!({
            "item_id": i.item_id, "component": i.component, "action": i.action, "display": i.display,
            "outcome": outcome(&i.item_id).map(|x| x.outcome), "reason": outcome(&i.item_id).and_then(|x| x.reason.clone()),
        })).collect::<Vec<_>>(),
        "keep": plan.keep.iter().map(|k| json!({"data": k.data, "display": k.display})).collect::<Vec<_>>(),
        "quarantine_items": plan.quarantine_items,
        "quarantine_logical_bytes": plan.quarantine_bytes.to_string(),
        "system_settings_to_restore": plan.items.iter().filter(|i| i.component == "system_setting").map(|i| i.display.clone()).collect::<Vec<_>>(),
        "warnings": plan.warnings.iter().map(|w| json!({"code": w.code, "message": w.message})).collect::<Vec<_>>(),
        "notes": plan.notes,
        "install_method": d.method.as_str(),
        "uninstall_command": d.uninstall_command,
        "plan_digest": plan.digest,
        "requires_interactive_confirmation": true,
    })))
}

pub fn self_uninstall(req: &str, p: &Parsed) -> ExitCode {
    let Some(copy) = this_copy() else { return fail(req, ErrorCode::InternalError, "cannot locate the running program", p.format) };
    let (install_dir, detected) = match p.raw_option("install-dir").map(PathBuf::from) {
        Some(dir) if !dir.is_absolute() => return fail(req, ErrorCode::InvalidArgument, "--install-dir must be an absolute path", p.format),
        Some(dir) if !release::same_path(&dir, &copy.dir) => {
            let dir = release::normalize(&dir);
            let r = ReleaseManifest::load(&dir).ok().flatten();
            let d = update_self::detect(&dir.join(release::exe_name()), r.as_ref(), &update_self::os_env);
            (dir, d)
        }
        _ => (copy.dir.clone(), copy.detected.clone()),
    };
    let store = store_dir(p);
    let user_path = crate::install_self::os_user_path();
    let remove_user_data = p.flag("remove-user-data");
    let build = || {
        crate::uninstall_self::plan(&crate::uninstall_self::Inputs {
            install_dir: &install_dir,
            store_dir: store.as_deref(),
            detected: &detected,
            remove_user_data,
            profiles: ["bash", "zsh", "fish", "powershell"].into_iter().filter_map(|s| crate::completion_cmd::default_profile(s).map(|pr| (s, pr))).collect(),
            user_path: user_path.as_deref(),
            credential_targets: if remove_user_data { credential_targets(store.as_deref()) } else { Vec::new() },
            resident_startup: resident_startup(),
        })
    };
    let plan = build();
    if p.format == Format::Json || p.non_interactive() || !crate::plan_cmd::interactive() {
        emit(&remove_record(req, "preview", &plan, &detected, &[]), p.format);
        return ExitCode::SUCCESS;
    }
    emit(&remove_record(req, "preview", &plan, &detected, &[]), Format::Text);
    println!("plan_digest {}", plan.digest);
    match crate::plan_cmd::prompt(crate::i18n::t("prompt.self_uninstall")) {
        Some(a) if a == "uninstall" => {}
        _ => return fail(req, ErrorCode::Cancelled, "nothing was removed", p.format),
    }
    let _held = match store.as_deref().filter(|s| s.is_dir()).map(instance_lock::exclusive).transpose() {
        Ok(h) => h,
        Err(e) => return fail(req, ErrorCode::Conflict, &e.to_string(), p.format),
    };
    // Execute only the plan that was shown.
    let again = build();
    if again.digest != plan.digest {
        return fail(req, ErrorCode::PlanStale, "the footprint changed after the plan was shown; nothing was removed — run `self uninstall` again", p.format);
    }
    let delete_credential = |t: &str| cm_platform::credentials::os_store().delete(t).map(|_| ()).map_err(|e| e.to_string());
    let current = (install_dir == copy.dir).then(|| copy.exe.clone());
    let done = crate::uninstall_self::execute(&plan, &install_dir, &crate::uninstall_self::Effects {
        user_path: user_path.as_deref(),
        current_exe: current,
        delete_credential: &delete_credential,
        defer: &crate::uninstall_self::defer_removal,
        remove_resident_startup: &remove_resident_startup,
    });
    let failed = done.iter().any(|d| d.outcome == "failed");
    emit(&remove_record(req, if failed { "partial" } else { "removed" }, &plan, &detected, &done), p.format);
    if failed { ExitCode::from(5) } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_protocol_table() {
        assert_eq!(exit_for(Status::Updated, false), ExitCode::from(0));
        assert_eq!(exit_for(Status::UsePackageManager, false), ExitCode::from(0));
        assert_eq!(exit_for(Status::VerificationFailed, false), ExitCode::from(3));
        assert_eq!(exit_for(Status::Locked, false), ExitCode::from(6));
        assert_eq!(exit_for(Status::CannotCheck, true), ExitCode::from(7));
        assert_eq!(exit_for(Status::RolledBack, false), ExitCode::from(8));
    }
}
