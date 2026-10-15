//! Space-insight overview for the CLI and the desktop (parity 7.3, DIN-03/04).
//!
//! [`overview_wire`] is pure: it turns the known objects, the old-downloads review and the measured tool directories
//! into the `insight_overview` record body (cards with a known size or unknown, each with its observation time). The
//! desktop calls [`overview`] / [`old_downloads`], which gather the inputs on Windows. Nothing here selects, plans or
//! changes anything: old downloads are a review list with no item selected.

use cm_core::insights::{Insight, MeasuredDir, OLD_DOWNLOADS_DAYS, OldDownloadsReview, overview_cards};

/// `insight_overview` body from already gathered inputs.
pub fn overview_wire(known: &[Insight], downloads: &OldDownloadsReview, dirs: &[MeasuredDir], generated_at: &str) -> serde_json::Value {
    let cards = overview_cards(known, downloads, dirs);
    serde_json::json!({
        "generated_at": generated_at,
        "cards": cards.iter().map(|c| c.to_wire()).collect::<Vec<_>>(),
        "old_downloads": {"older_than_days": downloads.older_than_days, "item_count": downloads.item_count,
                          "total_logical_bytes": downloads.total_logical_bytes.map(|b| b.to_string()), "observed_at": downloads.observed_at},
    })
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
    }
}
