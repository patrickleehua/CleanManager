//! Parity 17.3 (LIF-03) and 17.4 (LIF-04): completion scripts are generated for four shells and a
//! profile install is only ever previewed for machine callers (the file is not touched); the
//! Doctor report is read-only, redacted by default (no user or computer name, no home path, no
//! environment or credentials) and an export is a preview until a person confirms in a terminal.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-cli-tools-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run(store: &Path, args: &[&str]) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).env("CM_STORE_DIR", store).output().unwrap();
    (o.status.code().unwrap(), String::from_utf8(o.stdout).unwrap())
}

fn json(store: &Path, args: &[&str]) -> (i32, Value) {
    let (code, out) = run(store, &[args, &["--json"]].concat());
    assert_eq!(out.lines().count(), 1, "{out}");
    (code, serde_json::from_str(&out).unwrap())
}

fn available(program: &str, probe: &[&str]) -> bool {
    Command::new(program).args(probe).output().is_ok_and(|o| o.status.success())
}

#[test]
fn completion_scripts_for_every_shell() {
    let base = scratch("print");
    let store = base.join("store");
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let (code, text) = run(&store, &["completion", "print", shell, "--format", "text"]);
        assert_eq!(code, 0);
        assert!(text.to_lowercase().starts_with(&format!("# cleanmanager {shell} completion")), "{shell}: {}", &text[..80.min(text.len())]);
        for word in ["capabilities", "scan", "wait", "entries", "plan", "review", "recovery", "purge", "installer", "doctor", "non-interactive", "format", "timeout"] {
            assert!(text.contains(word), "{shell} script lacks {word}");
        }
        assert!(!text.contains("host-scan-id"), "internal options are not offered");
        let (code, rec) = json(&store, &["completion", "print", shell]);
        assert_eq!(code, 0);
        assert_eq!((rec["kind"].as_str(), rec["action"].as_str(), rec["written"].as_bool()), (Some("completion_script"), Some("print"), Some(false)));
        assert_eq!(rec["script"].as_str().unwrap(), text, "JSON wraps the same script");
        std::fs::write(base.join(format!("cm.{shell}")), &text).unwrap();
    }
    // Syntax checks where the shell exists on this machine.
    if available("bash", &["--version"]) {
        let o = Command::new("bash").arg("-n").arg(base.join("cm.bash")).output().unwrap();
        assert!(o.status.success(), "bash -n: {}", String::from_utf8_lossy(&o.stderr));
    }
    for ps in ["pwsh", "powershell"] {
        if available(ps, &["-NoProfile", "-Command", "exit 0"]) {
            let check = format!(
                "$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$null, [ref]$e); if ($e.Count) {{ $e | Out-String | Write-Error; exit 1 }}",
                base.join("cm.powershell").display()
            );
            let o = Command::new(ps).args(["-NoProfile", "-Command", &check]).output().unwrap();
            assert!(o.status.success(), "{ps} parse: {}", String::from_utf8_lossy(&o.stderr));
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn completion_install_is_a_preview_for_machine_callers() {
    let base = scratch("install");
    let store = base.join("store");
    let profile = base.join("profile.ps1");
    std::fs::write(&profile, "# existing profile\r\nSet-Alias ll Get-ChildItem\r\n").unwrap();
    let before = std::fs::read(&profile).unwrap();
    for extra in [&["--json"][..], &["--non-interactive", "--format", "text"][..], &["--json", "--non-interactive"][..]] {
        let (code, out) = run(&store, &[&["completion", "install", "powershell", "--profile", profile.to_str().unwrap()][..], extra].concat());
        assert_eq!(code, 0, "{out}");
        if extra.contains(&"--json") {
            let rec: Value = serde_json::from_str(out.trim_end()).unwrap();
            assert_eq!((rec["action"].as_str(), rec["written"].as_bool()), (Some("preview"), Some(false)));
            assert!(rec["profile_path"]["display"].as_str().unwrap().ends_with("profile.ps1"));
            let block = rec["script"].as_str().unwrap();
            assert!(block.starts_with("# >>> cleanmanager completion >>>") && block.contains("completion print powershell --format text"));
        }
        assert_eq!(std::fs::read(&profile).unwrap(), before, "preview must not modify the profile");
    }
    // A profile that does not exist is not created by a preview either.
    let missing = base.join("nested").join("new_profile");
    let (code, rec) = json(&store, &["completion", "install", "bash", "--profile", missing.to_str().unwrap()]);
    assert_eq!((code, rec["written"].as_bool()), (0, Some(false)));
    assert!(!missing.exists() && !base.join("nested").exists());
    let _ = std::fs::remove_dir_all(&base);
}

fn secrets() -> Vec<String> {
    let mut v = Vec::new();
    for var in ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME", "USERPROFILE", "HOME"] {
        if let Ok(s) = std::env::var(var)
            && s.len() >= 3
        {
            v.push(s.to_lowercase());
        }
    }
    v
}

#[test]
fn doctor_is_read_only_redacted_and_exports_only_after_confirmation() {
    // The store lives under the user's temp directory, so its path contains the user name.
    let base = scratch("doctor");
    let store = base.join("store");
    let (code, absent) = json(&store, &["doctor"]);
    assert_eq!(code, 0);
    assert!(!store.exists(), "doctor does not create a store");
    let present = absent["checks"].as_array().unwrap().iter().find(|c| c["check"] == "store.present").unwrap();
    assert_eq!(present["status"], "warning");

    let fx = base.join("fx");
    std::fs::create_dir_all(&fx).unwrap();
    std::fs::write(fx.join("a.txt"), "a").unwrap();
    let (code, _) = json(&store, &["scan", "--root", fx.to_str().unwrap()]);
    assert_eq!(code, 0);
    let (code, out) = run(&store, &["doctor", "--json"]);
    assert_eq!(code, 0);
    let rec: Value = serde_json::from_str(out.trim_end()).unwrap();
    assert_eq!((rec["kind"].as_str(), rec["redacted"].as_bool()), (Some("doctor_report"), Some(true)));
    let lower = out.to_lowercase();
    for s in secrets() {
        assert!(!lower.contains(&s), "redacted report contains {s:?}");
    }
    let areas: std::collections::BTreeSet<&str> = rec["checks"].as_array().unwrap().iter().map(|c| c["area"].as_str().unwrap()).collect();
    for area in ["platform", "permissions", "config", "store"] {
        assert!(areas.contains(area), "missing {area} checks");
    }
    assert!(rec["checks"].as_array().unwrap().iter().any(|c| c["check"] == "store.open" && c["status"] == "ok"));
    assert_eq!(rec["error_codes"].as_array().unwrap().len(), 13, "the full error-code table");
    assert!(!rec["capabilities"].as_array().unwrap().is_empty());
    for forbidden in ["password", "token", "secret", "api_key", "PATH="] {
        assert!(!out.contains(forbidden), "{forbidden}");
    }

    // Export: a preview of the exact report; nothing is written without a person at a terminal.
    let target = base.join("report.json");
    for extra in [&["--json"][..], &["--non-interactive", "--format", "text"][..]] {
        let (code, out) = run(&store, &[&["doctor", "--export", target.to_str().unwrap()][..], extra].concat());
        assert_eq!(code, 0, "{out}");
        if extra.contains(&"--json") {
            let rec: Value = serde_json::from_str(out.trim_end()).unwrap();
            assert_eq!(rec["export"]["written"], false);
            assert!(rec["export"]["path"].as_str().unwrap().ends_with("report.json"));
        }
        assert!(!target.exists(), "export preview wrote a file");
    }
    // Unredacted on request (still no environment dump or credentials).
    let (_, raw) = run(&store, &["doctor", "--no-redact", "--json"]);
    let raw: Value = serde_json::from_str(raw.trim_end()).unwrap();
    assert_eq!(raw["redacted"], false);
    let _ = std::fs::remove_dir_all(&base);
}
