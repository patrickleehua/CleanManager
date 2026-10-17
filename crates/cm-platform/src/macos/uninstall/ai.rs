//! UNI-07 on macOS: npm-global AI command-line tools (Claude Code, Codex CLI, Gemini CLI; the shared
//! `cm_core::uninstall::AI_CLIS` table).
//!
//! - Records: `<prefix>/lib/node_modules/<package>/package.json` under the npm global prefixes this user's Node.js
//!   installations use: Homebrew (`/opt/homebrew`, `/usr/local`, also where the nodejs.org installer puts Node), every
//!   nvm version (`~/.nvm/versions/node/*`), npm's documented user prefix `~/.npm-global` and the `prefix=` of
//!   `~/.npmrc`. Nothing is run to find them (no `npm prefix -g` while listing). A linked package (`npm link`) is a
//!   developer checkout, not an installation, and is not listed. Volta keeps its own package store and is used only as a
//!   Node.js provider; packages Volta installed are not listed.
//! - Route: `node <npm-cli.js> uninstall --global --prefix <prefix> --ignore-scripts <package>` (see
//!   `cm_core::uninstall::mac::select`), through the trusted tool runner with the user-owned policy Homebrew's `brew`
//!   uses: `node` and every folder above it, and `npm-cli.js` and every folder above it, must be owned by root or this
//!   user and writable by nobody else (group write only for admin/wheel). The Node.js is the prefix's own, else the
//!   first trusted one of Homebrew, `/usr/local`, nvm (newest first) and Volta. The environment is cleared and rebuilt
//!   with `HOME`, `USER`/`LOGNAME` and npm's notifier, fund and audit turned off; time is bounded.
//! - Running (UNI-02): Node runs these tools, so the executable says little: a process of this user is running the
//!   tool when its executable lies inside the package (Codex ships native binaries) or one of its first arguments
//!   resolves into the package (`node /opt/homebrew/bin/claude` → `…/node_modules/@anthropic-ai/claude-code/cli.js`),
//!   read with `sysctl(KERN_PROCARGS2)`.
//! - Leftovers: the entries of the tool's data folder (`~/.claude`, `~/.codex`, `~/.gemini`) and its home-folder
//!   settings files; decisions in `cm_core::uninstall::mac::ai_leftovers`. Sessions, history, settings and credentials
//!   are never preselected; git worktrees are blocked; another remaining installation of the tool keeps everything.
//!
//! Tests: while `CM_UNINSTALL_TEST_ROOT`/`HOME` redirect the environment, every prefix is under the fake root or home
//! (a fixture `bin/node` script there stands in for Node.js) and the real `~/.npmrc` and `NPM_CONFIG_PREFIX` are ignored.

use std::os::unix::fs::PermissionsExt;

use cm_core::uninstall::mac::{AiDataEntry, ai_leftovers, ai_session_name};
use cm_core::uninstall::{AI_CLIS, AiCli, LeftoverKind, tool_instance_id};

use super::*;

const MANIFEST_MAX: u64 = 1 << 20;
const NPMRC_MAX: u64 = 256 << 10;
/// Largest Node.js binary that is hashed for the plan binding.
pub(super) const NODE_MAX: u64 = 512 << 20;
/// Entries a session / worktree probe looks at below one data-folder entry.
const PROBE_MAX: usize = 5_000;

fn home(env: &UninstallEnv) -> Option<&Path> {
    env.apps.home.as_deref()
}

/// `prefix=` of `~/.npmrc` (`~/` expanded; while redirected only a `~/` prefix inside the test home is accepted).
fn npmrc_prefix(env: &UninstallEnv) -> Option<PathBuf> {
    let rc = home(env)?.join(".npmrc");
    let text = std::fs::symlink_metadata(&rc).ok().filter(|m| m.is_file() && m.len() <= NPMRC_MAX).and_then(|_| std::fs::read_to_string(&rc).ok())?;
    let value = text.lines().map(str::trim).filter(|l| !l.starts_with(['#', ';'])).find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "prefix").then(|| v.trim().trim_matches('"').to_owned())
    })?;
    if let Some(rest) = value.strip_prefix("~/") {
        return Some(home(env)?.join(rest));
    }
    (!env.is_redirected() && value.starts_with('/')).then(|| PathBuf::from(value))
}

