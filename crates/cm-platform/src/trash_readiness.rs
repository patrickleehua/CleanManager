//! Up-front readiness of the platform bin (the Windows Recycle Bin, the macOS Trash) for plans that move objects
//! there (MTR-03, WFA-35). Every entry that creates such a plan (manual `trash`, `clean`, `plan create`, `purge` and
//! `installer` plans in trash mode, the desktop) asks this before freezing it: a target on a volume without a usable
//! bin refuses the whole plan with `unsupported` and the `trash_unavailable` message naming the volumes, instead of
//! producing a plan that a person would review and approve only for it to fail at execution. Nothing is ever deleted
//! instead, and the mode is never switched silently: the person chooses quarantine or turns the bin on.

use cm_core::path::RawPath;

/// What the platform calls its bin, in messages.
pub const BIN: &str = cm_core::manual_trash::BIN_NAME;

/// A volume among the targets that has no usable bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unready {
    /// The volume for people: its root or mount folder (Windows), its mount point (macOS), or the target path when
    /// even that could not be determined.
    pub volume: String,
    pub reason: String,
}

/// Every volume among `paths` without a usable bin, each volume checked once (first-seen order); empty when all are
/// ready.
pub fn unready(paths: &[RawPath]) -> Vec<Unready> {
    #[cfg(windows)]
    let found = crate::windows::actions::recycle_bins_unready(paths);
    #[cfg(target_os = "macos")]
    let found = crate::macos::actions::trash::trash_unready(paths);
    found.into_iter().map(|(volume, reason)| Unready { volume, reason }).collect()
}

/// The refusal message: `trash_unavailable: …` with each volume and why, and that no permanent delete is offered.
pub fn message(unready: &[Unready]) -> String {
    let list: Vec<String> = unready.iter().map(|u| format!("{} ({})", u.volume, u.reason)).collect();
    format!("trash_unavailable: no usable {BIN} on {}; there is no permanent-delete alternative", list.join(", "))
}

fn volumes(unready: &[Unready]) -> String {
    unready.iter().map(|u| u.volume.as_str()).collect::<Vec<_>>().join(", ")
}

/// How to make the bin usable again on this platform.
#[cfg(windows)]
const ENABLE_BIN: &str = "turn the Recycle Bin on for it (right-click the Recycle Bin › Properties › select the volume › Custom size; a policy setting has to be changed by its administrator)";
#[cfg(target_os = "macos")]
const ENABLE_BIN: &str = "move the items to a volume that has a Trash (network, read-only and some external volumes have none)";

/// The next step for a refused plan. `quarantine`: how this entry offers quarantine mode (`--mode quarantine` in the
/// CLI), which keeps the objects on their own volume and needs no bin; `None` when it offers none (manual selection).
pub fn next_step(unready: &[Unready], quarantine: Option<&str>) -> String {
    let vols = volumes(unready);
    match quarantine {
        Some(q) => format!("Create the plan again with {q}, or {ENABLE_BIN} and plan again; affected: {vols}"),
        None => format!("Nothing on {vols} can be moved to the {BIN} until you {ENABLE_BIN}; then plan again"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_every_volume_and_offers_no_delete() {
        let u = vec![Unready { volume: "W:\\".into(), reason: "turned off".into() }, Unready { volume: "X:\\".into(), reason: "network".into() }];
        let m = message(&u);
        assert!(m.starts_with("trash_unavailable: ") && m.contains("W:\\ (turned off)") && m.contains("X:\\ (network)"), "{m}");
        assert!(m.contains("no permanent-delete alternative"), "{m}");
        let with = next_step(&u, Some("--mode quarantine"));
        assert!(with.contains("--mode quarantine") && with.contains("W:\\, X:\\"), "{with}");
        assert!(!next_step(&u, None).contains("quarantine"));
    }

    #[test]
    fn a_ready_volume_is_not_listed() {
        let here = crate::paths::raw_path(std::env::temp_dir().as_os_str()).unwrap();
        // The temp folder's volume normally has a bin; whatever the answer, it is asked once for two paths on it.
        let both = unready(&[here.clone(), here]);
        assert!(both.len() <= 1, "{both:?}");
    }
}
