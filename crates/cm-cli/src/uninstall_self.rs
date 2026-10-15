//! `self uninstall` (LIF-02, RES-07; add-mole-parity-suite 17.2): a reviewed plan that removes
//! CleanManager's own footprint and nothing else.
//!
//! What is removed by default:
//! - the program files listed in the release manifest whose SHA-256 still matches (a file changed
//!   since installation is kept and reported), the manifest/install records, update backups and
//!   staging leftovers, then the directories they leave empty;
//! - caches: the rebuildable scan index and last-run previews in the store directory;
//! - system changes CleanManager made: the user PATH entry `self install --add-to-path` recorded,
//!   and shell-profile completion blocks that load *this* copy (`completion install`).
//!
//! What is kept by default: configuration, the protection list, operation history (with registry
//! and startup backups) and the quarantine. A non-empty quarantine is reported with its item count
//! and size and the restore/purge commands; uninstalling never deletes quarantined objects.
//! `--remove-user-data` also removes configuration, protection list and history — only when the
//! quarantine is empty and no operation awaits reconciliation.
//!
//! Resident components: this build installs no service, scheduled task, startup entry, helper or
//! background process, and changes no other system setting, so there is nothing else to remove or
//! restore; the plan says so instead of searching for look-alikes it did not create.
//!
//! Package-manager installs are not removed here: the plan names the package manager's command.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::install_self::{self, UserPathStore};
use crate::release::{self, INSTALL_STATE_FILE, InstallState, RELEASE_FILE, ReleaseManifest, display, safe_relative, same_path};
use crate::update_self::{BACKUP_DIR, Detected, STAGING_PREFIX};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub item_id: String,
    /// `cli`, `caches`, `system_setting`, `user_data`.
    pub component: &'static str,
    /// `remove_file`, `remove_empty_dir`, `remove_path_entry`, `remove_profile_block`, `remove_credential`.
    pub action: &'static str,
    pub path: PathBuf,
    pub display: String,
    /// Files: the content that may be removed; anything else at that path is kept.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// `configuration`, `protection_list`, `history`, `quarantine`, `registry_backups`.
    pub data: &'static str,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<Item>,
    pub keep: Vec<Kept>,
    pub quarantine_items: u64,
    pub quarantine_bytes: u64,
    pub warnings: Vec<Warning>,
    pub notes: Vec<String>,
    pub digest: String,
}

pub struct Inputs<'a> {
    pub install_dir: &'a Path,
    pub store_dir: Option<&'a Path>,
    pub detected: &'a Detected,
    pub remove_user_data: bool,
    /// Shell profiles that may hold a completion block: (shell, profile path).
    pub profiles: Vec<(&'static str, PathBuf)>,
    pub user_path: Option<&'a dyn UserPathStore>,
    /// AI diagnosis provider ids whose keys are in the system credential store.
    pub credential_targets: Vec<String>,
}

const CACHE_FILES: &[&str] = &["index.db", "index.db-wal", "index.db-shm", "installer-last-run.json", "purge-last-run.json"];
const CONFIG_FILES: &[&str] = &["purge-settings.json", "app-updates.json", "status-alerts.json", "process-pins.json", "diag-providers.json"];
const PROTECTION_FILES: &[&str] = &["protection.json", "protection.json.lock"];
const HISTORY_FILES: &[&str] = &["audit.db", "audit.db-wal", "audit.db-shm", cm_store::migrate::LOG_FILE];

fn classify(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if CACHE_FILES.contains(&lower.as_str()) {
        Some("caches")
    } else if CONFIG_FILES.contains(&lower.as_str()) {
        Some("configuration")
    } else if PROTECTION_FILES.contains(&lower.as_str()) {
        Some("protection_list")
    } else if HISTORY_FILES.contains(&lower.as_str()) || (lower.starts_with("audit.db.pre-v") && lower.ends_with(".bak")) {
        Some("history")
    } else {
        None
    }
}