/// nvm's Node.js versions, newest first (by version number).
fn nvm_versions(env: &UninstallEnv) -> Vec<PathBuf> {
    let Some(h) = home(env) else { return vec![] };
    let mut v: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(h.join(".nvm/versions/node"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let key = e.file_name().to_string_lossy().trim_start_matches('v').split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect();
            (key, e.path())
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// The npm global prefixes looked at, in order, each once (existing folders only).
pub(super) fn prefixes(env: &UninstallEnv) -> Vec<PathBuf> {
    let mut out = vec![env.system("/opt/homebrew"), env.system("/usr/local")];
    out.extend(nvm_versions(env));
    if let Some(h) = home(env) {
        out.push(h.join(".npm-global"));
    }
    out.extend(npmrc_prefix(env));
    if !env.is_redirected() {
        out.extend(["NPM_CONFIG_PREFIX", "npm_config_prefix"].iter().filter_map(std::env::var_os).map(PathBuf::from).filter(|p| p.is_absolute()));
    }
    let mut seen = std::collections::BTreeSet::new();
    out.into_iter().filter(|p| p.is_dir()).filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))).collect()
}

fn package_dir(prefix: &Path, package: &str) -> PathBuf {
    package.split('/').fold(prefix.join("lib/node_modules"), |p, c| p.join(c))
}

fn manifest_version(manifest: &Path) -> Option<String> {
    let bytes = std::fs::symlink_metadata(manifest).ok().filter(|m| m.is_file() && m.len() <= MANIFEST_MAX).and_then(|_| std::fs::read(manifest).ok())?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(str::to_owned)
}

/// The npm-global AI CLI records of every prefix (one record per prefix and package).
pub(super) fn records(env: &UninstallEnv, out: &mut Vec<UninstallRecord>) {
    for prefix in prefixes(env) {
        for t in AI_CLIS {
            let dir = package_dir(&prefix, t.npm_package);
            // A linked package (`npm link`) is a developer checkout, not an installation.
            if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) || !std::fs::symlink_metadata(dir.join("package.json")).is_ok_and(|m| m.is_file()) {
                continue;
            }
            let prefix_text = prefix.to_string_lossy().into_owned();
            out.push(UninstallRecord {
                instance_id: tool_instance_id(RecordSource::NpmGlobal, &format!("{}\0{prefix_text}", t.npm_package)),
                name: t.name.into(),
                version: manifest_version(&dir.join("package.json")),
                publisher: None,
                source: RecordSource::NpmGlobal,
                key: None,
                key_name: None,
                product_code: None,
                package_full_name: None,
                package_family: None,
                uninstall_string: None,
                quiet_uninstall_string: None,
                install_location: Some(dir.to_string_lossy().into_owned()),
                system_component: false,
                winget: None,
                package_name: Some(t.npm_package.into()),
                ai_tool: Some(t.id.into()),
                protected_class: None,
                mac: Some(MacRecord {
                    bundle_id: None,
                    team_id: None,
                    homebrew_kind: None,
                    volume_id: None,
                    file_id: None,
                    admin_reason: None,
                    pkg_receipts: vec![],
                    npm_prefix: Some(prefix_text),
                }),
            });
        }
    }
}

/// A file the tool runner may hand to `node` (`npm-cli.js`): a regular file that it and every folder above it are
/// owned by root or this user and writable by nobody else (group write only for admin/wheel). `Ok` is the resolved path.
pub(super) fn trusted_script(path: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("{} cannot be resolved: {e}", path.display()))?;
    let me = euid();
    let check = |p: &Path| -> Result<std::fs::Metadata, String> {
        let m = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        if m.uid() != 0 && m.uid() != me {
            return Err(format!("{} is owned by uid {}, not root or the current user (uid {me})", p.display(), m.uid()));
        }
        let mode = m.permissions().mode();
        if mode & 0o002 != 0 || (mode & 0o020 != 0 && !PRIVILEGED_GROUPS.contains(&m.gid())) {
            return Err(format!("{} is writable by others (mode {:o})", p.display(), mode & 0o7777));
        }
        Ok(m)
    };
    if !check(&real)?.is_file() {
        return Err(format!("{} is not a regular file", real.display()));
    }
    for dir in real.ancestors().skip(1) {
        check(dir)?;
    }
    Ok(real)
}

