//! `doctor` (LIF-04): a read-only diagnostic report for issue reports — version, platform,
//! capability matrix, permission context, configuration validity, the error-code table, recent
//! error codes from the operation history and a store self-check.
//!
//! The report never contains credentials, environment variables or file contents. It is
//! redacted by default: the home directory becomes `~`, and the user and computer names become
//! `<user>`/`<host>` wherever they appear. `--export PATH` previews the exact redacted report
//! and writes the file only after a person types `export` in an interactive text terminal;
//! machine callers only ever receive the preview.
//!
//! `--repair-index` is the one change doctor can make: an unusable scan index is moved aside (kept, never
//! deleted) and replaced by an empty one; a healthy index is left alone (see `repair_index`).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::capability::CapabilityProvider;
use cm_core::error::{ErrorCode, ExitClass};
use cm_core::protocol::record;
use cm_store::Stores;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::commands::{fields, store_dir};
use crate::i18n::t;
use crate::{Format, emit, fail};

fn check(area: &str, name: &str, status: &str, detail: impl Into<String>) -> Value {
    json!({"area": area, "check": name, "status": status, "detail": detail.into()})
}

/// Replace the home directory, user name and computer name in every string (case-insensitive).
pub struct Redactor {
    pairs: Vec<(String, &'static str)>,
}

impl Redactor {
    pub fn from_env() -> Redactor {
        let mut pairs = Vec::new();
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok();
        if let Some(h) = home.filter(|h| h.len() > 3) {
            pairs.push((h.trim_end_matches(['\\', '/']).to_owned(), "~"));
        }
        for (var, mask) in [("USERNAME", "<user>"), ("USER", "<user>"), ("COMPUTERNAME", "<host>"), ("HOSTNAME", "<host>")] {
            if let Ok(v) = std::env::var(var)
                && v.len() >= 2
            {
                // Windows 8.3 aliases of long names (`C:\Users\LONGNA~1`) leak the name too.
                let alias: String = v.chars().filter(|c| !c.is_whitespace() && *c != '.').take(6).collect();
                if v.chars().count() > 8 && alias.chars().count() == 6 {
                    pairs.extend((1..=9).map(|n| (format!("{alias}~{n}"), mask)));
                }
                pairs.push((v, mask));
            }
        }
        // Longest first, so the home directory is replaced before the user name inside it.
        pairs.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        Redactor { pairs }
    }

    pub fn text(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for (needle, mask) in &self.pairs {
            out = replace_ci(&out, needle, mask);
        }
        out
    }

    /// Redact a record. Raw path bytes cannot be redacted piecewise, so redacted paths keep only their display.
    pub fn value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.value(x))).collect()),
            other => other.clone(),
        }
    }
}

fn replace_ci(haystack: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return haystack.to_owned();
    }
    let lower_h = haystack.to_lowercase();
    let lower_n = needle.to_lowercase();
    // Byte offsets only line up when lowercasing kept the length (ASCII-like text); otherwise compare exactly.
    if lower_h.len() != haystack.len() || lower_n.len() != needle.len() {
        return haystack.replace(needle, with);
    }
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while let Some(pos) = lower_h[i..].find(&lower_n) {
        out.push_str(&haystack[i..i + pos]);
        out.push_str(with);
        i += pos + needle.len();
    }
    out.push_str(&haystack[i..]);
    out
}

