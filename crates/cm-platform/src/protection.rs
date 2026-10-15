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

/// Pre-action protection check used by the executor: hard deny (by identity) → default
/// classes → user list. Loaded once per operation; an unavailable list denies everything.
/// `rule_refs` are the plan's `rule_id@version` references: a rule the user protected after
/// planning stops all of the plan's items (plans do not record a rule per item).
pub fn checker(store_dir: &Path, rule_refs: Vec<String>) -> impl Fn(&ActionTarget<'_>) -> Result<(), String> {
    let user = load(store_dir);
    #[cfg(windows)]
    let hard = crate::windows::hard_deny::hard_deny_set();
    move |target: &ActionTarget<'_>| {
        let user = user.as_ref().map_err(|e| format!("protection_unavailable: {e}"))?;
        for r in &rule_refs {
            let id = r.split('@').next().unwrap_or(r);
            if let Some(e) = user.protects_rule(id) {
                return Err(format!("rule {id} is protected by user entry {}", e.id));
            }
        }
        #[cfg(windows)]
        {
            let cm_core::path::RawPath::Windows(units) = target.path else { return Err("not a Windows path".into()) };
            let chain = crate::windows::hard_deny::identity_chain(units);
            let subject = cm_core::protect::Subject { path: target.path, chain: &chain, default_class: None, rule_id: None, recursive: false };
            match cm_core::protect::evaluate(&subject, &hard, user) {
                cm_core::protect::ProtectionDecision::Allowed => Ok(()),
                denied => Err(format!("{denied:?}")),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = target;
            Err("protection_unavailable: no hard-deny set for this platform yet".into())
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
}
