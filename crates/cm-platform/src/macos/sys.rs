//! Thin libc layer shared by the macOS scan backends: owned descriptors, no-follow opens,
//! `fstat`/`fstatat`, raw-byte directory listing, volume identity and the per-thread dataless
//! materialization policy. Nothing here reads file content or follows a symbolic link.

use std::ffi::{CStr, CString};

use libc::c_int;

/// `SF_DATALESS` from `<sys/stat.h>`: the object's content (or directory listing) lives with a
/// provider and accessing it may materialize it. Not exported by the `libc` crate.
pub(crate) const SF_DATALESS: u32 = 0x4000_0000;

/// Every directory open: read-only, must be a directory, never a symlink, never inherited.
const DIR_FLAGS: c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Current `errno`, never 0 (a failing call that left errno unset becomes `EIO`).
pub(crate) fn errno() -> i32 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(0) | None => libc::EIO,
        Some(e) => e,
    }
}

fn raw_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn clear_errno() {
    // SAFETY: `__error` returns the calling thread's errno slot, valid for the thread's lifetime.
    unsafe { *libc::__error() = 0 };
}

pub(crate) fn describe(e: i32) -> String {
    std::io::Error::from_raw_os_error(e).to_string()
}

/// An owned file descriptor, closed on drop.
#[derive(Debug)]
pub(crate) struct Fd(c_int);

impl Fd {
    /// Open the scan root: a directory that is not itself a symlink. Intermediate components
    /// are resolved by the kernel as usual; only the user-selected root is opened by path.
    pub(crate) fn open_dir(path: &CStr) -> Result<Fd, i32> {
        // SAFETY: NUL-terminated path; no mode argument is needed without O_CREAT.
        let fd = unsafe { libc::open(path.as_ptr(), DIR_FLAGS) };
        if fd < 0 { Err(errno()) } else { Ok(Fd(fd)) }
    }

    /// Open a directory, following links in every component including the last. Only for the
    /// protection layer, which identifies an object's parent the way the kernel resolves it;
    /// the object itself is still examined with `stat_child` (no follow).
    pub(crate) fn open_dir_following(path: &CStr) -> Result<Fd, i32> {
        // SAFETY: NUL-terminated path; no mode argument is needed without O_CREAT.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if fd < 0 { Err(errno()) } else { Ok(Fd(fd)) }
    }

    /// Open one child directory relative to this directory without following a symlink.
    pub(crate) fn open_child_dir(&self, name: &CStr) -> Result<Fd, i32> {
        // SAFETY: valid directory fd and NUL-terminated single-component name.
        let fd = unsafe { libc::openat(self.0, name.as_ptr(), DIR_FLAGS) };
        if fd < 0 { Err(errno()) } else { Ok(Fd(fd)) }
    }

    /// A fresh open file description of this same directory (own offset, starts at 0), so a
    /// listing never depends on how far an earlier listing advanced a shared offset.
    pub(crate) fn reopen_self(&self) -> Result<Fd, i32> {
        self.open_child_dir(c".")
    }

