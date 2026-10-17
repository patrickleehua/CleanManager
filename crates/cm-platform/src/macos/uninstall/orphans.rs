//! UNI-08 on macOS: orphaned leftovers of apps removed earlier, under this user's `~/Library` (`Application Support`,
//! `Caches`, `Logs`, `Preferences`, `Saved Application State`, `LaunchAgents`; `Containers` reported only). The
//! decisions live in `cm_core::uninstall::mac::orphan_leftovers`.
//!
//! - "Installed" is every record `apps uninstall list` has (bundles in `/Applications`, `~/Applications` and
//!   `/System/Applications`, App Store apps, Homebrew casks and formulae, npm-global AI CLIs) plus the installer
//!   package receipts in `/var/db/receipts`. Apps on other volumes or in other folders are not seen, which is one
//!   reason every candidate stays unselected and needs the idle period.
//! - Idle: the newest modification time of the entry and of its direct children (never read inside `Containers`).
//! - A launchd job is a candidate only when the program it starts no longer exists.
//! - Sizes are read only for listed candidates that can be planned. Nothing is changed here: a candidate a person names
//!   goes to the Trash through the leftover plan path (`fs_trash`), reviewed on its own. System-level locations
//!   (`/Library`) are not examined: removing them would need administrator rights, which this build never uses.

use cm_core::uninstall::mac::{LibraryEntry, ORPHAN_AREAS, orphan_leftovers};

use super::*;

/// Entries of one folder an idle check reads at most.
const IDLE_CHILDREN_MAX: usize = 2_000;

/// Installer package identifiers on record (`/var/db/receipts/<id>.plist`).
fn receipts(env: &UninstallEnv) -> Vec<String> {
    std::fs::read_dir(env.system("/var/db/receipts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".plist")).map(str::to_owned))
        .collect()
}

/// Seconds since the newest modification of `path` and its direct children (links not followed).
fn idle_seconds(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let mut newest = meta.modified().ok()?;
    if meta.is_dir() {
        for e in std::fs::read_dir(path).ok()?.flatten().take(IDLE_CHILDREN_MAX) {
            if let Some(t) = e.path().symlink_metadata().ok().and_then(|m| m.modified().ok()) {
                newest = newest.max(t);
            }
        }
    }
    Some(std::time::SystemTime::now().duration_since(newest).map(|d| d.as_secs()).unwrap_or(0))
}

/// UNI-08: the orphan candidates of this environment, read-only. `sizes`: read the logical sizes of plannable ones.
pub fn orphans(env: &UninstallEnv, sizes: bool) -> Vec<Leftover> {
    let (recs, _) = records(env);
    let inst = installed(&recs);
    let receipts = receipts(env);
    let entries: Vec<LibraryEntry> = library_entries(env).into_iter().filter(|e| ORPHAN_AREAS.contains(&e.area)).collect();
    let idle = |e: &LibraryEntry| if e.area.privacy_protected() { None } else { idle_seconds(Path::new(&e.path)) };
    let exists = |program: &str| std::fs::symlink_metadata(env.system(program)).is_ok();
    let mut items = orphan_leftovers(&entries, &inst, &receipts, &idle, &exists);
    if sizes {
        for l in &mut items {
            if let Binding::Path { path } = &l.binding {
                l.logical_bytes = logical_size(Path::new(path));
            }
        }
    }
    items
}
