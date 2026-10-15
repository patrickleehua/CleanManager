//! Read-only listing of the current user's Recycle Bin (MTR-03), as an index-like view the core plans purges from.
//!
//! Each fixed local volume keeps `X:\$Recycle.Bin\<user SID>\`; a recycled object is `$R<suffix>` (file or folder) and
//! its record `$I<suffix>` holds the original path, size and deletion time. Only the current user's folder is read
//! (an elevated process could read other users' folders; it does not). Objects are identified by volume and file ID;
//! links and special objects inside a recycled folder are recorded but never followed. Nothing is opened for content
//! except the small `$I` records.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cm_core::analysis::{EntryFacts, IndexView, MemView};
use cm_core::capability::Platform;
use cm_core::manual_trash::BinItem;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::path::RawPath;

use super::wide;

/// One volume's bin folder for this user, as a view plus its items.
#[derive(Debug)]
pub struct BinListing {
    pub bin_dir: PathBuf,
    pub view: MemView,
    pub items: Vec<BinItem>,
    pub observed_at: String,
}

/// String form (`S-1-5-21-…`) of the current user's SID.
pub fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: current-process pseudo handle; valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: 8-byte aligned buffer of at least `needed` bytes.
    let ok = unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), needed, &mut needed) };
    // SAFETY: handle from OpenProcessToken, closed once.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return None;
    }
    // SAFETY: the call wrote a TOKEN_USER whose SID points inside `buf`; a SID is revision, sub-authority count,
    // 6-byte big-endian authority and that many little-endian 32-bit sub-authorities.
    unsafe {
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid as *const u8;
        if sid.is_null() {
            return None;
        }
        let revision = *sid;
        let count = *sid.add(1) as usize;
        let auth = (0..6).fold(0u64, |acc, i| (acc << 8) | u64::from(*sid.add(2 + i)));
        let mut s = format!("S-{revision}-{auth}");
        for i in 0..count {
            let p = sid.add(8 + 4 * i);
            s.push_str(&format!("-{}", u32::from_le_bytes([*p, *p.add(1), *p.add(2), *p.add(3)])));
        }
        Some(s)
    }
}

/// (original path, size, deletion FILETIME) from a `$I` record (format 1: Vista–8.1, format 2: Windows 10+).
pub fn parse_info(data: &[u8]) -> Option<(String, u64, u64)> {
    let u64_at = |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")));
    let (version, size, deleted) = (u64_at(0)?, u64_at(8)?, u64_at(16)?);
    let name: &[u8] = match version {
        1 => data.get(24..24 + 520)?,
        2 => {
            let chars = u32::from_le_bytes(data.get(24..28)?.try_into().ok()?) as usize;
            data.get(28..28 + chars.checked_mul(2)?)?
        }
        _ => return None,
    };
    let units: Vec<u16> = name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some((String::from_utf16_lossy(&units), size, deleted))
}

struct Builder {
    view: MemView,
    next: u64,
}

impl Builder {
    fn add(&mut self, parent: Option<u64>, path: &Path, name: RawPath) -> std::io::Result<u64> {
        let md = std::fs::symlink_metadata(path)?;
        let (volume_id, file_id) = super::super::directory::identity_of(&wide(path))?;
        let reparse = md.file_attributes() & 0x400 != 0;
        let kind = if reparse { ObjectKind::Reparse } else if md.is_dir() { ObjectKind::Directory } else if md.is_file() { ObjectKind::File } else { ObjectKind::Other };
        let id = self.next;
        self.next += 1;
        self.view.names.insert(id, name);
        self.view.parents.insert(id, parent);
        if let Some(p) = parent {
            self.view.kids.entry(p).or_default().push(id);
        }
        self.view.identities.insert((volume_id.clone(), file_id.clone()), id);
        self.view.facts.insert(id, EntryFacts {
            kind,
            boundary: reparse,
            object_id: id,
            logical: (kind == ObjectKind::File).then_some(md.len()),
            allocated: None,
            volume_id,
            file_id,
            quality: IdentityQuality::Strong,
            cloud: CloudState::NotCloud,
            modified_unix: Some(cm_core::time::filetime_to_unix(md.last_write_time())),
            link_count: None,
            skipped_here: 0,
        });
        Ok(id)
    }

