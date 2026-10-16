//! The single source of macOS object identity text. Scan rows, hard-deny roots, plans and later
//! executors must all spell `(volume_id, file_id)` through these functions so identities compare
//! byte for byte:
//!
//! - `volume_id`: `uuid:<8-4-4-4-12 upper-case hex>` when the volume UUID could be read and
//!   matched to the object's device, otherwise `fsid:<16 lower-case hex>` (not stable across
//!   remounts; such identities are weak).
//! - `file_id`: `inode64:<16 lower-case hex>` of `st_ino` (APFS/HFS+ file IDs are persistent).

// Some helpers wait for the later macOS work packages (executors, uninstall).
#![allow(dead_code)]

use super::sys::{Fd, volume_of};

const FILE_ID_PREFIX: &str = "inode64:";

/// `file_id` of an inode number.
pub(crate) fn file_id(ino: u64) -> String {
    format!("{FILE_ID_PREFIX}{ino:016x}")
}

/// Inode number of a `file_id` written by [`file_id`]; `None` for any other spelling.
pub(crate) fn parse_file_id(id: &str) -> Option<u64> {
    let hex = id.strip_prefix(FILE_ID_PREFIX)?;
    (hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then(|| u64::from_str_radix(hex, 16).ok()).flatten()
}

/// `volume_id` of a volume UUID.
pub(crate) fn uuid_volume_id(u: &[u8; 16]) -> String {
    let h: Vec<String> = u.iter().map(|b| format!("{b:02X}")).collect();
    format!("uuid:{}-{}-{}-{}-{}", h[0..4].concat(), h[4..6].concat(), h[6..8].concat(), h[8..10].concat(), h[10..16].concat())
}

/// `volume_id` fallback from `statfs.f_fsid` when no UUID is available.
pub(crate) fn fsid_volume_id(fsid: [i32; 2]) -> String {
    format!("fsid:{:08x}{:08x}", fsid[0] as u32, fsid[1] as u32)
}

/// `volume_id` of the volume holding `fd` (whose `st_dev` is `dev`), as the scan backends record it.
pub(crate) fn volume_id(fd: &Fd, dev: i32) -> Result<String, i32> {
    volume_of(fd, dev).map(|v| v.id)
}

/// Whether a `volume_id` is UUID-based (stable across remounts).
pub(crate) fn is_stable_volume_id(id: &str) -> bool {
    id.starts_with("uuid:")
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    use cm_core::path::RawPath;
    use cm_core::scan::{EntryRow, ObjectRow, ScanBackend, ScanOptions, ScanSink, SkippedRow};

    use super::*;
    use crate::macos::posix::PosixBackend;
    use crate::macos::testfs::Fixture;

    #[test]
    fn encodings_are_byte_identical_to_the_scan_format() {
        assert_eq!(file_id(0x1234), "inode64:0000000000001234");
        assert_eq!(file_id(u64::MAX), format!("inode64:{:016x}", u64::MAX));
        assert_eq!(parse_file_id("inode64:00000000000000ff"), Some(255));
        for bad in ["inode64:FF", "inode64:00000000000000FF", "ino:00000000000000ff", "inode64:00000000000000fg"] {
            assert_eq!(parse_file_id(bad), None, "{bad}");
        }
        let u = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        assert_eq!(uuid_volume_id(&u), "uuid:01234567-89AB-CDEF-0123-456789ABCDEF");
        assert_eq!(fsid_volume_id([0x0100_0004, -1]), "fsid:01000004ffffffff");
        assert!(is_stable_volume_id(&uuid_volume_id(&u)) && !is_stable_volume_id("fsid:01000004ffffffff"));
    }

    #[derive(Default)]
    struct Rows(Vec<ObjectRow>, Vec<EntryRow>);

    impl ScanSink for Rows {
        fn write(&mut self, o: &[ObjectRow], e: &[EntryRow], _: &[SkippedRow]) -> Result<(), String> {
            self.0.extend_from_slice(o);
            self.1.extend_from_slice(e);
            Ok(())
        }
        fn cancel_requested(&mut self) -> bool {
            false
        }
    }

    #[test]
    fn matches_the_walker_for_a_fixture() {
        let fx = Fixture::new("ids");
        let file = fx.write("a.txt", b"identity");
        let root = RawPath::unix(fx.path().as_os_str().as_bytes().to_vec()).unwrap();
        let mut rows = Rows::default();
        PosixBackend.scan(&root, &mut rows, &ScanOptions::default());
        let entry = rows.1.iter().find(|e| e.name.raw_bytes() == b"a.txt").expect("file listed");
        let object = rows.0.iter().find(|o| o.object_id == entry.object_id).unwrap();
        let meta = std::fs::symlink_metadata(&file).unwrap();
        assert_eq!(object.file_id, file_id(meta.ino()));
        let dir = Fd::open_dir(&CString::new(fx.path().as_os_str().as_bytes()).unwrap()).unwrap();
        assert_eq!(object.volume_id, volume_id(&dir, meta.dev() as i32).unwrap());
    }
}