    /// A second descriptor of the same open file description (`F_DUPFD_CLOEXEC`), owned separately: a worker thread can
    /// keep using it after this one is closed, and its number is never reused under it.
    pub(crate) fn duplicate(&self) -> Result<Fd, i32> {
        // SAFETY: valid fd; F_DUPFD_CLOEXEC takes the lowest acceptable new descriptor number.
        let fd = unsafe { libc::fcntl(self.0, libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 { Err(errno()) } else { Ok(Fd(fd)) }
    }

    pub(crate) fn raw(&self) -> c_int {
        self.0
    }

    pub(crate) fn stat(&self) -> Result<libc::stat, i32> {
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and out-pointer.
        if unsafe { libc::fstat(self.0, &mut st) } != 0 { Err(errno()) } else { Ok(st) }
    }

    /// Metadata of `name` inside this directory; a symlink reports itself, never its target.
    pub(crate) fn stat_child(&self, name: &[u8]) -> Result<libc::stat, i32> {
        let name = CString::new(name).map_err(|_| libc::EINVAL)?;
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid directory fd, NUL-terminated name and out-pointer.
        if unsafe { libc::fstatat(self.0, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            Err(errno())
        } else {
            Ok(st)
        }
    }

    pub(crate) fn statfs(&self) -> Result<libc::statfs, i32> {
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and out-pointer.
        if unsafe { libc::fstatfs(self.0, &mut sfs) } != 0 { Err(errno()) } else { Ok(sfs) }
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: this value owns the descriptor and closes it exactly once.
        unsafe { libc::close(self.0) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Directory,
    Symlink,
    /// FIFO, socket, device: recorded, never opened.
    Other,
}

/// Metadata of one directory entry, from `fstatat` or a complete `getattrlistbulk` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meta {
    pub kind: Kind,
    pub dev: i32,
    pub ino: u64,
    pub size: Option<u64>,
    pub alloc: Option<u64>,
    pub nlink: Option<u32>,
    pub mtime: Option<i64>,
    pub flags: u32,
    /// The listing reports another file system mounted on this directory.
    pub mount: bool,
    /// Block sharing of a file (`ObjectRow::shared_blocks`); `None` from `fstat`, which has no
    /// such field.
    pub shared_blocks: Option<bool>,
}

impl Meta {
    pub(crate) fn from_stat(st: &libc::stat) -> Meta {
        let kind = match st.st_mode & libc::S_IFMT {
            libc::S_IFREG => Kind::File,
            libc::S_IFDIR => Kind::Directory,
            libc::S_IFLNK => Kind::Symlink,
            _ => Kind::Other,
        };
        Meta {
            kind,
            dev: st.st_dev,
            ino: st.st_ino,
            size: u64::try_from(st.st_size).ok(),
            alloc: u64::try_from(st.st_blocks).ok().and_then(|b| b.checked_mul(512)),
            nlink: Some(u32::from(st.st_nlink)),
            mtime: Some(st.st_mtime),
            flags: st.st_flags,
            mount: false,
            shared_blocks: None,
        }
    }
}

/// One listed name. A per-entry failure is kept as its errno, never as a zero-sized object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub name: Vec<u8>,
    pub meta: Result<Meta, i32>,
}

struct DirStream(*mut libc::DIR);

impl Drop for DirStream {
    fn drop(&mut self) {
        // SAFETY: the stream came from fdopendir and is closed exactly once (closing its fd).
        unsafe { libc::closedir(self.0) };
    }
}

/// Raw names of a directory (without `.`/`..`), read through a fresh descriptor of `dir`.
pub(crate) fn read_names(dir: &Fd) -> Result<Vec<Vec<u8>>, i32> {
    let own = dir.reopen_self()?;
    // SAFETY: valid directory fd; on success the stream owns it.
    let dirp = unsafe { libc::fdopendir(own.raw()) };
    if dirp.is_null() {
        return Err(errno());
    }
    std::mem::forget(own); // now owned (and closed) by the stream
    let stream = DirStream(dirp);
    let mut names = Vec::new();
    loop {
        clear_errno();
        // SAFETY: valid stream; the returned record stays valid until the next readdir call.
        let ent = unsafe { libc::readdir(stream.0) };
        if ent.is_null() {
            return match raw_errno() {
                0 => Ok(names),
                e => Err(e),
            };
        }
        // SAFETY: non-null record returned by readdir for this stream.
        let ent = unsafe { &*ent };
        let len = usize::from(ent.d_namlen).min(ent.d_name.len());
        let name: Vec<u8> = ent.d_name[..len].iter().map(|&c| c as u8).collect();
        if name.is_empty() || name == b"." || name == b".." {
            continue;
        }
        names.push(name);
    }
}

/// `readdir` + `fstatat(AT_SYMLINK_NOFOLLOW)` listing used by `mac.posix` and as the
/// per-directory fallback of `mac.bulk`.
pub(crate) fn list_posix(dir: &Fd) -> Result<Vec<Record>, i32> {
    let names = read_names(dir)?;
    Ok(names
        .into_iter()
        .map(|name| {
            let meta = dir.stat_child(&name).map(|st| Meta::from_stat(&st));
            Record { name, meta }
        })
        .collect())
}

/// Volume facts of the scan root.
#[derive(Debug, Clone)]
pub(crate) struct Volume {
    /// `f_fstypename`, e.g. `apfs`, `hfs`, `msdos`, `smbfs`.
    pub filesystem: String,
    /// `uuid:<UUID>` when the volume UUID could be read and matched to the root's device,
    /// otherwise `fsid:<hex>` (not stable across reboots or remounts); spelled by `ids`.
    pub id: String,
    pub has_uuid: bool,
}

fn c_chars_to_bytes(chars: &[libc::c_char]) -> Vec<u8> {
    chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect()
}

pub(crate) fn volume_of(root: &Fd, dev: i32) -> Result<Volume, i32> {
    let sfs = root.statfs()?;
    let filesystem = String::from_utf8_lossy(&c_chars_to_bytes(&sfs.f_fstypename)).into_owned();
    let mount_on = c_chars_to_bytes(&sfs.f_mntonname);
    if let Some(uuid) = CString::new(mount_on).ok().and_then(|m| volume_uuid(&m, dev)) {
        return Ok(Volume { filesystem, id: super::ids::uuid_volume_id(&uuid), has_uuid: true });
    }
    // SAFETY: fsid_t is two i32 values with private field names; read them as plain data.
    let fsid: [i32; 2] = unsafe { std::ptr::read((&sfs.f_fsid as *const libc::fsid_t).cast::<[i32; 2]>()) };
    Ok(Volume { filesystem, id: super::ids::fsid_volume_id(fsid), has_uuid: false })
}

/// UUID of the volume mounted at `mount_on`, accepted only when that mount's device is the
/// root's device (the mount table may have changed since `fstatfs`).
fn volume_uuid(mount_on: &CStr, dev: i32) -> Option<[u8; 16]> {
    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_DEVID,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buf = [0u8; 128];
    // SAFETY: valid attrlist, writable buffer of the given size and NUL-terminated path.
    let rc = unsafe {
        libc::getattrlist(
            mount_on.as_ptr(),
            (&mut list as *mut libc::attrlist).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    if rc != 0 {
        return None;
    }
    parse_volume_uuid(&buf, dev)
}

/// Parse a `getattrlist` reply for `RETURNED_ATTRS | CMN_DEVID` + `VOL_INFO | VOL_UUID`:
/// `u32 length, attribute_set_t returned, dev_t devid, uuid_t uuid`. Bounds-checked.
pub(crate) fn parse_volume_uuid(buf: &[u8], expected_dev: i32) -> Option<[u8; 16]> {
    let u32_at = |o: usize| buf.get(o..o + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    let len = u32_at(0)? as usize;
    if len < 44 || len > buf.len() {
        return None;
    }
    let (common, vol) = (u32_at(4)?, u32_at(8)?);
    if common & libc::ATTR_CMN_DEVID == 0 || vol & libc::ATTR_VOL_UUID == 0 {
        return None;
    }
    let devid = u32_at(24)? as i32;
    let uuid: [u8; 16] = buf.get(28..44)?.try_into().ok()?;
    (devid == expected_dev && uuid != [0u8; 16]).then_some(uuid)
}

const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
const IOPOL_SCOPE_THREAD: c_int = 1;
const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;

// `<sys/resource.h>`; in libSystem, not exported by the `libc` crate.
unsafe extern "C" {
    fn getiopolicy_np(iotype: c_int, scope: c_int) -> c_int;
    fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
}

/// Turns dataless materialization off for the calling thread (TN3150) and restores the
/// previous policy on drop. The scan runs entirely on the thread that holds this guard.
pub(crate) struct NoMaterialize {
    previous: c_int,
}

impl NoMaterialize {
    pub(crate) fn enter() -> Result<NoMaterialize, i32> {
        // SAFETY: plain syscall wrappers with integer arguments.
        let previous = unsafe { getiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD) };
        if previous < 0 {
            return Err(errno());
        }
        // SAFETY: as above.
        let rc = unsafe {
            setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)
        };
        if rc != 0 { Err(errno()) } else { Ok(NoMaterialize { previous }) }
    }
}

impl Drop for NoMaterialize {
    fn drop(&mut self) {
        // SAFETY: restores the policy read in `enter` on the same thread.
        unsafe { setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD, self.previous) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(len: u32, common: u32, vol: u32, dev: i32, uuid: [u8; 16]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&len.to_ne_bytes());
        for v in [common, vol, 0, 0, 0] {
            b.extend_from_slice(&v.to_ne_bytes());
        }
        b.extend_from_slice(&dev.to_ne_bytes());
        b.extend_from_slice(&uuid);
        b
    }

    #[test]
    fn volume_uuid_reply_is_bounds_checked_and_device_matched() {
        let uuid = [0xAB; 16];
        let ok = reply(44, libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_DEVID, libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID, 7, uuid);
        assert_eq!(parse_volume_uuid(&ok, 7), Some(uuid));
        assert_eq!(parse_volume_uuid(&ok, 8), None, "another device's UUID is not used");
        assert_eq!(parse_volume_uuid(&ok[..40], 7), None, "short buffer");
        assert_eq!(parse_volume_uuid(&reply(400, libc::ATTR_CMN_DEVID, libc::ATTR_VOL_UUID, 7, uuid), 7), None, "length past buffer");
        assert_eq!(parse_volume_uuid(&reply(44, libc::ATTR_CMN_DEVID, 0, 7, uuid), 7), None, "UUID not returned");
        assert_eq!(parse_volume_uuid(&reply(44, libc::ATTR_CMN_DEVID, libc::ATTR_VOL_UUID, 7, [0; 16]), 7), None);
        assert_eq!(parse_volume_uuid(&[], 7), None);
    }
}