/// `(node, npm-cli.js)` of one Node.js installation folder, when both pass the trust policy.
fn node_in(dir: &Path) -> Option<(String, String)> {
    let (node, npm) = (dir.join("bin/node"), dir.join("lib/node_modules/npm/bin/npm-cli.js"));
    crate::macos::cmd::trusted_user_program(&node, euid()).ok()?;
    trusted_script(&npm).ok()?;
    Some((node.to_string_lossy().into_owned(), npm.to_string_lossy().into_owned()))
}

/// The trusted Node.js that removes a package of `prefix`: the prefix's own, else Homebrew, `/usr/local`, nvm (newest
/// first) or Volta.
pub(super) fn node_for(env: &UninstallEnv, prefix: Option<&str>) -> Option<(String, String)> {
    if let Some(found) = prefix.and_then(|p| node_in(Path::new(p))) {
        return Some(found);
    }
    let mut dirs = vec![env.system("/opt/homebrew"), env.system("/usr/local")];
    dirs.extend(nvm_versions(env));
    if let Some(h) = home(env) {
        let mut volta: Vec<PathBuf> = std::fs::read_dir(h.join(".volta/tools/image/node")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        volta.sort();
        volta.reverse();
        dirs.extend(volta);
    }
    dirs.iter().find_map(|d| node_in(d))
}

/// UNI-07: when the tool was last used, from the newest entry of its data folder (Unix seconds); `None` = unknown.
pub fn last_used(env: &UninstallEnv, tool_id: &str) -> Option<i64> {
    let tool = cm_core::uninstall::ai_cli(tool_id)?;
    let dir = home(env)?.join(tool.home_dir);
    std::iter::once(dir.clone())
        .chain(std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()))
        .filter_map(|p| p.symlink_metadata().ok()?.modified().ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// Whether below `dir` (links not followed, `depth` levels, bounded) an entry satisfies `hit`.
fn probe(dir: &Path, depth: usize, hit: &dyn Fn(&str) -> bool) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, level)) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            seen += 1;
            if seen > PROBE_MAX {
                // Too large to rule it out: treat it as a hit (never preselected, or blocked).
                return true;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if hit(&name) {
                return true;
            }
            if level + 1 < depth && e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((e.path(), level + 1));
            }
        }
    }
    false
}

/// UNI-07: the data-folder leftovers of one removed AI CLI (`installed`: the instances still installed).
pub(super) fn leftovers(env: &UninstallEnv, identity: &AppIdentity, tool: &AiCli, installed: &[MacInstalled], sizes: bool) -> Vec<Leftover> {
    let Some(h) = home(env) else { return vec![] };
    let dir = h.join(tool.home_dir);
    let mut entries = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = e.path();
        let is_dir = ft.is_dir();
        entries.push(AiDataEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            is_dir,
            logical_bytes: if sizes { logical_size(&path) } else { None },
            worktree: is_dir && (path.join(".git").exists() || probe(&path, 3, &|n| n == ".git")),
            sessions_inside: is_dir && probe(&path, 3, &ai_session_name),
        });
    }
    let home_files: Vec<(String, u64)> = tool
        .home_files
        .iter()
        .filter_map(|n| {
            let p = h.join(n);
            let m = p.symlink_metadata().ok().filter(|m| m.is_file())?;
            Some((p.to_string_lossy().into_owned(), m.len()))
        })
        .collect();
    let still = installed.iter().find(|o| o.instance_id != identity.instance_id && o.name == tool.name);
    let mut out = ai_leftovers(tool, &format!("~/{}", tool.home_dir), &entries, &home_files, still);
    // The package folder, if the package manager left it (never preselected).
    if let Some(pkg) = identity.install_location.as_deref().filter(|p| Path::new(p).is_dir()) {
        out.push(cm_core::uninstall::finish_leftover(Leftover {
            object_id: cm_core::uninstall::object_id(LeftoverKind::Directory, pkg), kind: LeftoverKind::Directory, group: cm_core::uninstall::Group::Bundle,
            display: pkg.to_owned(), evidence: vec![format!("installed package folder of {}", tool.name)], marker: None, tier: String::new(), selected: false,
            warnings: vec![], logical_bytes: if sizes { logical_size(Path::new(pkg)) } else { None }, note: None, requires_admin: false,
            binding: Binding::Path { path: pkg.to_owned() },
        }));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Running (UNI-02)

/// A buffer of `KERN_ARGMAX` bytes for [`process_args`] (`None` when the limit cannot be read).
fn args_buffer() -> Option<Vec<u8>> {
    let mut argmax: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: two-level MIB; `argmax` is a writable c_int and `len` its size; no new value is set.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut len, std::ptr::null_mut(), 0) } != 0 || argmax <= 0 {
        return None;
    }
    Some(vec![0u8; argmax as usize])
}

