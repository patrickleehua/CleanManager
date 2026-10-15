//! Real-machine tests for PRT-03/04: identity matching against this machine's system roots.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use cm_core::protect::{DenyLayer, check_identity, check_path, prefilter};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

use super::*;

fn raw(p: &Path) -> RawPath {
    RawPath::windows(wide(p)).unwrap()
}

fn system_root() -> PathBuf {
    env_path("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// A unique scratch directory that removes junctions (never their targets) before itself.
struct Scratch {
    dir: PathBuf,
    junctions: Vec<PathBuf>,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("cm-hard-deny-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir, junctions: Vec::new() }
    }

    fn junction(&mut self, name: &str, target: &Path) -> PathBuf {
        let link = self.dir.join(name);
        let ok = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(target).output().unwrap();
        assert!(ok.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&ok.stderr));
        self.junctions.push(link.clone());
        link
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for j in &self.junctions {
            // RemoveDirectoryW on a junction deletes the reparse point only.
            let _ = std::fs::remove_dir(j);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn assert_hard_denied(d: &ProtectionDecision, what: &str) {
    assert!(matches!(d, ProtectionDecision::Denied { layer: DenyLayer::HardDeny, .. }), "{what}: {d:?}");
}

#[test]
fn hard_deny_roots_include_this_machines_system_root() {
    let roots = hard_deny_roots();
    let id = identity(&wide(&system_root())).unwrap();
    assert!(
        roots.iter().any(|r| r.class == HardDenyClass::SystemRoot && r.volume_id == id.volume_id && r.file_id == id.file_id),
        "SystemRoot identity missing from {} roots",
        roots.len()
    );
    for class in [HardDenyClass::VolumeRoot, HardDenyClass::UsersRoot, HardDenyClass::ProgramFiles, HardDenyClass::ProgramDataRoot] {
        assert!(roots.iter().any(|r| r.class == class), "no {class:?} root");
    }
    if let Some(profile) = env_path("USERPROFILE") {
        let pid = identity(&wide(&profile)).unwrap();
        let set = hard_deny_set();
        assert!(set.root_at(&pid).is_some(), "the current profile root is denied");
        assert!(set.covering(&pid).is_none(), "but only exactly, not its descendants");
    }
}

#[test]
fn hard_deny_case_variant_resolves_to_a_denied_identity() {
    let set = hard_deny_set();
    let canonical = system_root().join("System32");
    let text = system_root().display().to_string();
    // c:\WINDOWS\system32 style: lower-case drive, upper-case directory, lower-case child.
    let variant = format!("{}{}\\system32", text[..2].to_lowercase(), text[2..].to_uppercase());
    assert_ne!(variant, canonical.display().to_string());
    let chain = identity_chain(&variant.encode_utf16().collect::<Vec<_>>());
    assert_eq!(chain.object, Some(identity(&wide(&canonical)).unwrap()), "same object, different spelling");
    assert_hard_denied(&check_identity(&chain, &set), &variant);
}

#[test]
fn hard_deny_short_and_volume_guid_forms_are_denied_by_identity() {
    let set = hard_deny_set();
    let long = env_path("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let mut z = wide(&long);
    z.push(0);
    let mut buf = vec![0u16; 1024];
    // SAFETY: NUL-terminated input; output length bounded by the buffer.
    let n = unsafe { GetShortPathNameW(z.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    assert!(n > 0 && n < buf.len(), "GetShortPathNameW failed: {}", io::Error::last_os_error());
    buf.truncate(n);
    let short = String::from_utf16_lossy(&buf);
    if short.eq_ignore_ascii_case(&long.display().to_string()) {
        eprintln!("8.3 names are disabled on this volume; short-name case skipped");
    } else {
        eprintln!("short form under test: {short}");
        let p = RawPath::windows(buf.clone()).unwrap();
        assert_eq!(prefilter(&p), Ok(()), "{short} passes the text filter, so identity must catch it");
        assert_hard_denied(&check_path(&p, &identity_chain(&buf), &set), &short);
    }

    // \\?\Volume{GUID}\Program Files: no drive letter at all.
    let h = Handle::open(&full_path(&wide(&long)).unwrap().1, FILE_READ_ATTRIBUTES | SYNCHRONIZE).unwrap();
    let guid = h.volume_guid().expect("system volume has a GUID name");
    let text = long.display().to_string();
    let by_guid = format!("{guid}{}", &text[2..]);
    let units: Vec<u16> = by_guid.encode_utf16().collect();
    assert_hard_denied(&check_identity(&identity_chain(&units), &set), &by_guid);
}

#[test]
fn hard_deny_junction_into_windows_is_denied_through_the_link() {
    let set = hard_deny_set();
    let mut s = Scratch::new("junction");
    let link = s.junction("jwin", &system_root());
    assert_eq!(prefilter(&raw(&link)), Ok(()), "the text alone looks harmless");

    let chain = identity_chain(&wide(&link));
    assert!(chain.links.iter().any(|l| l.on_object && l.target.is_ok()), "{chain:?}");
    let d = check_path(&raw(&link), &chain, &set);
    assert!(matches!(&d, ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason } if reason.contains("link")), "{d:?}");

    // Below the junction: the ancestor link (and the real path) deny.
    let inside = link.join("System32").join("drivers");
    assert_hard_denied(&check(&raw(&inside), &set), "path below the junction");
    // The real location is reported so the UI can show where the link leads.
    let real = identity_chain(&wide(&inside)).real_path.expect("real path").display().to_lowercase();
    assert!(real.contains("system32"), "{real}");
}

#[test]
fn hard_deny_unresolvable_link_is_denied() {
    let set = hard_deny_set();
    let mut s = Scratch::new("broken");
    let target = s.dir.join("target");
    std::fs::create_dir(&target).unwrap();
    let link = s.junction("jbroken", &target);
    assert!(check(&raw(&link), &set).is_allowed(), "a junction to an ordinary directory is allowed");
    std::fs::remove_dir(&target).unwrap();
    let chain = identity_chain(&wide(&link));
    assert!(chain.links.iter().any(|l| l.target.is_err()), "{chain:?}");
    assert_hard_denied(&check_path(&raw(&link), &chain, &set), "dangling junction");
}

/// `\\localhost\C$` stands in for any SMB share of a local folder (Windows itself creates a
/// `Users` share when public folder sharing is on, and mapped drives can point at either). The
/// text filter only knows administrative shares by name, so for `\\host\Users\me\AppData\...`
/// the identity layer is the only guard. Over SMB the handle has no volume GUID, so the
/// identities must still be matched to the local volume by serial (regression: they were not,
/// and every path through a loopback share was `Allowed`).
#[test]
fn hard_deny_smb_loopback_alias_is_denied_by_identity() {
    let set = hard_deny_set();
    let sys = system_root();
    let text = sys.display().to_string();
    let share_root = format!(r"\\localhost\{}$", &text[..1]);
    if !Path::new(&format!(r"{share_root}\")).exists() {
        eprintln!("loopback administrative share {share_root} is not reachable; SMB alias case skipped");
        return;
    }
    let mut cases = vec![format!("{share_root}{}", &text[2..]), format!("{share_root}{}\\System32\\drivers", &text[2..])];
    if let Some(profile) = env_path("USERPROFILE") {
        let p = profile.display().to_string();
        cases.push(format!("{share_root}{}", &p[2..]));
        let protect = profile.join("AppData").join("Roaming").join("Microsoft").join("Protect");
        if protect.exists() {
            cases.push(format!("{share_root}{}", &protect.display().to_string()[2..]));
        }
    }
    for unc in cases {
        let units: Vec<u16> = unc.encode_utf16().collect();
        let mut chain = identity_chain(&units);
        assert!(chain.object.is_some(), "{unc} should be identifiable over SMB: {chain:?}");
        // The real path `\\?\UNC\localhost\C$\...` is caught by the text filter; drop it so only
        // the identities decide, as for a share whose name the text filter does not know.
        chain.real_path = None;
        assert_hard_denied(&check_identity(&chain, &set), &unc);
    }
}

#[test]
fn hard_deny_ordinary_temp_directory_is_allowed() {
    let set = hard_deny_set();
    let s = Scratch::new("plain");
    let dir = s.dir.join("cache").join("x");
    std::fs::create_dir_all(&dir).unwrap();
    let d = check(&raw(&dir), &set);
    assert!(d.is_allowed(), "{}: {d:?}", dir.display());
    assert_hard_denied(&check(&raw(&s.dir.join("missing")), &set), "an object that cannot be opened");
}
