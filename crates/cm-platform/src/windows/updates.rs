//! Windows update channels and the verified update backend (parity 10.1-10.3, UPD-01..04).
//!
//! - winget: the App Installer package's own `winget.exe` (resolved from the installed package, else the per-user
//!   execution alias), run without a shell, no console window, a time limit and bounded output. Detection runs only
//!   `winget list`; the package manager contacts its configured sources and nothing else is sent.
//! - Scoop: read from the local Scoop directory (`%SCOOP%` or `~\scoop`): installed `manifest.json`/`install.json` and
//!   the bucket's manifest. No network; the result says how old the local bucket copy is. Scoop steps run
//!   `scoop.ps1` through Windows PowerShell with `-File` and fixed arguments.
//! - Self-updaters: Squirrel (`Update.exe` beside `app-*` folders) and electron-updater (`resources\app-update.yml`)
//!   are reported as detected-but-unverifiable; nothing reads or contacts their feeds.
//! - Installed signer: the verified signer of the app's top-level executables (uninstallers and updaters skipped).

use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::apps::{AppInstance, AppSource};
use cm_core::path::RawPath;
use cm_core::updates::{Channel, ScoopApp, SelfUpdaterProbe, ToolOutput, UpdateBackend, UpdateTarget, scoop_install_bucket, scoop_manifest_homepage, scoop_manifest_version};

use super::processes::{file_sha256, running_under, signature_of};

/// `Microsoft.DesktopAppInstaller` package family (provides winget).
pub const APP_INSTALLER_FAMILY: &str = "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe";

fn raw(path: &str) -> Option<RawPath> {
    use std::os::windows::ffi::OsStrExt;
    RawPath::windows(std::ffi::OsStr::new(path).encode_wide().collect()).ok()
}

/// The real `winget.exe` inside the installed App Installer package, else the execution alias.
pub fn winget_exe(apps: &[AppInstance]) -> Option<PathBuf> {
    let packaged = apps.iter().find_map(|a| match &a.source {
        AppSource::Msix { package_family, .. } if package_family.eq_ignore_ascii_case(APP_INSTALLER_FAMILY) => {
            a.install_location.as_ref().map(|l| PathBuf::from(l.display()).join("winget.exe"))
        }
        _ => None,
    });
    packaged.filter(|p| p.is_file()).or_else(|| {
        let alias = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps\winget.exe");
        alias.exists().then_some(alias)
    })
}

fn scoop_root() -> Option<PathBuf> {
    std::env::var_os("SCOOP").map(PathBuf::from).or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("scoop"))).filter(|p| p.is_dir())
}

/// `scoop.ps1` of the local Scoop install.
pub fn scoop_exe() -> Option<PathBuf> {
    let p = scoop_root()?.join(r"apps\scoop\current\bin\scoop.ps1");
    p.is_file().then_some(p)
}

fn modified_utc(p: &Path) -> Option<String> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(cm_core::time::utc_from_unix(secs as i64))
}

/// Scoop apps below `root` (testable with a fixture directory).
pub fn scoop_apps_in(root: &Path) -> Vec<ScoopApp> {
    let Ok(dir) = std::fs::read_dir(root.join("apps")) else { return Vec::new() };
    let mut out = Vec::new();
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "scoop" {
            continue;
        }
        let current = e.path().join("current");
        let Some(installed_version) = std::fs::read_to_string(current.join("manifest.json")).ok().and_then(|t| scoop_manifest_version(&t)) else { continue };
        let homepage_local = std::fs::read_to_string(current.join("manifest.json")).ok().and_then(|t| scoop_manifest_homepage(&t));
        let bucket = std::fs::read_to_string(current.join("install.json")).ok().and_then(|t| scoop_install_bucket(&t));
        let (bucket_version, bucket_updated_at, homepage) = match &bucket {
            Some(b) => {
                let base = root.join("buckets").join(b);
                let manifest = [base.join("bucket").join(format!("{name}.json")), base.join(format!("{name}.json"))].into_iter().find(|p| p.is_file());
                let text = manifest.and_then(|m| std::fs::read_to_string(m).ok());
                let git = base.join(".git");
                let updated = ["FETCH_HEAD", "ORIG_HEAD", "HEAD"].iter().find_map(|f| modified_utc(&git.join(f)));
                (text.as_deref().and_then(scoop_manifest_version), updated, text.as_deref().and_then(scoop_manifest_homepage).or(homepage_local))
            }
            None => (None, None, homepage_local),
        };
        out.push(ScoopApp { name, installed_version, bucket, bucket_version, bucket_updated_at, homepage, install_dir: e.path().to_string_lossy().into_owned() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Inventory records for Scoop apps that have no installation record of their own (Scoop does not register them).
pub fn scoop_instances(scoop: &[ScoopApp], apps: &[AppInstance]) -> Vec<AppInstance> {
    scoop
        .iter()
        .filter(|s| {
            let d = s.install_dir.to_lowercase();
            !apps.iter().any(|a| a.install_location.as_ref().is_some_and(|l| l.display().to_lowercase().starts_with(&d)))
        })
        .map(|s| {
            let mut a = AppInstance::new(s.name.clone(), AppSource::Unknown, raw(&format!("{}\\current", s.install_dir)));
            a.version = Some(s.installed_version.clone());
            a.evidence.push(cm_core::apps::Evidence::new(cm_core::apps::EvidenceKind::InstallLocation, "Scoop app directory"));
            a
        })
        .collect()
}

/// Squirrel / electron-updater detection from the install location (read-only file checks).
pub fn self_updater(app: &AppInstance) -> Option<SelfUpdaterProbe> {
    let loc = PathBuf::from(app.install_location.as_ref()?.display());
    if loc.join("Update.exe").is_file() {
        let version_dir = std::fs::read_dir(&loc).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("app-"));
        if let Some(v) = version_dir {
            return Some(SelfUpdaterProbe { evidence: format!("Squirrel: Update.exe and {v}") });
        }
    }
    loc.join(r"resources\app-update.yml").is_file().then(|| SelfUpdaterProbe { evidence: "electron-updater: resources\\app-update.yml".into() })
}

const SKIP_EXE: &[&str] = &["unins", "uninstall", "update.exe", "updater", "crashpad", "crashreporter", "helper"];

/// Verified signer of the app's top-level executables (`None` = none verifies).
pub fn installed_signer(app: &AppInstance) -> Option<String> {
    let loc = PathBuf::from(app.install_location.as_ref()?.display());
    let mut exes: Vec<PathBuf> = std::fs::read_dir(&loc)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            !SKIP_EXE.iter().any(|s| n.starts_with(s) || n == *s || n.contains(s))
        })
        .collect();
    exes.sort();
    exes.iter().take(8).find_map(|p| {
        let s = signature_of(&p.to_string_lossy());
        matches!(s.state, cm_core::process::SignatureState::Signed | cm_core::process::SignatureState::CatalogSigned).then_some(s.publisher).flatten()
    })
}