fn store_checks(dir: &Path, checks: &mut Vec<Value>, recent: &mut BTreeMap<String, (u64, Option<String>)>) {
    if !dir.exists() {
        checks.push(check("store", "store.present", "warning", format!("no store yet at {} (created by the first scan)", dir.display())));
        return;
    }
    match Stores::open(dir) {
        Err(e) => checks.push(check("store", "store.open", "failed", format!("{e} ({})", e.code().as_str()))),
        Ok(stores) => {
            checks.push(check("store", "store.open", "ok", format!("audit store at {} passed SQLite quick_check and its schema version check", dir.display())));
            // Doctor always runs the index's full check; other commands only when it is due (PERF-WFA-01).
            match stores.index.verify() {
                Ok(()) => checks.push(check("store", "store.index", "ok", "scan index passed SQLite quick_check and its schema version check")),
                Err(e) => checks.push(check("store", "store.index", "failed", match e.required_action() {
                    Some(action) => format!("{e} ({}); history, quarantine and recovery are not affected. Run `{action}`: the damaged file is kept as index.db.damaged-<time>, an empty index is created and scans must be run again", e.code().as_str()),
                    None => format!("{e} ({})", e.code().as_str()),
                })),
            }
            match stores.index.list_scans(1, None) {
                Ok((items, _)) => checks.push(check("store", "store.index.scans", "ok", match items.first() {
                    Some(s) => format!("latest scan {} is {}", s.scan_id, serde_json::to_value(s.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()),
                    None => "no scans recorded".into(),
                })),
                Err(e) => checks.push(check("store", "store.index.scans", "failed", e.to_string())),
            }
            match stores.audit.quarantine_items() {
                Ok(q) => checks.push(check("store", "store.quarantine", "ok", format!("{} quarantine record(s)", q.len()))),
                Err(e) => checks.push(check("store", "store.quarantine", "failed", e.to_string())),
            }
            match stores.audit.operations_page(50, None) {
                Ok((ops, _)) => {
                    let unresolved = ops.iter().filter(|o| o.state == "reconciliation_required").count();
                    checks.push(check("store", "store.operations", if unresolved > 0 { "warning" } else { "ok" },
                        format!("{} recent operation(s), {unresolved} awaiting reconciliation", ops.len())));
                    for op in &ops {
                        let Ok(Some(report)) = stores.audit.operation_report(&op.operation_id) else { continue };
                        let Ok(v) = serde_json::from_str::<Value>(&report) else { continue };
                        for item in v["items"].as_array().into_iter().flatten().filter(|i| i["status"] != "succeeded") {
                            if let Some(code) = item["code"].as_str() {
                                let e = recent.entry(code.to_owned()).or_insert((0, None));
                                e.0 += 1;
                                e.1 = e.1.take().max(op.finished_at.clone());
                            }
                        }
                    }
                }
                Err(e) => checks.push(check("store", "store.operations", "failed", e.to_string())),
            }
        }
    }
    match cm_platform::protection::load(dir) {
        Ok(_) => checks.push(check("config", "config.protection_list", "ok", "user protection list is readable and valid")),
        Err(e) => checks.push(check("config", "config.protection_list", "failed", format!("protection list unavailable (writes stop): {e}"))),
    }
    let purge = dir.join(cm_core::purge::SETTINGS_FILE);
    if purge.exists() {
        let ok = std::fs::read_to_string(&purge).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).is_some();
        checks.push(check("config", "config.purge_settings", if ok { "ok" } else { "failed" }, if ok { "purge settings parse" } else { "purge settings are not valid JSON" }));
    }
}

/// Build the (unredacted) report value.
fn report(req: &str, store: Option<PathBuf>) -> Value {
    let host = cm_platform::host();
    let mut checks = Vec::new();
    checks.push(check("platform", "platform.engine", "ok", format!("engine {} on {}, schema {}", cm_core::ENGINE_VERSION, host.platform().as_str(), cm_core::SCHEMA_VERSION)));
    #[cfg(windows)]
    match cm_platform::windows::privileges::enabled_bypass_privileges() {
        Ok(p) if p.is_empty() => checks.push(check("permissions", "permissions.bypass_privileges", "ok", "ordinary-permission context: no ACL-bypass privilege is enabled")),
        Ok(p) => checks.push(check("permissions", "permissions.bypass_privileges", "warning", format!("enabled: {}", p.join(", ")))),
        Err(e) => checks.push(check("permissions", "permissions.bypass_privileges", "unknown", e.to_string())),
    }
    let caps = host.capabilities();
    let limited = caps.iter().filter(|c| !matches!(c.state, cm_core::capability::CapabilityState::Available)).count();
    checks.push(check("permissions", "permissions.capabilities", if limited > 0 { "warning" } else { "ok" }, format!("{limited} of {} capabilities are limited (see capabilities)", caps.len())));
    let rules_dir = crate::analysis_cmd::default_rules_dir();
    match crate::analysis_cmd::load_rules_and_registry(&rules_dir) {
        Ok((rules, registry)) => checks.push(check("config", "config.rules", "ok", format!("{} rules, {} registry entries, digest {}", rules.rules.len(), registry.len(), rules.digest))),
        Err(e) => checks.push(check("config", "config.rules", "failed", format!("rule set unavailable: {e}"))),
    }
    let mut recent = BTreeMap::new();
    match &store {
        Some(dir) => store_checks(dir, &mut checks, &mut recent),
        None => checks.push(check("store", "store.present", "failed", "no store directory; pass --store or set CM_STORE_DIR")),
    }
    let codes: Vec<Value> = ErrorCode::ALL
        .iter()
        .map(|c| json!({"code": c, "exit_code": ExitClass::Error(*c).code(), "retryable": c.retryable()}))
        .collect();
    let recent: Vec<Value> = recent.into_iter().map(|(code, (count, last))| json!({"code": code, "count": count, "last_seen": last})).collect();
    record("doctor_report", req, fields(json!({
        "generated_at": cm_core::time::now_utc(),
        "redacted": false,
        "engine_version": cm_core::ENGINE_VERSION,
        "platform": host.platform(),
        "capabilities": caps,
        "checks": checks,
        "error_codes": codes,
        "recent_errors": recent,
        "export": null,
    })))
}

