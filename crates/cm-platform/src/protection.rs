//! User protection list storage and the pre-action protection check (PRT-01..05). The list
//! lives next to the stores as `protection.json`; a missing file is an empty list, anything
//! unreadable or invalid makes protection unavailable and every write stops (fail closed).

use std::path::Path;

use cm_core::capability::Platform;
use cm_core::execute::ActionTarget;
use cm_core::protect::{ProtectionUnavailable, UserProtection};

pub const PROTECTION_FILE: &str = "protection.json";

fn platform() -> Platform {
    if cfg!(windows) { Platform::Windows } else { Platform::Macos }
}

/// Load the user list from `store_dir`. Missing file → empty list.
pub fn load(store_dir: &Path) -> Result<UserProtection, ProtectionUnavailable> {
    match std::fs::read_to_string(store_dir.join(PROTECTION_FILE)) {
        Ok(text) => cm_core::protect::load_protection_file_for(&text, platform()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserProtection::empty(platform())),
        Err(e) => Err(ProtectionUnavailable::Unreadable(e.to_string())),
    }
}

/// Atomic replace: write a sibling temp file, flush it, then rename over the list, so a crash
/// leaves either the old or the new list and never a truncated one.
pub fn save(store_dir: &Path, list: &UserProtection) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(store_dir)?;
    let tmp = store_dir.join(format!("{PROTECTION_FILE}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(list.serialize().as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, store_dir.join(PROTECTION_FILE)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Why a read-modify-write of the list did not happen.
#[derive(Debug)]
pub enum UpdateError<E> {
    /// The stored list is unusable; nothing was written (PRT-02).
    Unavailable(ProtectionUnavailable),
    /// The change itself was refused; nothing was written.
    Rejected(E),
    /// Another writer holds the list lock.
    Busy,
    Io(std::io::Error),
}

const LOCK_FILE: &str = "protection.json.lock";
const LOCK_STALE: std::time::Duration = std::time::Duration::from_secs(30);

/// Exclusive lock file shared by every writer (CLI and GUI), removed on drop. A lock older than
/// 30 s is a crashed writer's and is broken.
struct ListLock(std::path::PathBuf);

impl ListLock {
    fn acquire(store_dir: &Path) -> std::io::Result<Option<ListLock>> {
        std::fs::create_dir_all(store_dir)?;
        let path = store_dir.join(LOCK_FILE);
        for _ in 0..60 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Some(ListLock(path))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > LOCK_STALE));
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl Drop for ListLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Load, change and atomically save the list under the writer lock, so concurrent CLI and GUI
/// writers never lose each other's entries. `change` runs on the freshly loaded list.
pub fn update<T, E>(store_dir: &Path, change: impl FnOnce(&mut UserProtection) -> Result<T, E>) -> Result<(T, UserProtection), UpdateError<E>> {
    let _lock = ListLock::acquire(store_dir).map_err(UpdateError::Io)?.ok_or(UpdateError::Busy)?;
    let mut list = load(store_dir).map_err(UpdateError::Unavailable)?;
    let out = change(&mut list).map_err(UpdateError::Rejected)?;
    save(store_dir, &list).map_err(UpdateError::Io)?;
    Ok((out, list))
}

/// Platform hard-deny roots of this machine, resolved by identity.
pub fn hard_deny_set() -> cm_core::protect::HardDenySet {
    #[cfg(windows)]
    {
        crate::windows::hard_deny::hard_deny_set()
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::hard_deny::hard_deny_set()
    }
}

/// Identity facts for one path on this platform. A path of the other platform yields an empty
/// chain, which every check treats as unidentified (denied).
pub fn identity_chain(path: &cm_core::path::RawPath) -> cm_core::protect::IdentityChain {
    match path {
        #[cfg(windows)]
        cm_core::path::RawPath::Windows(units) => crate::windows::hard_deny::identity_chain(units),
        #[cfg(target_os = "macos")]
        cm_core::path::RawPath::Unix(bytes) => crate::macos::hard_deny::identity_chain(bytes),
        _ => cm_core::protect::IdentityChain::default(),
    }
}

/// Pre-action protection check used by the executor: hard deny (by identity) → default
/// classes → user list. Loaded once per operation; an unavailable list denies everything.
/// `rule_refs` are the plan's `rule_id@version` references: a rule the user protected after
/// planning stops all of the plan's items (plans do not record a rule per item).
pub fn checker(store_dir: &Path, rule_refs: Vec<String>) -> impl Fn(&ActionTarget<'_>) -> Result<(), String> {
    let user = load(store_dir);
    let hard = hard_deny_set();
    move |target: &ActionTarget<'_>| {
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        for r in &rule_refs {
            let id = r.split('@').next().unwrap_or(r);
            if let Some(e) = user.protects_rule(id) {
                return Err(format!("rule {id} is protected by user entry {}", e.id));
            }
        }
        let chain = identity_chain(target.path);
        let subject = cm_core::protect::Subject { path: target.path, chain: &chain, default_class: None, rule_id: None, recursive: false };
        match cm_core::protect::evaluate(&subject, &hard, user) {
            cm_core::protect::ProtectionDecision::Allowed => Ok(()),
            denied => Err(format!("{denied:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_and_garbage_fails_closed() {
        let dir = std::env::temp_dir().join(format!("cm-protection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir).unwrap().entries().is_empty());
        let mut list = UserProtection::empty(platform());
        list.add(cm_core::protect::UserEntry::rule("u1", "microsoft.edge.code-cache", "2026-10-15T00:00:00Z")).unwrap();
        save(&dir, &list).unwrap();
        assert_eq!(load(&dir).unwrap().entries().len(), 1);
        std::fs::write(dir.join(PROTECTION_FILE), b"{not json").unwrap();
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_is_serialized_and_never_overwrites_an_unusable_list() {
        let dir = std::env::temp_dir().join(format!("cm-protection-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    update(&dir, |l| l.add(cm_core::protect::UserEntry::rule(&format!("u_{i}"), &format!("rule.{i}"), "2026-10-15T00:00:00Z"))).map(|_| ())
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap().is_ok());
        }
        assert_eq!(load(&dir).unwrap().entries().len(), 8, "no writer lost another's entry");
        assert!(!dir.join(LOCK_FILE).exists());
        std::fs::write(dir.join(PROTECTION_FILE), b"{\"schema_version\": 99, \"entries\": []}").unwrap();
        let r = update(&dir, |l| l.add(cm_core::protect::UserEntry::rule("u_x", "r", "2026-10-15T00:00:00Z")));
        assert!(matches!(r, Err(UpdateError::Unavailable(_))));
        assert!(std::fs::read_to_string(dir.join(PROTECTION_FILE)).unwrap().contains("99"), "unusable list left for repair");
        std::fs::remove_dir_all(&dir).ok();
    }
}
