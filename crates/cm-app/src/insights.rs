//! Space-insight overview for the CLI and the desktop (parity 7.3, DIN-03/04).
//!
//! [`overview_wire`] is pure: it turns the known objects, the old-downloads review and the measured tool directories
//! into the `insight_overview` record body (cards with a known size or unknown, each with its observation time). The
//! CLI and the desktop call [`overview`] / [`old_downloads`], which gather the inputs on Windows and macOS. Nothing here
//! selects, plans or changes anything: old downloads are a review list with no item selected.

use cm_core::insights::{Insight, MeasuredDir, OLD_DOWNLOADS_DAYS, OldDownloadsReview, overview_cards};

/// `insight_overview` body from already gathered inputs.
pub fn overview_wire(known: &[Insight], downloads: &OldDownloadsReview, dirs: &[MeasuredDir], generated_at: &str) -> serde_json::Value {
    let cards = overview_cards(known, downloads, dirs);
    let mut v = serde_json::json!({
        "generated_at": generated_at,
        "cards": cards.iter().map(|c| c.to_wire()).collect::<Vec<_>>(),
        // An unreadable or pending folder has no known file count: null, never 0.
        "old_downloads": {"older_than_days": downloads.older_than_days, "item_count": downloads.known_item_count(),
                          "total_logical_bytes": downloads.total_logical_bytes.map(|b| b.to_string()), "observed_at": downloads.observed_at},
    });
    if let Some(cause) = downloads.note.as_deref().and_then(cm_core::permission::cause_in) {
        v["old_downloads"]["permission_cause"] = cause.as_str().into();
    }
    v
}

/// `old_downloads_review` body, at most `limit` items (largest first; the totals cover every item).
pub fn old_downloads_wire(review: &OldDownloadsReview, limit: usize) -> serde_json::Value {
    let mut w = review.to_wire();
    if let Some(items) = w["items"].as_array_mut() {
        items.truncate(limit);
    }
    w["shown"] = w["items"].as_array().map_or(0, Vec::len).into();
    w
}

/// Default threshold, re-exported for callers.
pub const DEFAULT_DAYS: u32 = OLD_DOWNLOADS_DAYS;

/// Overview of this machine (Windows): known objects, Downloads older than `days`, tool directories.
#[cfg(windows)]
pub fn overview(days: u32) -> serde_json::Value {
    let known = cm_platform::windows::insights::list();
    let downloads = cm_platform::windows::insights::old_downloads(days);
    let dirs = cm_platform::windows::insights::card_dirs();
    overview_wire(&known, &downloads, &dirs, &cm_core::time::now_utc())
}

/// Old-downloads review of this machine (Windows).
#[cfg(windows)]
pub fn old_downloads(days: u32, limit: usize) -> serde_json::Value {
    old_downloads_wire(&cm_platform::windows::insights::old_downloads(days), limit)
}

/// Overview of this Mac: known objects (snapshot count, virtual machines and containers, device backups, swap,
/// purgeable estimate), Downloads older than `days` and tool directories, all below the resolver's home (so the
/// `CM_RESOLVE_*` fixture overrides apply).
#[cfg(target_os = "macos")]
pub fn overview(days: u32) -> serde_json::Value {
    let known = cm_platform::macos::insights::list();
    let downloads = cm_platform::macos::insights::old_downloads(days);
    let dirs = cm_platform::macos::insights::card_dirs();
    overview_wire(&known, &downloads, &dirs, &cm_core::time::now_utc())
}

/// Old-downloads review of this Mac's Downloads folder (the resolver's).
#[cfg(target_os = "macos")]
pub fn old_downloads(days: u32, limit: usize) -> serde_json::Value {
    old_downloads_wire(&cm_platform::macos::insights::old_downloads(days), limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_core::insights::{DownloadEntry, old_downloads as review};
    use cm_core::path::RawPath;

    #[test]
    fn overview_and_review_wire() {
        let p = |s: &str| RawPath::windows(s.encode_utf16().collect()).unwrap();
        let now = 1_800_000_000;
        let entries = (0..5).map(|i| DownloadEntry { path: p(&format!(r"C:\D\f{i}.bin")), logical_bytes: 100 + i, modified_unix: now - 100 * 86_400 }).collect();
        let r = review(Some(p(r"C:\D")), entries, now, 90, "2026-10-15T00:00:00Z", None);
        let o = overview_wire(&[], &r, &[], "2026-10-15T00:00:01Z");
        assert_eq!(o["cards"][0]["kind"], "old_downloads");
        assert_eq!(o["cards"][0]["logical_bytes"], "510");
        assert_eq!(o["cards"][0]["observed_at"], "2026-10-15T00:00:00Z");
        let w = old_downloads_wire(&r, 2);
        assert_eq!((w["shown"].as_u64(), w["item_count"].as_u64()), (Some(2), Some(5)));
        assert_eq!(w["items"][0]["logical_bytes"], "104");
        assert!(w["items"].as_array().unwrap().iter().all(|i| i["selected"] == false));
        assert!(o["old_downloads"].get("permission_cause").is_none() && w.get("permission_cause").is_none(), "no cause, no field");

        // Unknown folder (pending prompt): no file count and no size, the cause as a field.
        let note = "permission_required (permission_cause=privacy_prompt_pending): macOS did not answer within 5 s";
        let mut pending = review(Some(p(r"C:\D")), vec![], now, 90, "2026-10-15T00:00:00Z", Some(note.into()));
        pending.total_logical_bytes = None;
        let o = overview_wire(&[], &pending, &[], "2026-10-15T00:00:01Z");
        assert!(o["old_downloads"]["item_count"].is_null() && o["old_downloads"]["total_logical_bytes"].is_null(), "{o}");
        assert_eq!(o["old_downloads"]["permission_cause"], "privacy_prompt_pending");
        assert!(o["cards"][0]["count"].is_null() && o["cards"][0]["permission_cause"] == "privacy_prompt_pending", "{o}");
        let w = old_downloads_wire(&pending, 10);
        assert!(w["item_count"].is_null() && w["permission_cause"] == "privacy_prompt_pending", "{w}");
    }
}