/// `doctor --repair-index` (BUG-WFA-01): a full check of the scan index; only an unusable one is moved aside
/// (kept as `index.db.damaged-<time>`, never deleted) and replaced by an empty index. The audit store, and with it
/// history, quarantine and recovery, is never touched. Runs before the report, so the report shows the result.
fn repair_index(req: &str, p: &Parsed) -> Result<Value, ExitCode> {
    let Some(dir) = store_dir(p) else {
        return Err(fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format));
    };
    let unchanged = |state: &str, message: &str| json!({"status": "not_needed", "previous_state": state, "kept_files": [], "message": message});
    if !dir.is_dir() {
        return Ok(unchanged("missing", "There is no store yet; nothing was changed."));
    }
    // CleanManager's own files change: no update may replace the program meanwhile (LIF-01).
    let _lifecycle = match crate::instance_lock::shared(&dir) {
        Ok(g) => g,
        Err(e @ crate::instance_lock::LockError::Busy(_)) => return Err(fail(req, ErrorCode::Conflict, &e.to_string(), p.format)),
        Err(crate::instance_lock::LockError::Io(..)) => None,
    };
    match Stores::repair_index(&dir) {
        Ok(r) if r.repaired => {
            let kept: Vec<String> = r.kept.iter().map(|k| k.display().to_string()).collect();
            Ok(json!({
                "status": "repaired",
                "previous_state": r.previous_state,
                "kept_files": kept,
                "message": format!(
                    "The scan index was unusable ({}). It was moved aside as {} and an empty index was created. Operation history, quarantine and recovery were not touched. Earlier scans and candidate sets are no longer available: run scan again.",
                    r.problem.unwrap_or_default(), kept.join(", ")
                ),
            }))
        }
        Ok(r) if r.previous_state == "missing" => Ok(unchanged("missing", "There is no scan index yet; nothing was changed (the next scan creates it).")),
        Ok(r) => Ok(unchanged(r.previous_state, "The scan index passed the full integrity check; nothing was changed.")),
        // The file could not be moved: on Windows another process holds it open. Everything moved was put back.
        Err(cm_store::StoreError::Io(e)) => Err(fail(
            req,
            ErrorCode::Conflict,
            &format!("{e}. Another CleanManager process (a running scan or the desktop app) may hold the index open; close it and run doctor --repair-index again. Nothing was changed."),
            p.format,
        )),
        Err(e) => Err(crate::commands::store_error(req, &e, p.format)),
    }
}

pub fn doctor(req: &str, p: &Parsed) -> ExitCode {
    let redact = !p.flag("no-redact");
    let repair = if p.flag("repair-index") {
        match repair_index(req, p) {
            Ok(r) => Some(r),
            Err(code) => return code,
        }
    } else {
        None
    };
    let mut v = report(req, store_dir(p));
    if let Some(r) = repair {
        v["index_repair"] = r;
    }
    if redact {
        v = Redactor::from_env().value(&v);
        v["redacted"] = json!(true);
    }
    let Some(target) = p.raw_option("export").map(PathBuf::from) else {
        emit(&v, p.format);
        return ExitCode::SUCCESS;
    };
    let shown = if redact { Redactor::from_env().text(&target.display().to_string()) } else { target.display().to_string() };
    v["export"] = json!({"path": shown, "written": false});
    // Preview first; machine callers never write the file (LIF-04).
    if p.format == Format::Json || p.non_interactive() {
        emit(&v, p.format);
        return ExitCode::SUCCESS;
    }
    if target.exists() {
        return fail(req, ErrorCode::Conflict, &format!("{shown} already exists; choose a new file"), p.format);
    }
    let body = serde_json::to_string_pretty(&v).expect("report serializes");
    println!("{} {shown}:\n\n{body}\n", t("preview.doctor"));
    match crate::plan_cmd::prompt(t("prompt.doctor")) {
        Some(a) if a == "export" => {}
        _ => return fail(req, ErrorCode::Cancelled, "not exported; no file was written", p.format),
    }
    v["export"]["written"] = json!(true);
    let text = serde_json::to_string_pretty(&v).expect("report serializes") + "\n";
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = written {
        return fail(req, ErrorCode::IoError, &format!("cannot write {shown}: {e}"), p.format);
    }
    emit(&v, p.format);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_is_case_insensitive_and_home_first() {
        let r = Redactor { pairs: vec![("C:\\Users\\Alice".into(), "~"), ("alice".into(), "<user>")] };
        assert_eq!(r.text("c:\\users\\ALICE\\AppData and Alice"), "~\\AppData and <user>");
        let v = r.value(&json!({"a": ["x ALICE y"], "n": 1}));
        assert_eq!(v, json!({"a": ["x <user> y"], "n": 1}));
    }
}
