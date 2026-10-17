//! Command dispatch. Each command emits exactly one terminal record and maps its outcome to the
//! stable exit classification (CLI-05). Every arm's options must also be declared in
//! `args::COMMANDS` (the table that completions and the CLI-12 checks are built from); a unit
//! test compares this file's `p.allow(..)` lists against it.

use std::path::PathBuf;
use std::process::ExitCode;

use cm_core::capability::CapabilityProvider;
use cm_core::error::ErrorCode;
use cm_core::protocol::capabilities_record;
use cm_store::{StoreError, Stores};
use serde_json::{Map, Value};

use crate::args::Parsed;
use crate::{Format, emit, fail};

pub fn run(req: &str, p: Parsed) -> ExitCode {
    // CLI-12: one check for every command group, before any command-specific code runs.
    if let Err(msg) = p.check_selection_widening() {
        return fail(req, ErrorCode::InvalidArgument, &msg, p.format);
    }
    // D15: a capability whose release gate has not passed refuses before any work unless experimental features are on.
    if let Err(code) = crate::features_cmd::guard(req, &p) {
        return code;
    }
    // LIF-01: write commands hold the lifecycle lock shared, so no update swaps files under them.
    let _lifecycle = match crate::self_cmd::write_guard(req, &p) {
        Ok(g) => g,
        Err(code) => return code,
    };
    let words: Vec<&str> = p.words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["capabilities"] => p.allow(&["store"], &[]).map(|_| capabilities(req, &p)),
        ["experimental"] => p.allow(&["store", "state"], &[]).map(|_| crate::features_cmd::experimental(req, &p)),
        ["scan"] => p.allow(&["root", "store", "timeout", "host-scan-id"], &["detach"]).map(|_| crate::scan_cmd::scan(req, &p)),
        ["scan", "show", id] => p.allow(&["store"], &[]).map(|_| crate::scan_cmd::scan_show(req, &p, id)),
        ["scan", "wait", id] => p.allow(&["store", "timeout"], &[]).map(|_| crate::scan_cmd::scan_wait(req, &p, id)),
        ["scan", "cancel", id] => p.allow(&["store"], &[]).map(|_| crate::scan_cmd::scan_cancel(req, &p, id)),
        ["entries", "list"] => p.allow(&["scan", "store", "sort", "kind", "parent", "name", "limit", "cursor"], &[]).map(|_| crate::scan_cmd::entries_list(req, &p)),
        ["completion", "print", shell] => p.allow(&[], &[]).map(|_| crate::completion_cmd::print(req, &p, shell)),
        ["completion", "install", shell] => p.allow(&["profile"], &[]).map(|_| crate::completion_cmd::install(req, &p, shell)),
        ["doctor"] => p.allow(&["store", "export"], &["no-redact"]).map(|_| crate::doctor_cmd::doctor(req, &p)),
        ["self", "update"] => p.allow(&["store", "manifest"], &["check"]).map(|_| crate::self_cmd::self_update(req, &p)),
        ["self", "install"] => p.allow(&["store", "dir"], &["add-to-path"]).map(|_| crate::self_cmd::self_install(req, &p)),
        ["self", "uninstall"] => p.allow(&["store", "install-dir"], &["remove-user-data"]).map(|_| crate::self_cmd::self_uninstall(req, &p)),
        ["help"] => p.allow(&[], &[]).map(|_| crate::completion_cmd::help(req, &p)),
        ["analyze"] => p.allow(&["scan", "store", "rules", "preview-rule"], &[]).map(|_| crate::analysis_cmd::analyze_cmd(req, &p)),
        ["candidates", "list"] => p.allow(&["set", "store", "limit", "cursor"], &[]).map(|_| crate::analysis_cmd::candidates_list(req, &p)),
        ["clean", "list"] => p.allow(&["rules"], &[]).map(|_| crate::clean_cmd::clean_list(req, &p)),
        ["clean"] => p
            .allow(&["scan", "store", "rules", "mode", "candidates", "volume", "limit", "cursor"], &["dry-run", "non-interactive"])
            .map(|_| crate::clean_cmd::clean(req, &p)),
        ["plan", "create"] => p.allow(&["set", "store", "mode", "candidates"], &["all-default-selected"]).map(|_| crate::plan_cmd::plan_create(req, &p)),
        ["plan", "show", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_show(req, &p, id)),
        ["plan", "review", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_review(req, &p, id)),
        ["plan", "revoke", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::plan_revoke(req, &p, id)),
        ["plan", "apply", id] => p.allow(&["store", "rules"], &[]).map(|_| crate::apply_cmd::plan_apply(req, &p, id)),
        ["operations", "show", id] => p.allow(&["store"], &[]).map(|_| crate::apply_cmd::operations_show(req, &p, id)),
        ["operations", "reconcile", id] => p.allow(&["store"], &[]).map(|_| crate::apply_cmd::operations_reconcile(req, &p, id)),
        ["operations", "list"] => p.allow(&["store", "limit", "cursor"], &[]).map(|_| crate::recovery_cmd::operations_list(req, &p)),
        ["recovery" | "quarantine", "list"] => p.allow(&["store", "limit", "cursor"], &[]).map(|_| crate::recovery_cmd::recovery_list(req, &p)),
        ["recovery", "show", id] => p.allow(&["store"], &[]).map(|_| crate::recovery_cmd::recovery_show(req, &p, id)),
        ["recovery", "plan", "create"] => p.allow(&["store", "restore", "purge"], &[]).map(|_| crate::recovery_cmd::recovery_plan_create(req, &p)),
        ["policy", "create"] => p.allow(&["store", "scope", "rules", "mode"], &[]).map(|_| crate::plan_cmd::policy_create(req, &p)),
        ["apps", "list"] => p.allow(&["limit", "cursor"], &[]).map(|_| crate::apps_cmd::apps_list(req, &p)),
        ["apps", "updates"] => p.allow(&["store"], &[]).map(|_| crate::updates_cmd::updates_check(req, &p)),
        ["apps", "updates", "show"] => p.allow(&["store"], &[]).map(|_| crate::updates_cmd::updates_show(req, &p)),
        ["apps", "updates", "config"] => p.allow(&["store", "checks"], &[]).map(|_| crate::updates_cmd::updates_config(req, &p)),
        ["apps", "update"] => p.allow(&["store", "app"], &[]).map(|_| crate::updates_cmd::update_plan(req, &p)),
        ["apps", "uninstall", "list"] => p.allow(&[], &[]).map(|_| crate::uninstall_cmd::list(req, &p)),
        ["apps", "uninstall", "plan", "create"] => p.allow(&["store", "instance"], &["end-running"]).map(|_| crate::uninstall_cmd::plan_create(req, &p)),
        ["apps", "uninstall", "leftovers"] => p.allow(&["store", "operation"], &[]).map(|_| crate::uninstall_cmd::leftovers(req, &p)),
        ["apps", "uninstall", "leftovers", "plan", "create"] => p.allow(&["store", "operation", "items", "exclude"], &[]).map(|_| crate::uninstall_cmd::leftovers_plan(req, &p)),
        ["apps", "uninstall", "orphans"] => p.allow(&[], &[]).map(|_| crate::uninstall_cmd::orphans(req, &p)),
        ["apps", "uninstall", "restore"] => p.allow(&["store", "operation"], &[]).map(|_| crate::uninstall_cmd::restore(req, &p)),
        ["startup", "list"] => p.allow(&[], &[]).map(|_| crate::startup_cmd::startup_list(req, &p)),
        ["startup", op @ ("disable" | "enable" | "remove-stale"), id] => p.allow(&["store"], &[]).map(|_| crate::startup_cmd::startup_plan(req, &p, op, id)),
        ["startup", "restore"] => p.allow(&["store", "operation"], &[]).map(|_| crate::startup_cmd::startup_restore(req, &p)),
        ["trash"] => p.allow(&["scan", "store", "entry"], &[]).map(|_| crate::trash_cmd::trash_plan(req, &p)),
        ["trash", "inspect"] => p.allow(&["scan", "store", "entry"], &[]).map(|_| crate::trash_cmd::trash_inspect(req, &p)),
        ["trash", "list"] => p.allow(&[], &[]).map(|_| crate::trash_cmd::trash_list(req, &p)),
        ["trash", "purge"] => p.allow(&["store", "item"], &[]).map(|_| crate::trash_cmd::trash_purge(req, &p)),
        ["status"] => p.allow(&["interval", "count", "store"], &["watch"]).map(|_| crate::status_cmd::status(req, &p)),
        ["status", "alerts"] => p.allow(&["store", "state", "cpu-threshold", "window-seconds", "cooldown-seconds"], &[]).map(|_| crate::status_cmd::status_alerts(req, &p)),
        ["process", "list"] => p.allow(&["store", "by", "search", "limit"], &[]).map(|_| crate::process_cmd::process_list(req, &p)),
        ["process", "show", pid] => p.allow(&["store"], &[]).map(|_| crate::process_cmd::process_show(req, &p, pid)),
        ["process", "pin", name] => p.allow(&["store"], &[]).map(|_| crate::process_cmd::process_pin(req, &p, name, true)),
        ["process", "unpin", name] => p.allow(&["store"], &[]).map(|_| crate::process_cmd::process_pin(req, &p, name, false)),
        ["process", "end", pid] => p.allow(&["store"], &[]).map(|_| crate::process_cmd::process_end(req, &p, pid)),
        ["insights"] => p.allow(&[], &[]).map(|_| crate::insights_cmd::insights_list(req, &p)),
        ["insights", "overview"] => p.allow(&["older-than-days"], &[]).map(|_| crate::insights_cmd::insights_overview(req, &p)),
        ["insights", "downloads"] => p.allow(&["older-than-days", "limit"], &[]).map(|_| crate::insights_cmd::insights_downloads(req, &p)),
        ["diag", "config"] => p.allow(&["store"], &[]).map(|_| crate::diag_cmd::diag_config(req, &p)),
        ["diag", "preview"] => p.allow(&["scan", "store", "entry", "provider", "origin", "disclose", "question"], &[]).map(|_| crate::diag_cmd::diag_preview(req, &p)),
        ["diag", "create"] => p.allow(&["scan", "store", "entry", "provider", "origin", "disclose", "question", "payload-digest"], &[]).map(|_| crate::diag_cmd::diag_create(req, &p)),
        ["diag", "show", id] => p.allow(&["store"], &[]).map(|_| crate::diag_cmd::diag_show(req, &p, id)),
        ["diag", "list"] => p.allow(&["store", "limit", "cursor"], &[]).map(|_| crate::diag_cmd::diag_list(req, &p)),
        ["policy", "list"] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_list(req, &p)),
        ["policy", "enable", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Enabled)),
        ["policy", "pause", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Paused)),
        ["policy", "revoke", id] => p.allow(&["store"], &[]).map(|_| crate::plan_cmd::policy_set_state(req, &p, id, cm_core::authz::PolicyState::Revoked)),
        ["protect", "list"] => p.allow(&["store"], &[]).map(|_| crate::protect_cmd::protect_list(req, &p)),
        ["protect", "add"] => p.allow(&["store", "path", "match", "glob", "rule", "task", "note"], &[]).map(|_| crate::protect_cmd::protect_add(req, &p)),
        ["protect", "remove", id] => p.allow(&["store"], &[]).map(|_| crate::protect_cmd::protect_remove(req, &p, id)),
        ["protect", "check"] => p.allow(&["store", "path", "rule"], &[]).map(|_| crate::protect_cmd::protect_check(req, &p)),
        ["purge", "roots"] | ["purge", "roots", "list"] => p.allow(&["store"], &[]).map(|_| crate::purge_cmd::roots(req, &p, None)),
        ["purge", "roots", "add"] => p.allow(&["store", "path"], &[]).map(|_| crate::purge_cmd::roots(req, &p, Some(true))),
        ["purge", "roots", "remove"] => p.allow(&["store", "path"], &[]).map(|_| crate::purge_cmd::roots(req, &p, Some(false))),
        ["purge", "scan" | "preview"] | ["purge"] if p.words.len() > 1 || p.flag("dry-run") => {
            p.allow(&["store", "root", "recent-days", "max-depth"], &["dry-run", "non-interactive"]).map(|_| crate::purge_cmd::scan(req, &p))
        }
        ["purge", "plan", "create"] => p.allow(&["store", "set", "mode", "candidates"], &["all-default-selected", "non-interactive"]).map(|_| crate::purge_cmd::plan_create(req, &p)),
        ["installer", "locations"] => p.allow(&["store"], &[]).map(|_| crate::installer_cmd::locations(req, &p)),
        ["installer", "scan" | "preview"] | ["installer"] if p.words.len() > 1 || p.flag("dry-run") => {
            p.allow(&["store", "source", "max-depth"], &["dry-run", "non-interactive"]).map(|_| crate::installer_cmd::scan(req, &p))
        }
        ["installer", "plan", "create"] => p.allow(&["store", "set", "mode", "candidates", "source"], &["all-default-selected", "non-interactive"]).map(|_| crate::installer_cmd::plan_create(req, &p)),
        ["optimize", "list"] => p.allow(&["store", "search"], &[]).map(|_| crate::optimize_cmd::list(req, &p)),
        ["optimize", "plan", "create"] => p.allow(&["store", "tasks", "volume"], &[]).map(|_| crate::optimize_cmd::plan_create(req, &p)),
        ["optimize", "diagnose"] => p.allow(&["window"], &[]).map(|_| crate::optimize_cmd::diagnose(req, &p)),
        ["optimize", "history"] => p.allow(&["store", "limit"], &[]).map(|_| crate::optimize_cmd::history(req, &p)),
        ["optimize", "restore"] => p.allow(&["store", "operation"], &[]).map(|_| crate::optimize_cmd::restore(req, &p)),
        [] => Err("missing command".to_string()),
        other => Err(format!("unknown command: {}", other.join(" "))),
    };
    match result {
        Ok(code) => code,
        Err(msg) => fail(req, ErrorCode::InvalidArgument, &msg, p.format),
    }
}

fn capabilities(req: &str, p: &Parsed) -> ExitCode {
    let host = cm_platform::host();
    let mut caps = host.capabilities();
    cm_app::features::apply_to(store_dir(p).as_deref(), &mut caps);
    emit(&capabilities_record(req, host.platform(), &caps), p.format);
    ExitCode::SUCCESS
}

pub fn store_dir(p: &Parsed) -> Option<PathBuf> {
    p.raw_option("store").map(PathBuf::from).or_else(cm_platform::paths::default_store_dir)
}

pub fn open_stores(req: &str, p: &Parsed) -> Result<Stores, ExitCode> {
    let Some(dir) = store_dir(p) else {
        return Err(fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format));
    };
    Stores::open(&dir).map_err(|e| store_error(req, &e, p.format))
}

pub fn store_error(req: &str, e: &StoreError, format: Format) -> ExitCode {
    fail(req, e.code(), &e.to_string(), format)
}

pub fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}