    /// Descendants of a recycled folder (links are leaves; unreadable folders are marked).
    fn walk(&mut self, dir_entry: u64, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            self.view.facts.get_mut(&dir_entry).expect("added").skipped_here += 1;
            return;
        };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(name) = RawPath::windows(wide(Path::new(&e.file_name()))) else { continue };
            match self.add(Some(dir_entry), &path, name) {
                Ok(id) if self.view.facts[&id].kind == ObjectKind::Directory => self.walk(id, &path),
                Ok(_) => {}
                Err(_) => self.view.facts.get_mut(&dir_entry).expect("added").skipped_here += 1,
            }
        }
    }
}

/// Stable id of a recycled object from its identity (FNV-1a 64; selection only, the identity is re-verified).
fn item_id(volume_id: &str, file_id: &str) -> String {
    let h = format!("{volume_id}|{file_id}").bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("tb_{h:016x}")
}

/// List one bin folder (`X:\$Recycle.Bin\<SID>`, or a test folder laid out the same way).
pub fn list_dir(bin_dir: &Path) -> Result<BinListing, String> {
    let root_path = RawPath::windows(wide(bin_dir)).map_err(|e| e.to_string())?;
    let mut b = Builder { view: MemView { platform: Some(Platform::Windows), ..Default::default() }, next: 0 };
    let root = b.add(None, bin_dir, root_path).map_err(|e| format!("cannot open {}: {e}", bin_dir.display()))?;
    b.view.root = root;
    let observed_at = cm_core::time::now_utc();
    let mut names: Vec<String> = std::fs::read_dir(bin_dir).map_err(|e| e.to_string())?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut items = Vec::new();
    for name in names.iter().filter(|n| n.len() > 2 && n[..2].eq_ignore_ascii_case("$R")) {
        let suffix = &name[2..];
        let data_path = bin_dir.join(name);
        let Ok(data_name) = RawPath::windows(wide(Path::new(name))) else { continue };
        let Ok(data) = b.add(Some(root), &data_path, data_name) else { continue };
        let kind = b.view.facts[&data].kind;
        if kind == ObjectKind::Directory {
            b.walk(data, &data_path);
        }
        let info_name = names.iter().find(|n| n.len() > 2 && n[..2].eq_ignore_ascii_case("$I") && n[2..] == *suffix);
        let mut info = None;
        let mut record = None;
        if let Some(i) = info_name {
            let p = bin_dir.join(i);
            if let Ok(n) = RawPath::windows(wide(Path::new(i))) {
                info = b.add(Some(root), &p, n).ok();
            }
            record = std::fs::read(&p).ok().and_then(|d| parse_info(&d));
        }
        // Sizes and links inside a folder, from the view (index-like, no content read).
        let mut stack = vec![data];
        let (mut total, mut special, mut unread) = (Some(0u64), false, false);
        while let Some(e) = stack.pop() {
            let f = &b.view.facts[&e];
            unread |= f.skipped_here > 0;
            match f.kind {
                ObjectKind::Directory => stack.extend_from_slice(b.view.children(e)),
                ObjectKind::File => total = total.zip(f.logical).map(|(t, s)| t + s),
                _ => special = true,
            }
        }
        let refusal = if special {
            Some("contains a link or special object; empty it with Explorer (Recycle Bin, Delete) instead".to_string())
        } else if unread {
            Some("part of it could not be read; empty it with Explorer instead".to_string())
        } else {
            None
        };
        let f = &b.view.facts[&data];
        items.push(BinItem {
            trash_item_id: item_id(&f.volume_id, &f.file_id),
            original_path: record.as_ref().map(|r| r.0.clone()),
            deleted_at: record.as_ref().map(|r| cm_core::time::utc_from_unix(cm_core::time::filetime_to_unix(r.2))),
            kind,
            logical_bytes: total,
            data_entry: data,
            info_entry: info,
            refusal,
        });
    }
    Ok(BinListing { bin_dir: bin_dir.to_path_buf(), view: b.view, items, observed_at })
}

