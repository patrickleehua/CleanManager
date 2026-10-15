use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("cm-probe-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(p.join("sub")).unwrap();
        Dir(p)
    }
    fn file(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::write(&p, b"x").unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn owner_probe_is_tri_state_and_enumeration_failure_is_unknown() {
    let running = ProcessSnapshot::fixed(Ok(&["System", "CHROME.EXE"]));
    assert_eq!(probe_owner(Some("google.chrome"), &running).state, ProbeState::InUse);
    assert_eq!(probe_owner(Some("microsoft.edge"), &running).state, ProbeState::NotInUse);
    let failed = ProcessSnapshot::fixed(Err("snapshot denied"));
    assert_eq!(probe_owner(Some("microsoft.edge"), &failed).state, ProbeState::Unknown, "a failed listing is never 'not running'");
    assert_eq!(probe_owner(Some("unmapped.app"), &running).state, ProbeState::Unknown);
    assert_eq!(probe_owner(None, &running).state, ProbeState::Unknown);
    assert!(ProcessSnapshot::take().names.is_ok_and(|n| n.contains("system")), "the real snapshot lists processes");
}

#[test]
fn an_open_handle_makes_the_tree_in_use() {
    let d = Dir::new("handles");
    d.file("a.bin");
    let held = d.file("sub\\index");
    assert_eq!(probe_open_files(&d.0, false).state, ProbeState::NotInUse);
    let h = std::fs::File::open(&held).unwrap();
    let r = probe_open_files(&d.0, false);
    assert_eq!(r.state, ProbeState::InUse, "{r:?}");
    assert_eq!(probe_open_files(&d.0, true).state, ProbeState::NotInUse, "database probe only looks at database/log files");
    drop(h);
    let db = d.file("sub\\state.db-wal");
    let h = std::fs::File::open(&db).unwrap();
    assert_eq!(probe_open_files(&d.0, true).state, ProbeState::InUse);
    drop(h);
    assert_eq!(probe_open_files(&d.0, true).state, ProbeState::NotInUse);
}

#[test]
fn a_missing_root_is_unknown_not_free() {
    let d = Dir::new("missing");
    let r = probe_open_files(&d.0.join("nope"), false);
    assert_eq!(r.state, ProbeState::Unknown);
    assert_eq!(probe_downloads(&d.0.join("nope")).state, ProbeState::Unknown);
}

#[test]
fn held_download_markers_are_in_use() {
    let d = Dir::new("downloads");
    let marker = d.file("setup.exe.crdownload");
    d.file("done.zip");
    assert_eq!(probe_downloads(&d.0).state, ProbeState::NotInUse);
    let h = std::fs::OpenOptions::new().append(true).open(&marker).unwrap();
    assert_eq!(probe_downloads(&d.0).state, ProbeState::InUse);
    assert_eq!(probe_downloads(&marker).state, ProbeState::InUse, "a marker file itself can be the candidate");
    assert_eq!(probe_downloads_shallow(&d.0).state, ProbeState::InUse);
    drop(h);
    assert_eq!(probe_downloads_shallow(&d.0).state, ProbeState::NotInUse);
}

#[test]
fn endpoint_security_locations_are_in_use() {
    for p in ["C:\\ProgramData\\Microsoft\\Windows Defender\\Scans", "C:\\ProgramData\\CrowdStrike\\x", "C:\\Users\\a\\AppData\\Local\\Sophos\\cache"] {
        assert_eq!(probe_endpoint(Path::new(p)).state, ProbeState::InUse, "{p}");
    }
    assert_eq!(probe_endpoint(Path::new("C:\\Users\\a\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\GPUCache")).state, ProbeState::NotInUse);
}

#[test]
fn every_declared_probe_is_answered_and_unknown_kinds_are_unknown() {
    let d = Dir::new("all");
    d.file("x");
    let raw = RawPath::windows(d.0.as_os_str().encode_wide().collect()).unwrap();
    let kinds: Vec<String> = ["owner_process", "open_file_handle", "vcs_tracked", "open_file_handle"].iter().map(|s| s.to_string()).collect();
    let r = probe_candidate(&raw, &kinds, Some("google.chrome"), &ProcessSnapshot::fixed(Ok(&["idle.exe"])));
    let by: Vec<(&str, ProbeState)> = r.iter().map(|p| (p.probe.as_str(), p.state)).collect();
    assert_eq!(by, vec![("open_file_handle", ProbeState::NotInUse), ("owner_process", ProbeState::NotInUse), ("vcs_tracked", ProbeState::Unknown)]);
}

#[test]
fn administrator_locations_are_skipped_without_an_elevated_token() {
    assert_eq!(probe_elevation(true).state, ProbeState::NotInUse);
    let refused = probe_elevation(false);
    assert_eq!(refused.state, ProbeState::Unknown, "no elevation is never read as allowed");
    assert!(refused.detail.contains("administrator"));
    let d = Dir::new("admin");
    d.file("x");
    let raw = RawPath::windows(d.0.as_os_str().encode_wide().collect()).unwrap();
    let r = probe_candidate(&raw, &["elevated_token".to_string()], None, &ProcessSnapshot::fixed(Ok(&["idle.exe"])));
    let expected = if has_admin_rights() { ProbeState::NotInUse } else { ProbeState::Unknown };
    assert_eq!(r[0].state, expected, "the live probe follows this process token");
}

#[test]
fn wave_two_owners_map_to_their_processes() {
    for owner in ["bytedance.feishu", "tencent.wechat", "netease.uu-remote", "docker.desktop", "microsoft.onedrive", "microsoft.winget"] {
        assert!(owner_processes(owner).is_some_and(|p| !p.is_empty()), "{owner}");
    }
    assert!(!owner_processes("pnpm").unwrap().iter().any(|p| p.eq_ignore_ascii_case("node.exe")), "node.exe runs for many tools, never an owner");
}