/// Count and size of objects still held in quarantine, and operations awaiting reconciliation.
fn store_facts(store: &Path) -> Result<(u64, u64, u64, u64), String> {
    if !store.join(cm_store::AUDIT_FILE).is_file() {
        return Ok((0, 0, 0, 0));
    }
    let audit = cm_store::AuditStore::open(&store.join(cm_store::AUDIT_FILE)).map_err(|e| e.to_string())?;
    let (mut items, mut bytes, mut cursor) = (0u64, 0u64, None::<String>);
    loop {
        let (rows, next) = audit.quarantine_page(200, cursor.as_deref()).map_err(|e| e.to_string())?;
        for r in &rows {
            items += 1;
            let v: Value = serde_json::from_str(&r.record_json).unwrap_or(Value::Null);
            bytes += v["logical_bytes"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        }
        match next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    let mut pending = 0u64;
    let mut cursor = None::<String>;
    loop {
        let (ops, next) = audit.operations_page(200, cursor.as_deref()).map_err(|e| e.to_string())?;
        pending += ops.iter().filter(|o| matches!(o.state.as_str(), "reconciliation_required" | "running" | "validating" | "stopping" | "created")).count() as u64;
        match next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    let backups = audit.retained_backup_count().map_err(|e| e.to_string())?;
    Ok((items, bytes, pending, backups))
}

/// The completion block of `profile` if it loads the program in `install_dir`.
pub fn completion_block(text: &str, install_dir: &Path) -> Option<(usize, usize)> {
    let start = text.find(crate::completion_cmd::BEGIN)?;
    let end_rel = text[start..].find(crate::completion_cmd::END)?;
    let mut end = start + end_rel + crate::completion_cmd::END.len();
    if text[end..].starts_with("\r\n") {
        end += 2;
    } else if text[end..].starts_with('\n') {
        end += 1;
    }
    let exe = install_dir.join(release::exe_name()).display().to_string();
    let block = &text[start..end];
    let refers = if cfg!(windows) { block.to_lowercase().contains(&exe.to_lowercase()) } else { block.contains(&exe) };
    refers.then_some((start, end))
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                files_below(&p, out, dirs);
                dirs.push(p);
            }
            Ok(_) => out.push(p),
            Err(_) => {}
        }
    }
}