/// Bin folders of the current user on every fixed local volume that has one.
pub fn user_bin_dirs() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    let Some(sid) = current_user_sid() else { return Vec::new() };
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
        .filter(|root| {
            let z: Vec<u16> = wide(root).into_iter().chain(std::iter::once(0)).collect();
            // SAFETY: NUL-terminated root path. Fixed drives only (3 = DRIVE_FIXED).
            unsafe { GetDriveTypeW(z.as_ptr()) == 3 }
        })
        .map(|root| root.join("$Recycle.Bin").join(&sid))
        .filter(|d| d.is_dir())
        .collect()
}

/// Every listing of this user's bins, plus the folders that could not be read.
pub fn list_user_bins() -> (Vec<BinListing>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for d in user_bin_dirs() {
        match list_dir(&d) {
            Ok(l) => ok.push(l),
            Err(e) => failed.push((d, e)),
        }
    }
    (ok, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_v2(path: &str, size: u64, ft: u64) -> Vec<u8> {
        let units: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut d = Vec::new();
        d.extend_from_slice(&2u64.to_le_bytes());
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&ft.to_le_bytes());
        d.extend_from_slice(&(units.len() as u32).to_le_bytes());
        d.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        d
    }

    #[test]
    fn records_parse_in_both_formats() {
        let ft = 133_000_000_000_000_000u64;
        assert_eq!(parse_info(&info_v2("D:\\v\\x.mov", 9, ft)), Some(("D:\\v\\x.mov".into(), 9, ft)));
        let mut v1 = Vec::new();
        v1.extend_from_slice(&1u64.to_le_bytes());
        v1.extend_from_slice(&5u64.to_le_bytes());
        v1.extend_from_slice(&ft.to_le_bytes());
        let mut name = vec![0u8; 520];
        for (i, u) in "C:\\a.txt".encode_utf16().enumerate() {
            name[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        v1.extend(name);
        assert_eq!(parse_info(&v1), Some(("C:\\a.txt".into(), 5, ft)));
        assert_eq!(parse_info(&[0u8; 10]), None);
        assert_eq!(parse_info(&info_v2("x", 1, 1)[..30]), None, "truncated name");
        assert!(current_user_sid().is_some_and(|s| s.starts_with("S-1-")));
    }

    /// A folder laid out like a bin (synthetic `$R`/`$I` names in a temp directory; the real bin is not read).
    #[test]
    fn a_bin_folder_lists_items_with_their_records() {
        let dir = std::env::temp_dir().join(format!("cm-binlist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("$RAB12CD").join("sub")).unwrap();
        std::fs::write(dir.join("$RAB12CD").join("one.txt"), "111").unwrap();
        std::fs::write(dir.join("$RAB12CD").join("sub").join("two.txt"), "2222").unwrap();
        std::fs::write(dir.join("$IAB12CD"), info_v2("D:\\old", 7, 133_000_000_000_000_000)).unwrap();
        std::fs::write(dir.join("$RXY.mov"), vec![0u8; 900]).unwrap();
        std::fs::write(dir.join("$IXY.mov"), info_v2("D:\\x.mov", 900, 133_000_000_000_000_000)).unwrap();
        let l = list_dir(&dir).unwrap();
        assert_eq!(l.items.len(), 2);
        let folder = l.items.iter().find(|i| i.kind == ObjectKind::Directory).unwrap();
        assert_eq!(folder.original_path.as_deref(), Some("D:\\old"));
        assert_eq!(folder.logical_bytes, Some(7));
        assert!(folder.refusal.is_none() && folder.info_entry.is_some());
        assert!(folder.trash_item_id.starts_with("tb_"));
        let input = cm_core::manual_trash::ManualInput {
            plan_id: "mp", scope_id: None, scan_id: None, scan_revision: None, created_unix: 1_800_000_000, observed_at: &l.observed_at,
            policy_digest: cm_core::plan::v2::NO_RULES_DIGEST,
        };
        let plan = cm_core::manual_trash::build_purge_plan(&l.view, &l.items.iter().collect::<Vec<_>>(), &input).unwrap();
        assert_eq!(plan.body.items.len(), 4 + 1 + 2, "folder: two files, sub, itself and its record; file and its record");
        assert!(l.view.path(l.view.root()).display().ends_with(&dir.file_name().unwrap().to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
