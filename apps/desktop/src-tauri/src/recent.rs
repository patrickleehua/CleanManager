//! Scans started from this desktop, remembered so the overview can reopen them after a
//! restart. This is desktop UI state only (scan ID, root display, start time); the facts come
//! from the index store each time. The index has no "list scans" query yet.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentScan {
    pub scan_id: String,
    pub root_display: String,
    pub started_at: String,
}

pub struct RecentScans {
    file: Option<PathBuf>,
    pub items: Vec<RecentScan>,
}

impl RecentScans {
    pub fn load(file: Option<PathBuf>) -> Self {
        let items = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<Vec<RecentScan>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| crate::args::scan_id(&r.scan_id).is_ok())
            .take(MAX_RECENT)
            .collect();
        RecentScans { file, items }
    }

    pub fn push(&mut self, scan_id: &str, root_display: &str, started_at: &str) {
        self.items.retain(|r| r.scan_id != scan_id);
        self.items.insert(0, RecentScan { scan_id: scan_id.into(), root_display: root_display.into(), started_at: started_at.into() });
        self.items.truncate(MAX_RECENT);
        self.save();
    }

    pub fn root_of(&self, scan_id: &str) -> Option<String> {
        self.items.iter().find(|r| r.scan_id == scan_id).map(|r| r.root_display.clone())
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("json.tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.items)
            && std::fs::write(&tmp, bytes).is_ok()
        {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_dedupes_caps_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("cm-desktop-recent-{}", std::process::id()));
        let file = dir.join("recent-scans.json");
        let _ = std::fs::remove_dir_all(&dir);
        let mut r = RecentScans::load(Some(file.clone()));
        for i in 0..25 {
            r.push(&format!("s_{i}"), "C:\\x", "2026-10-15T00:00:00Z");
        }
        r.push("s_3", "C:\\y", "2026-10-15T00:00:01Z");
        assert_eq!(r.items.len(), MAX_RECENT);
        assert_eq!(r.items[0].scan_id, "s_3");
        let again = RecentScans::load(Some(file));
        assert_eq!(again.items, r.items);
        assert_eq!(again.root_of("s_3").as_deref(), Some("C:\\y"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_or_hostile_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("cm-desktop-recent-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("recent-scans.json");
        std::fs::write(&file, br#"[{"scan_id":"../../etc","root_display":"x","started_at":"y"}]"#).unwrap();
        assert!(RecentScans::load(Some(file.clone())).items.is_empty());
        std::fs::write(&file, b"not json").unwrap();
        assert!(RecentScans::load(Some(file)).items.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