/// Build the plan. Reads only; nothing is changed.
pub fn plan(inp: &Inputs<'_>) -> Plan {
    let mut items: Vec<Item> = Vec::new();
    let mut keep = Vec::new();
    let mut warnings = Vec::new();
    let mut notes = vec!["This build installs no service, scheduled task, startup entry, helper or background process and changes no other system setting; there is nothing else to remove or restore.".to_owned()];
    let mut push = |component: &'static str, action: &'static str, path: PathBuf, display: String, sha256: Option<String>| {
        let item_id = format!("i{:03}", items.len() + 1);
        items.push(Item { item_id, component, action, path, display, sha256 });
    };
    let dir = inp.install_dir;
    let state = InstallState::load(dir);
    // Program files.
    if inp.detected.method.managed() {
        warnings.push(Warning {
            code: "package_manager",
            message: format!(
                "this copy is managed by {}; remove the program with `{}` (CleanManager does not delete files a package manager owns)",
                inp.detected.method.as_str(),
                inp.detected.uninstall_command.clone().unwrap_or_default()
            ),
        });
    } else {
        match ReleaseManifest::load(dir) {
            Ok(Some(m)) => {
                let mut dirs: Vec<PathBuf> = Vec::new();
                for f in &m.files {
                    let Ok(rel) = safe_relative(&f.path) else { continue };
                    let p = dir.join(&rel);
                    match release::sha256_file(&p) {
                        Ok((h, _)) if h == f.sha256 => push("cli", "remove_file", p.clone(), display(&p), Some(h)),
                        Ok(_) => warnings.push(Warning { code: "modified_since_install", message: format!("{} changed since it was installed and is kept", display(&p)) }),
                        Err(_) => {}
                    }
                    dirs.extend(p.ancestors().skip(1).take_while(|a| *a != dir).map(Path::to_path_buf));
                }
                for name in [RELEASE_FILE, INSTALL_STATE_FILE, crate::instance_lock::LOCK_FILE] {
                    let p = dir.join(name);
                    if let Ok((h, _)) = release::sha256_file(&p) {
                        push("cli", "remove_file", p.clone(), display(&p), Some(h));
                    }
                }
                // Update backups and staging leftovers: reserved names only this program creates.
                for e in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name == BACKUP_DIR || name.starts_with(STAGING_PREFIX) || name.contains(".removing-") {
                        let mut files = Vec::new();
                        let mut sub = Vec::new();
                        if e.path().is_dir() {
                            files_below(&e.path(), &mut files, &mut sub);
                            dirs.extend(sub);
                            dirs.push(e.path());
                        } else {
                            files.push(e.path());
                        }
                        for f in files {
                            if let Ok((h, _)) = release::sha256_file(&f) {
                                push("cli", "remove_file", f.clone(), display(&f), Some(h));
                            }
                        }
                    }
                }
                dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()).then_with(|| a.cmp(b)));
                dirs.dedup();
                for d in dirs {
                    push("cli", "remove_empty_dir", d.clone(), display(&d), None);
                }
                push("cli", "remove_empty_dir", dir.to_path_buf(), display(dir), None);
            }
            Ok(None) => warnings.push(Warning {
                code: "no_release_manifest",
                message: format!("{} has no {RELEASE_FILE}: this copy was not installed from a CleanManager release, so its program files are not removed (delete them yourself or with the tool that placed them)", display(dir)),
            }),
            Err(e) => warnings.push(Warning { code: "no_release_manifest", message: e }),
        }
    }
    // System changes CleanManager made.
    if let (Some(entry), Some(store)) = (state.as_ref().and_then(|s| s.path_entry_added.clone()), inp.user_path)
        && install_self::path_contains(store, &entry).unwrap_or(false)
    {
        push("system_setting", "remove_path_entry", PathBuf::from(&entry), format!("user PATH entry {}", display(Path::new(&entry))), None);
    }
    for (shell, profile) in &inp.profiles {
        if let Ok(text) = std::fs::read_to_string(profile)
            && completion_block(&text, dir).is_some()
        {
            push("system_setting", "remove_profile_block", profile.clone(), format!("{shell} completion block in {}", display(profile)), None);
        }
    }
    // Store directory: caches go, user data stays unless asked and safe.
    let (mut q_items, mut q_bytes) = (0, 0);
    if let Some(store) = inp.store_dir.filter(|s| s.is_dir()) {
        let (qi, qb, pending, backups) = match store_facts(store) {
            Ok(f) => f,
            Err(e) => {
                warnings.push(Warning { code: "history_unreadable", message: format!("the operation history cannot be read ({e}); user data is kept") });
                (0, 0, 1, 0)
            }
        };
        (q_items, q_bytes) = (qi, qb);
        if qi > 0 {
            warnings.push(Warning {
                code: "quarantine_not_empty",
                message: format!(
                    "the quarantine still holds {qi} item(s), {qb} bytes; restore them (`cleanmanager recovery list`, then `cleanmanager recovery plan create --restore ID`), purge them (`--purge ID`), or keep them: uninstalling never deletes quarantined objects"
                ),
            });
        }
        if pending > 0 {
            warnings.push(Warning { code: "operations_pending", message: format!("{pending} operation(s) are unfinished or await reconciliation; their history is kept") });
        }
        let remove_data = inp.remove_user_data && qi == 0 && pending == 0;
        if inp.remove_user_data && !remove_data {
            warnings.push(Warning { code: "user_data_kept", message: "--remove-user-data was not applied: empty the quarantine and reconcile pending operations first".into() });
        }
        let mut names: Vec<(String, PathBuf)> = std::fs::read_dir(store).into_iter().flatten().filter_map(Result::ok).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
        names.sort();
        let mut kept_kinds: Vec<&'static str> = Vec::new();
        let mut removes_all = true;
        for (name, path) in &names {
            // The lifecycle lock is this program's own and is held while the plan runs; it never
            // appears in the plan (so the digest does not depend on it) and is removed with its directory.
            if name == crate::instance_lock::LOCK_FILE {
                continue;
            }
            match classify(name) {
                Some("caches") if path.is_file() => push("caches", "remove_file", path.clone(), display(path), release::sha256_file(path).ok().map(|(h, _)| h)),
                Some(_) if remove_data && path.is_file() => push("user_data", "remove_file", path.clone(), display(path), release::sha256_file(path).ok().map(|(h, _)| h)),
                Some(kind) => {
                    removes_all = false;
                    if !kept_kinds.contains(&kind) {
                        kept_kinds.push(kind);
                    }
                }
                None => {
                    removes_all = false;
                    if name == "quarantine" {
                        if !kept_kinds.contains(&"quarantine") {
                            kept_kinds.push("quarantine");
                        }
                    } else {
                        notes.push(format!("{} is not recognised as CleanManager data and is kept", display(path)));
                    }
                }
            }
        }
        if remove_data {
            for t in &inp.credential_targets {
                push("user_data", "remove_credential", PathBuf::from(t), format!("AI diagnosis key {t} in the system credential store"), None);
            }
            if removes_all {
                push("user_data", "remove_empty_dir", store.to_path_buf(), display(store), None);
            }
        } else if !inp.credential_targets.is_empty() && !kept_kinds.contains(&"configuration") {
            kept_kinds.push("configuration");
        }
        if qi > 0 && !kept_kinds.contains(&"quarantine") {
            kept_kinds.push("quarantine");
        }
        if backups > 0 && !remove_data {
            kept_kinds.push("registry_backups");
        }
        for kind in kept_kinds {
            let what = match kind {
                "configuration" => "settings (update checks, alerts, pins, purge roots, AI providers and their keys)",
                "protection_list" => "your protection list",
                "history" => "operation history, plans and authorizations",
                "quarantine" => "quarantined objects and their manifests",
                _ => "registry and startup backups needed to restore earlier removals",
            };
            keep.push(Kept { data: kind, display: format!("{what} in {}", display(store)) });
        }
    }
    // Order: system settings, caches and data first; program files (the running copy among them) last.
    let rank = |c: &str| match c {
        "system_setting" => 0,
        "caches" => 1,
        "user_data" => 2,
        _ => 3,
    };
    items.sort_by_key(|i| rank(i.component));
    for (n, i) in items.iter_mut().enumerate() {
        i.item_id = format!("i{:03}", n + 1);
    }
    let body = json!({
        "items": items.iter().map(|i| json!([i.item_id, i.component, i.action, i.path.to_string_lossy(), i.sha256])).collect::<Vec<_>>(),
        "keep": keep.iter().map(|k| k.data).collect::<Vec<_>>(),
        "quarantine": [q_items, q_bytes],
    });
    let digest = cm_core::c14n::digest(&body).expect("plan body is canonical JSON");
    Plan { items, keep, quarantine_items: q_items, quarantine_bytes: q_bytes, warnings, notes, digest }
}