/// `argv` of a process of this user (`sysctl(KERN_PROCARGS2)`) read into `buf`; `None` when unreadable or gone.
fn process_args(pid: i32, buf: &mut [u8]) -> Option<Vec<String>> {
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: three-level MIB; `buf` is writable for `size` bytes and `size` is updated to what was written.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    let buf = &buf[..size.min(buf.len())];
    // Layout: argc (int), the executable path, NUL padding, then argc NUL-terminated arguments.
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let mut rest = &buf[4..];
    let end = rest.iter().position(|&b| b == 0)?;
    rest = &rest[end..];
    let start = rest.iter().position(|&b| b != 0)?;
    rest = &rest[start..];
    let args: Vec<String> = rest.split(|&b| b == 0).take(usize::try_from(argc).ok()?).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    Some(args)
}

/// Processes of this user running the package at `dir` (this process excluded).
pub(super) fn running(dir: &str) -> RunningState {
    let Ok(real) = std::fs::canonicalize(dir) else { return RunningState::Known { processes: vec![] } };
    let inside = |p: &str| {
        let p = Path::new(p);
        p.starts_with(dir) || std::fs::canonicalize(p).is_ok_and(|r| r.starts_with(&real))
    };
    let (me, uid) = (std::process::id() as i32, euid());
    let mut buf = args_buffer();
    let snap = match crate::macos::procs::snapshot() {
        Ok(s) => s,
        Err(e) => return RunningState::Unknown { reason: format!("the process list cannot be read: {}", describe(e)) },
    };
    let processes = snap
        .procs
        .into_iter()
        .filter(|p| p.pid != me && !p.zombie && p.uid == uid)
        .filter_map(|p| {
            let exe = p.path.as_deref().map(|b| String::from_utf8_lossy(b).into_owned());
            let runs = exe.as_deref().is_some_and(inside)
                || buf.as_deref_mut().and_then(|b| process_args(p.pid, b)).is_some_and(|args| args.iter().skip(1).take(8).any(|a| a.starts_with('/') && inside(a)));
            runs.then_some(RunningProcess { pid: p.pid as u32, name: p.name, path: exe })
        })
        .collect();
    RunningState::Known { processes }
}

// ---------------------------------------------------------------------------------------------
// The npm run

/// Run the planned `node npm-cli.js uninstall …` through the trusted runner (user-owned policy, cleared environment).
pub(super) fn run(env: &UninstallEnv, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String> {
    let npm = choice.argv.first().ok_or("the npm program is missing from the plan")?;
    trusted_script(Path::new(npm))?;
    let started = Instant::now();
    let mut t = crate::macos::cmd::Tool::new(&choice.program)
        .args(&choice.argv)
        .owned_by_user_or_root()
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("npm_config_update_notifier", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_audit", "false")
        .timeout(Duration::from_secs(u64::from(timeout_seconds)))
        .output_caps(1 << 20, 256 << 10);
    if let Some(h) = home(env) {
        t = t.env("HOME", h);
    }
    for k in ["USER", "LOGNAME"] {
        if let Some(v) = std::env::var_os(k).filter(|v| !v.is_empty()) {
            t = t.env(k, v);
        }
    }
    let out = t.run().map_err(|e| e.to_string())?;
    let ms = started.elapsed().as_millis() as u64;
    Ok((if out.timed_out { None } else { out.exit_code }, ms))
}
