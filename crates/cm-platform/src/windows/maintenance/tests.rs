//! Windows task tests. Only read-only probes touch the real system; the icon-cache action runs on scratch folders. Shell
//! restarts, DNS flushes, retrims and DISM checks are never executed here (see docs/evidence for the recorded runs).

use super::*;

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cm-maint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn tasks(dir: &Path) -> WindowsTasks {
    WindowsTasks { explorer_dir: Some(dir.to_path_buf()), is_admin: false, now: || 1_800_000_000, trim_due_days: TRIM_DUE_DAYS }
}

fn spec(id: &str) -> &'static TaskSpec {
    cm_core::maintenance::find(id).unwrap()
}

#[test]
fn cache_file_names_are_a_closed_set() {
    for ok in ["iconcache_16.db", "IconCache_idx.db", "thumbcache_256.db", "thumbcache_wide_alternate.db"] {
        assert!(is_cache_file_name(ok), "{ok}");
    }
    for bad in ["iconcache.db", "IconCache.db", "thumbcache_16.db.bak", "explorer.db", "iconcache_..\\x.db", "thumbcache_16 .db", "iconcache_x.dbx"] {
        assert!(!is_cache_file_name(bad), "{bad}");
    }
}

#[test]
fn icon_cache_removes_only_unused_cache_files() {
    let d = scratch("icons");
    for n in ["iconcache_16.db", "iconcache_32.db", "thumbcache_256.db", "keep.db", "IconCache.db"] {
        std::fs::write(d.join(n), b"cache").unwrap();
    }
    std::fs::create_dir(d.join("iconcache_99.db")).unwrap();
    let t = tasks(&d);
    assert!(matches!(t.probe(spec(ids::ICON_CACHE)), Probe::Applicable { ref detail } if detail.starts_with("3 cache files")));
    // A file another handle holds open (as Explorer does) is never removed.
    let held = std::fs::File::open(d.join("thumbcache_256.db")).unwrap();
    let e = t.execute(spec(ids::ICON_CACHE));
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Attention, "partly_in_use"), "{}", e.summary);
    assert!(!d.join("iconcache_16.db").exists() && !d.join("iconcache_32.db").exists());
    assert!(d.join("thumbcache_256.db").exists() && d.join("keep.db").exists() && d.join("IconCache.db").exists() && d.join("iconcache_99.db").is_dir());
    // Everything left in use: attention, nothing removed.
    let e = t.execute(spec(ids::ICON_CACHE));
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Attention, "in_use"));
    drop(held);
    let e = t.execute(spec(ids::ICON_CACHE));
    assert_eq!((e.outcome, e.code.as_str()), (TaskOutcome::Applied, "rebuilt"));
    assert!(!d.join("thumbcache_256.db").exists());
    assert!(matches!(t.probe(spec(ids::ICON_CACHE)), Probe::NotNeeded { ref code, .. } if code == "no_cache_files"));
    assert_eq!(t.execute(spec(ids::ICON_CACHE)).outcome, TaskOutcome::Unchanged);
    let gone = tasks(&d.join("missing"));
    assert!(matches!(gone.probe(spec(ids::ICON_CACHE)), Probe::NotApplicable { .. }));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn icon_cache_never_follows_a_linked_cache_folder() {
    let d = scratch("icons-link");
    let real = d.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("iconcache_16.db"), b"cache").unwrap();
    let link = d.join("Explorer");
    let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&real).stdout(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if ok {
        let t = tasks(&link);
        assert!(matches!(t.probe(spec(ids::ICON_CACHE)), Probe::Unknown { .. }));
        assert_eq!(t.execute(spec(ids::ICON_CACHE)).outcome, TaskOutcome::Skipped);
        assert!(real.join("iconcache_16.db").exists());
        let _ = std::fs::remove_dir(&link);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn trim_dates_and_due_rule() {
    assert_eq!(systemtime_bytes_to_unix(&[0u8; 16]), None, "never optimized");
    let st = |y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16| -> Vec<u8> { [y, mo, 4, d, h, mi, s, 0].iter().flat_map(|w| w.to_le_bytes()).collect() };
    assert_eq!(systemtime_bytes_to_unix(&st(2026, 10, 1, 8, 30, 15)), Some(1_790_843_415));
    assert_eq!(systemtime_bytes_to_unix(&st(2000, 2, 29, 0, 0, 0)), Some(951_782_400));
    assert_eq!(systemtime_bytes_to_unix(&st(2026, 13, 1, 0, 0, 0)), None);
    assert_eq!(systemtime_bytes_to_unix(&[1, 2, 3]), None);
    let now = 1_790_843_415;
    assert!(trim_due(None, now, 30));
    assert!(!trim_due(Some(now - 29 * 86_400), now, 30));
    assert!(trim_due(Some(now - 31 * 86_400), now, 30));
}

#[test]
fn shell_hosts_must_be_system_apps_of_the_right_name() {
    let apps = r"c:\windows\systemapps\";
    assert!(is_shell_host("StartMenuExperienceHost.exe", r"C:\Windows\SystemApps\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\StartMenuExperienceHost.exe", apps));
    assert!(is_shell_host("SearchHost.exe", r"C:\Windows\SystemApps\MicrosoftWindows.Client.CBS_cw5n1h2txyewy\SearchHost.exe", apps));
    assert!(!is_shell_host("SearchHost.exe", r"C:\Users\x\AppData\Local\Temp\SearchHost.exe", apps), "same name elsewhere");
    assert!(!is_shell_host("explorer.exe", r"C:\Windows\explorer.exe", apps), "Explorer is never restarted");
    assert!(!is_shell_host("SearchHost.exe", r"C:\Windows\SystemApps\x\Other.exe", apps));
    assert!(!is_shell_host("SearchHost.exe", r"C:\Windows\SystemApps\..\Temp\SearchHost.exe", apps));
}

/// Read-only probes on the real system never fail hard and never act.
#[test]
fn real_probes_are_read_only_and_total() {
    let t = WindowsTasks { explorer_dir: Some(scratch("probe-only")), ..WindowsTasks::real() };
    for id in [ids::DNS_FLUSH, ids::SHELL_RESTART, ids::TRIM, ids::IMAGE_HEALTH] {
        let p = t.probe(spec(id));
        let (applicability, _) = p.applicability();
        assert!(["applicable", "not_needed", "not_applicable", "unknown"].contains(&applicability), "{id}: {p:?}");
    }
    assert!(matches!(t.probe(spec(ids::DNS_FLUSH)), Probe::Applicable { .. }), "dnsapi.dll is part of Windows");
    let unelevated = WindowsTasks { is_admin: false, ..WindowsTasks::real() };
    for id in [ids::TRIM, ids::IMAGE_HEALTH] {
        assert_eq!(unelevated.execute(spec(id)).code, "permission_required", "{id}: administrator tasks never run unelevated in-process");
    }
}