/// Result of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub item_id: String,
    /// `removed`, `scheduled` (the running program, removed right after this process exits), `skipped`, `failed`.
    pub outcome: &'static str,
    pub reason: Option<String>,
}

pub struct Effects<'a> {
    pub user_path: Option<&'a dyn UserPathStore>,
    /// The running program (it cannot delete itself while running).
    pub current_exe: Option<PathBuf>,
    pub delete_credential: &'a dyn Fn(&str) -> Result<(), String>,
    /// Arrange removal of the running program and its then-empty directory after exit.
    pub defer: &'a dyn Fn(&Path, &Path) -> Result<(), String>,
}

/// Carry out a reviewed plan item by item. Files are removed only if they still have the planned
/// content; directories only if empty; profile blocks only if they still load this copy.
pub fn execute(p: &Plan, install_dir: &Path, fx: &Effects<'_>) -> Vec<Done> {
    let mut out = Vec::new();
    let mut deferred: Option<PathBuf> = None;
    for i in &p.items {
        let r: Result<&'static str, String> = match i.action {
            "remove_file" => match release::sha256_file(&i.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("already gone".into()),
                Err(e) => Err(e.to_string()),
                Ok((h, _)) if Some(&h) != i.sha256.as_ref() && i.sha256.is_some() => Err("changed since the plan was shown; kept".into()),
                Ok(_) => match std::fs::remove_file(&i.path) {
                    Ok(()) => Ok("removed"),
                    Err(e) if fx.current_exe.as_ref().is_some_and(|c| same_path(c, &i.path)) => {
                        // The running image can be renamed but not deleted.
                        let aside = i.path.with_file_name(format!("{}.removing-{}", release::exe_name(), std::process::id()));
                        match std::fs::rename(&i.path, &aside) {
                            Ok(()) => {
                                deferred = Some(aside);
                                Ok("scheduled")
                            }
                            Err(_) => Err(format!("in use: {e}")),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                },
            },
            "remove_empty_dir" => match remove_dir_with_lock(&i.path) {
                Ok(()) => Ok("removed"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("already gone".into()),
                Err(_) if deferred.as_ref().is_some_and(|d| d.parent() == Some(i.path.as_path())) => Ok("scheduled"),
                Err(_) => Err("not empty; kept".into()),
            },
            "remove_path_entry" => match fx.user_path {
                Some(store) => install_self::remove_entry(store, &i.path.display().to_string()).map(|changed| if changed { "removed" } else { "skipped" }),
                None => Err("no user PATH on this platform".into()),
            },
            "remove_profile_block" => remove_block(&i.path, install_dir).map(|()| "removed"),
            "remove_credential" => (fx.delete_credential)(&i.path.display().to_string()).map(|()| "removed"),
            other => Err(format!("unknown action {other}")),
        };
        out.push(match r {
            Ok(outcome) => Done { item_id: i.item_id.clone(), outcome, reason: None },
            Err(why) if why == "already gone" => Done { item_id: i.item_id.clone(), outcome: "skipped", reason: Some(why) },
            Err(why) if why.ends_with("kept") => Done { item_id: i.item_id.clone(), outcome: "skipped", reason: Some(why) },
            Err(why) => Done { item_id: i.item_id.clone(), outcome: "failed", reason: Some(why) },
        });
    }
    if let Some(file) = deferred
        && let Err(e) = (fx.defer)(&file, install_dir)
    {
        for d in out.iter_mut().filter(|d| d.outcome == "scheduled") {
            d.outcome = "failed";
            d.reason = Some(format!("the running program was renamed to {} but could not be scheduled for removal ({e}); delete it after this window closes", display(&file)));
        }
    }
    out
}

/// Remove an empty directory; a lone lifecycle lock file (ours) does not count as content.
fn remove_dir_with_lock(dir: &Path) -> std::io::Result<()> {
    let lock = dir.join(crate::instance_lock::LOCK_FILE);
    let only_lock = std::fs::read_dir(dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect::<Vec<_>>() == [lock.clone()]).unwrap_or(false);
    if only_lock {
        let _ = std::fs::remove_file(&lock);
    }
    std::fs::remove_dir(dir)
}

fn remove_block(profile: &Path, install_dir: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(profile).map_err(|e| e.to_string())?;
    let Some((s, e)) = completion_block(&text, install_dir) else { return Err("the completion block is no longer there; kept".into()) };
    let new = format!("{}{}", &text[..s], &text[e..]);
    let tmp = profile.with_extension("cleanmanager-tmp");
    std::fs::write(&tmp, new).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, profile).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Windows: a detached `cmd.exe` waits for this process to exit, then deletes the renamed program
/// and removes the install directory if it is empty. Paths with `%` (cmd expansion) are refused.
pub fn defer_removal(file: &Path, dir: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let (f, d) = (file.display().to_string(), dir.display().to_string());
        if [&f, &d].iter().any(|s| s.contains('%') || s.contains('"')) {
            return Err("the path contains characters cmd.exe would expand".into());
        }
        let script = format!("/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{f}\" & rmdir \"{d}\"");
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        std::process::Command::new(system.join("System32").join("cmd.exe"))
            .raw_arg(script)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        // A running program may unlink itself on Unix.
        let _ = dir;
        std::fs::remove_file(file).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