fn run_tool(exe: &str, argv: &[String], timeout: Duration) -> Result<ToolOutput, String> {
    use std::ffi::OsString;
    let cwd = std::env::temp_dir();
    let (program, args): (PathBuf, Vec<OsString>) = if exe.to_lowercase().ends_with(".ps1") {
        let ps = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let mut a: Vec<OsString> = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", exe].iter().map(OsString::from).collect();
        a.extend(argv.iter().map(OsString::from));
        (ps, a)
    } else {
        (PathBuf::from(exe), argv.iter().map(OsString::from).collect())
    };
    match super::adapters::run_bounded(&program, &args, &cwd, timeout, 256 * 1024) {
        Ok(r) => {
            let mut output = String::from_utf8_lossy(&r.stdout).into_owned();
            output.push_str(&String::from_utf8_lossy(&r.stderr));
            Ok(ToolOutput { exit_code: r.exit, output, timed_out: false })
        }
        Err(super::adapters::RunError::Timeout { output, .. }) => Ok(ToolOutput { exit_code: None, output: String::from_utf8_lossy(&output).into_owned(), timed_out: true }),
        Err(super::adapters::RunError::Spawn(e)) => Err(e),
    }
}

/// `winget list` for detection (the only network use of a check).
pub fn winget_list(exe: &Path) -> Result<String, String> {
    let argv: Vec<String> = ["list", "--disable-interactivity", "--accept-source-agreements"].map(str::to_owned).to_vec();
    let out = run_tool(&exe.to_string_lossy(), &argv, Duration::from_secs(180))?;
    if out.timed_out {
        return Err("winget did not answer within 180 s".into());
    }
    match out.exit_code {
        Some(0) => Ok(out.output),
        other => Err(format!("winget list exited with {other:?}")),
    }
}

/// The Windows update backend for `update` plans.
pub struct WindowsUpdates;

impl UpdateBackend for WindowsUpdates {
    fn run(&self, exe: &str, argv: &[String], timeout_seconds: u32) -> Result<ToolOutput, String> {
        run_tool(exe, argv, Duration::from_secs(u64::from(timeout_seconds)))
    }

    fn file_sha256(&self, path: &str) -> Option<String> {
        file_sha256(path)
    }

    fn signer(&self, path: &str) -> Option<String> {
        let s = signature_of(path);
        matches!(s.state, cm_core::process::SignatureState::Signed | cm_core::process::SignatureState::CatalogSigned).then_some(s.publisher).flatten()
    }

    fn downloaded(&self, t: &UpdateTarget, dir: &str) -> Vec<String> {
        let mut out = Vec::new();
        match t.channel {
            Channel::Scoop => {
                let prefix = format!("{}#{}#", t.package_id, t.to_version).to_lowercase();
                if let Some(cache) = scoop_root().map(|r| r.join("cache"))
                    && let Ok(rd) = std::fs::read_dir(cache)
                {
                    out.extend(rd.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(&prefix)).map(|e| e.path().to_string_lossy().into_owned()));
                }
            }
            _ => {
                let mut stack = vec![PathBuf::from(dir)];
                while let Some(d) = stack.pop() {
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        let p = e.path();
                        let ft = e.file_type();
                        if ft.as_ref().is_ok_and(|f| f.is_dir()) && stack.len() < 8 {
                            stack.push(p);
                        } else if ft.is_ok_and(|f| f.is_file()) && !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("yaml") || x.eq_ignore_ascii_case("yml")) {
                            out.push(p.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn scratch_dir(&self, action_id: &str) -> Result<String, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("cleanmanager-update-{}-{action_id}-{nanos:x}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|e| format!("cannot create a download directory: {e}"))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    fn running(&self, t: &UpdateTarget) -> Option<bool> {
        running_under(t.install_location.as_deref()?)
    }

    fn installed_version(&self, t: &UpdateTarget) -> Option<String> {
        match t.channel {
            Channel::Scoop => {
                let manifest = scoop_root()?.join("apps").join(&t.package_id).join(r"current\manifest.json");
                scoop_manifest_version(&std::fs::read_to_string(manifest).ok()?)
            }
            _ => super::apps::inventory().0.into_iter().find(|a| a.instance_id == t.app_instance_id).and_then(|a| a.version),
        }
    }
}

#[cfg(test)]
mod tests;
