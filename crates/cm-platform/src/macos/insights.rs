//! macOS disk insights (parity 7.2/7.3, DIN-01..04): known large system and virtualization objects, overview card
//! directories and the old-downloads review.
//!
//! - Local snapshots: `/usr/bin/tmutil listlocalsnapshots /` through the trusted tool runner ([`super::cmd`]). Only
//!   their number is reported: APFS does not report the space a snapshot holds, so `logical_bytes` stays null with a
//!   note, never 0 and never a sum of file sizes. No snapshot is ever deleted or thinned (the guidance names
//!   `tmutil deletelocalsnapshots` as text for the person to run).
//! - Virtual machines and containers (Docker Desktop, UTM, Parallels, VMware Fusion, Lima, Colima, OrbStack): fixed,
//!   documented locations below the home folder, sized by what the filesystem allocated (`st_blocks * 512`, basis
//!   `allocated`), since their disk images are sparse. UTM's VMs live in its app container, which macOS privacy
//!   controls guard as another app's data: it is reported `permission_required`, never opened.
//! - Device backups (`~/Library/Application Support/MobileSync/Backup`, guarded by Full Disk Access): a denied read
//!   (`EPERM`/`EACCES`) is reported `permission_required`; nothing is retried or probed.
//! - Swap: `sysctl vm.swapusage` (`struct xsw_usage`). Purgeable space: `volumeAvailableCapacityForImportantUsage`
//!   minus `volumeAvailableCapacity` of the startup volume, labelled `estimate`.
//! - Old downloads: the Downloads folder the catalog resolver uses (so `CM_RESOLVE_HOME` / `CM_RESOLVE_DOWNLOADS`
//!   redirect it into fixtures), a review list with nothing selected.
//!
//! Safety boundary: directory listings and `lstat` only (links are not followed, mount points not crossed, dataless
//! iCloud folders not listed and nothing materialized); the one external tool is a fixed read-only `tmutil` query.
//! Nothing here deletes, moves, shrinks or reconfigures anything.

use std::collections::HashSet;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::capability::CapabilityState;
use cm_core::insights::{CardKind, DownloadEntry, Insight, MacObject, Measured, MeasuredDir, OldDownloadsReview, SizeBasis};
use cm_core::path::RawPath;

use super::cmd::{Tool, ToolError, ToolOutput};
use super::ffi::cf;
use super::resolve::ResolverEnv;
use super::sys::{NoMaterialize, SF_DATALESS, errno};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "insights.overview",
    CapabilityState::Unverified,
    "macOS insights (local snapshot count via tmutil, virtual machine and container disks by allocated size, device backups, \
     swap, purgeable estimate, overview cards and the old-downloads review) are implemented read-only and tested only on a \
     hosted runner, not on a real Mac; gate G1 not passed",
)];

/// Upper bound of each allocated-size walk; past it the size is reported as not measured.
pub const WALK_LIMIT: usize = 200_000;
/// Upper bound of the Downloads listing.
pub const DOWNLOADS_LIMIT: usize = 20_000;
const TMUTIL: &str = "/usr/bin/tmutil";
const TMUTIL_TIMEOUT: Duration = Duration::from_secs(20);
/// Prefix of every note about a read macOS denied (docs/platforms/macos.md §6: the blocking layer cannot be told
/// from `EPERM`/`EACCES` alone).
pub const PERMISSION_REQUIRED: &str = "permission_required (permission_cause=undetermined)";

/// Owner folders below the home folder.
const DOCKER_DATA: &[&str] = &["Library", "Containers", "com.docker.docker", "Data", "vms", "0", "data"];
const ORBSTACK_DATA: &[&str] = &["Library", "Group Containers", "HUAQ24HBR6.dev.orbstack", "data", "data.img"];
const UTM_CONTAINER: &[&str] = &["Library", "Containers", "com.utmapp.UTM"];
const MOBILESYNC: &[&str] = &["Library", "Application Support", "MobileSync", "Backup"];

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |p, c| p.join(c))
}

fn raw(p: &Path) -> Option<RawPath> {
    RawPath::unix(p.as_os_str().as_bytes().to_vec()).ok()
}

fn denied(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EPERM | libc::EACCES))
}

/// The resolver's locations with the `CM_RESOLVE_*` fixture overrides applied.
pub fn resolver_env() -> ResolverEnv {
    ResolverEnv::from_os().with_overrides(|k| std::env::var_os(k)).0
}

// ---------------------------------------------------------------------------------------------------------------
// Allocated sizes.

/// Allocated bytes below `root` (the root included): a read-only walk that does not follow links, does not cross
/// into other volumes, does not list dataless (cloud-only) folders and counts each hard-linked file once. Returns
/// `(allocated bytes, files)`; `Err` with the reason shown to the user when the root or any folder below it cannot be
/// read (a partial sum is never reported as the size) or the walk passes `limit` entries.
pub(crate) fn allocated_size(root: &Path, limit: usize) -> Result<(u64, u64), String> {
    let _guard = NoMaterialize::enter().map_err(|e| format!("dataless-file policy could not be set ({})", super::sys::describe(e)))?;
    let top = std::fs::symlink_metadata(root).map_err(|e| read_error(root, &e))?;
    if top.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link; not followed", root.display()));
    }
    let dev = top.dev();
    let mut total = top.blocks().saturating_mul(512);
    let mut files = 0u64;
    if !top.is_dir() {
        return Ok((total, 1));
    }
    let mut seen = 0usize;
    let mut denied_dirs = 0usize;
    let mut linked: HashSet<(u64, u64)> = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if first => return Err(read_error(root, &e)),
            Err(e) if denied(&e) => {
                denied_dirs += 1;
                continue;
            }
            Err(e) => return Err(format!("{} could not be read ({e}); size not reported as partial", dir.display())),
        };
        first = false;
        for entry in rd {
            let entry = entry.map_err(|e| format!("{} could not be listed ({e}); size not reported as partial", dir.display()))?;
            seen += 1;
            if seen > limit {
                return Err(format!("More than {limit} entries; not measured, scan the folder in disk analysis"));
            }
            // `DirEntry::metadata` is `lstat`: a link is measured as itself, never followed.
            let Ok(m) = entry.metadata() else { continue };
            if m.dev() != dev {
                continue; // another volume mounted inside
            }
            if !m.is_dir() && m.nlink() > 1 && !linked.insert((m.dev(), m.ino())) {
                continue; // a hard link already counted
            }
            total = total.saturating_add(m.blocks().saturating_mul(512));
            if m.is_dir() {
                if m.st_flags() & SF_DATALESS == 0 {
                    stack.push(entry.path());
                }
            } else {
                files += 1;
            }
        }
    }
    if denied_dirs > 0 {
        return Err(format!("{PERMISSION_REQUIRED}: {denied_dirs} sub-folder(s) could not be read; size not reported as partial"));
    }
    Ok((total, files))
}

fn read_error(path: &Path, e: &std::io::Error) -> String {
    if denied(e) {
        format!("{PERMISSION_REQUIRED}: macOS denied reading {} ({e})", path.display())
    } else {
        format!("{} could not be read ({e})", path.display())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Local snapshots.

/// Snapshot names listed by `tmutil listlocalsnapshots`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshots {
    pub total: u64,
    pub time_machine: u64,
}

/// Parse `tmutil listlocalsnapshots <mount>` output: an optional `Snapshots for disk <mount>:` header (macOS 11+), then
/// one snapshot name per line. Any other line (spaces inside a name, an error text) makes the output unrecognized.
pub(crate) fn parse_tmutil(stdout: &str) -> Result<Snapshots, String> {
    let mut s = Snapshots { total: 0, time_machine: 0 };
    for line in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("Snapshots for ") && line.ends_with(':') {
            continue;
        }
        if line.chars().any(char::is_whitespace) || line.chars().any(char::is_control) {
            let shown: String = line.chars().take(80).collect();
            return Err(format!("unrecognized tmutil output line: {shown}"));
        }
        s.total += 1;
        if line.starts_with("com.apple.TimeMachine.") {
            s.time_machine += 1;
        }
    }
    Ok(s)
}

const SNAPSHOT_SIZE_NOTE: &str = "Size unavailable: APFS does not report the space a local snapshot holds (it shares blocks with the live volume and other snapshots), so only the number is shown";

/// The local-snapshots insight from a `tmutil` run.
pub(crate) fn snapshots_insight(run: Result<ToolOutput, ToolError>, observed_at: &str) -> Insight {
    let unknown = |why: String| Insight::macos(MacObject::LocalSnapshots, None, "/", raw(Path::new("/")), Measured::Unknown(why), None, None, observed_at);
    let out = match run {
        Ok(o) => o,
        Err(e) => return unknown(format!("Snapshots could not be listed: {e}")),
    };
    if out.timed_out {
        return unknown("Snapshots could not be listed: tmutil did not answer in time".into());
    }
    if out.exit_code != Some(0) || out.stdout_truncated {
        let err: String = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).take(160).collect();
        return unknown(format!("Snapshots could not be listed: tmutil exited with {:?} {err}", out.exit_code).trim_end().to_owned());
    }
    match parse_tmutil(&String::from_utf8_lossy(&out.stdout)) {
        Ok(s) => {
            let note = format!("{} Time Machine snapshot(s), {} other", s.time_machine, s.total - s.time_machine);
            let mut i = Insight::macos(MacObject::LocalSnapshots, None, "/", raw(Path::new("/")), Measured::Unknown(SNAPSHOT_SIZE_NOTE.into()), Some(s.total), Some(note), observed_at);
            i.title = format!("{} on the startup disk", i.title);
            i
        }
        Err(why) => unknown(format!("Snapshots could not be counted: {why}")),
    }
}

/// Count the startup volume's local snapshots (read-only `tmutil listlocalsnapshots /`).
pub fn local_snapshots(observed_at: &str) -> Insight {
    let run = Tool::new(TMUTIL).args(["listlocalsnapshots", "/"]).timeout(TMUTIL_TIMEOUT).output_caps(256 * 1024, 16 * 1024).run();
    snapshots_insight(run, observed_at)
}

// ---------------------------------------------------------------------------------------------------------------
// Swap and purgeable space.

/// `struct xsw_usage` from `<sys/sysctl.h>`: `u_int64_t xsu_total, xsu_avail, xsu_used; u_int32_t xsu_pagesize;
/// boolean_t xsu_encrypted;` (32 bytes, native endianness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SwapUsage {
    pub total: u64,
    pub avail: u64,
    pub used: u64,
    pub pagesize: u32,
    pub encrypted: bool,
}

pub(crate) const XSW_USAGE_LEN: usize = 32;

/// Decode the `vm.swapusage` value; `None` unless it is exactly one `xsw_usage`.
pub(crate) fn parse_swapusage(b: &[u8]) -> Option<SwapUsage> {
    if b.len() != XSW_USAGE_LEN {
        return None;
    }
    let u64_at = |o: usize| u64::from_ne_bytes(b[o..o + 8].try_into().expect("8 bytes"));
    let u32_at = |o: usize| u32::from_ne_bytes(b[o..o + 4].try_into().expect("4 bytes"));
    Some(SwapUsage { total: u64_at(0), avail: u64_at(8), used: u64_at(16), pagesize: u32_at(24), encrypted: u32_at(28) != 0 })
}

fn sysctl_bytes(name: &std::ffi::CStr) -> Result<Vec<u8>, i32> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: NUL-terminated name, a writable buffer of `len` bytes and its length; no new value is set.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return Err(errno());
    }
    Ok(buf[..len.min(buf.len())].to_vec())
}

pub(crate) fn swap_insight(value: Result<Vec<u8>, i32>, observed_at: &str) -> Insight {
    let path = Path::new("/private/var/vm");
    let (size, note) = match value.map(|b| parse_swapusage(&b)) {
        Ok(Some(s)) => (
            Measured::Bytes { bytes: s.total, basis: SizeBasis::SystemQuery },
            Some(format!("vm.swapusage: {} bytes used, {} bytes free of {} bytes{}", s.used, s.avail, s.total, if s.encrypted { ", encrypted" } else { "" })),
        ),
        Ok(None) => (Measured::Unknown("vm.swapusage returned an unexpected value".into()), None),
        Err(e) => (Measured::Unknown(format!("vm.swapusage could not be read ({})", super::sys::describe(e))), None),
    };
    Insight::macos(MacObject::Swap, None, "/private/var/vm", raw(path), size, None, note, observed_at)
}

pub fn swap(observed_at: &str) -> Insight {
    swap_insight(sysctl_bytes(c"vm.swapusage"), observed_at)
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// `const CFStringRef kCFURLVolumeAvailableCapacityKey;` (CFURL.h)
    static kCFURLVolumeAvailableCapacityKey: cf::CFTypeRef;
    /// `const CFStringRef kCFURLVolumeAvailableCapacityForImportantUsageKey;` (CFURL.h, macOS 10.13+)
    static kCFURLVolumeAvailableCapacityForImportantUsageKey: cf::CFTypeRef;
    /// `Boolean CFURLCopyResourcePropertyForKey(CFURLRef url, CFStringRef key, void *propertyValueTypeRefPtr, CFErrorRef *error);`
    fn CFURLCopyResourcePropertyForKey(url: cf::CFTypeRef, key: cf::CFTypeRef, value: *mut cf::CFTypeRef, error: *mut cf::CFTypeRef) -> cf::Boolean;
}

/// One integer volume resource value of `url`.
fn volume_number(url: &cf::CfRef, key: cf::CFTypeRef) -> Result<i64, String> {
    let mut value: cf::CFTypeRef = std::ptr::null();
    let mut error: cf::CFTypeRef = std::ptr::null();
    // SAFETY: live URL and key; both out-pointers are valid. A value or error returned is +1 and owned below.
    let ok = unsafe { CFURLCopyResourcePropertyForKey(url.as_ptr(), key, &mut value, &mut error) };
    // SAFETY: NULL or an owned +1 reference (Copy rule).
    let _error = unsafe { cf::CfRef::from_create(error) };
    // SAFETY: as above.
    let value = unsafe { cf::CfRef::from_create(value) };
    match (ok != 0, value) {
        // SAFETY: a live CoreFoundation object.
        (true, Some(v)) => match unsafe { cf::to_plist(v.as_ptr()) } {
            Ok(cf::Plist::Integer(n)) => Ok(n),
            _ => Err("the volume capacity is not a number".into()),
        },
        _ => Err("the volume did not report its capacity".into()),
    }
}

/// `(capacity for important use, available capacity)` of the volume holding `path`.
fn volume_capacities(path: &Path) -> Result<(i64, i64), String> {
    let url = cf::file_url(path.as_os_str().as_bytes(), true).ok_or("the volume URL could not be created")?;
    // SAFETY: immutable CoreFoundation string constants.
    let (important_key, available_key) = unsafe { (kCFURLVolumeAvailableCapacityForImportantUsageKey, kCFURLVolumeAvailableCapacityKey) };
    Ok((volume_number(&url, important_key)?, volume_number(&url, available_key)?))
}

pub(crate) fn purgeable_insight(capacities: Result<(i64, i64), String>, observed_at: &str) -> Insight {
    let (size, note) = match capacities {
        Ok((important, available)) if important >= 0 && available >= 0 => {
            let estimate = (important - available).max(0) as u64;
            let note = format!(
                "capacity for important use ({important} bytes) minus available capacity ({available} bytes) of the startup volume, as macOS reports them; the system decides what it purges and when"
            );
            (Measured::Bytes { bytes: estimate, basis: SizeBasis::Estimate }, Some(note))
        }
        Ok(_) => (Measured::Unknown("the volume reported a negative capacity".into()), None),
        Err(why) => (Measured::Unknown(why), None),
    };
    Insight::macos(MacObject::PurgeableSpace, None, "/", None, size, None, note, observed_at)
}

pub fn purgeable(observed_at: &str) -> Insight {
    purgeable_insight(volume_capacities(Path::new("/")), observed_at)
}

// ---------------------------------------------------------------------------------------------------------------
// Virtual machines, containers and device backups below the home folder.

struct Found<'a> {
    home: &'a Path,
    now: &'a str,
    items: Vec<Insight>,
}

impl Found<'_> {
    fn push(&mut self, object: MacObject, label: Option<&str>, path: &Path, size: Measured, count: Option<u64>, note: Option<String>) {
        let d = path.to_string_lossy();
        self.items.push(Insight::macos(object, label, &d, raw(path), size, count, note, self.now));
    }

    /// One disk image file, by allocated size (its logical size, the sparse maximum, goes in the note).
    fn image(&mut self, object: MacObject, label: Option<&str>, path: &Path) {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => self.push(object, label, path, Measured::Unknown(read_error(path, &e)), None, None),
            Ok(m) if m.is_file() => {
                let note = format!("Logical size {} bytes; the space allocated on disk is shown", m.len());
                self.push(object, label, path, Measured::Bytes { bytes: m.blocks().saturating_mul(512), basis: SizeBasis::Allocated }, None, Some(note));
            }
            Ok(_) => {}
        }
    }

    /// A data folder measured as a whole.
    fn folder(&mut self, object: MacObject, path: &Path) {
        if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            return;
        }
        let size = match allocated_size(path, WALK_LIMIT) {
            Ok((bytes, _)) => Measured::Bytes { bytes, basis: SizeBasis::Allocated },
            Err(why) => Measured::Unknown(why),
        };
        self.push(object, None, path, size, None, None);
    }

    /// Virtual machine bundles (`<parent>/*.<ext>`), one insight for the folder with their count.
    fn bundles(&mut self, object: MacObject, parent: &Path, ext: &str) {
        let rd = match std::fs::read_dir(parent) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => return,
            Err(e) => return self.push(object, None, parent, Measured::Unknown(read_error(parent, &e)), None, None),
        };
        let mut bundles: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
            .collect();
        if bundles.is_empty() {
            return;
        }
        bundles.sort();
        let mut total = 0u64;
        for b in &bundles {
            match allocated_size(b, WALK_LIMIT) {
                Ok((bytes, _)) => total = total.saturating_add(bytes),
                Err(why) => {
                    let name = b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    return self.push(object, None, parent, Measured::Unknown(format!("{name}: {why}")), Some(bundles.len() as u64), None);
                }
            }
        }
        self.push(object, None, parent, Measured::Bytes { bytes: total, basis: SizeBasis::Allocated }, Some(bundles.len() as u64), None);
    }

    fn virtual_machines(&mut self) {
        let home = self.home.to_path_buf();
        let docker = join(&home, DOCKER_DATA);
        self.image(MacObject::DockerDisk, Some("Docker.raw"), &docker.join("Docker.raw"));
        self.image(MacObject::DockerDisk, Some("Docker.qcow2"), &docker.join("Docker.qcow2"));
        self.image(MacObject::OrbStackDisk, None, &join(&home, ORBSTACK_DATA));
        let utm = join(&home, UTM_CONTAINER);
        if std::fs::symlink_metadata(&utm).is_ok_and(|m| m.is_dir()) {
            let why = format!(
                "{PERMISSION_REQUIRED}: UTM keeps its virtual machines in its app container, which macOS protects as another app's data; CleanManager does not open it. See their sizes in UTM or System Settings > General > Storage"
            );
            self.push(MacObject::UtmVms, None, &utm, Measured::Unknown(why), None, None);
        }
        self.bundles(MacObject::ParallelsVms, &home.join("Parallels"), "pvm");
        self.bundles(MacObject::VmwareVms, &home.join("Virtual Machines.localized"), "vmwarevm");
        self.bundles(MacObject::VmwareVms, &home.join("Virtual Machines"), "vmwarevm");
        self.folder(MacObject::LimaVms, &home.join(".lima"));
        self.folder(MacObject::ColimaVms, &home.join(".colima"));
    }

    fn device_backups(&mut self) {
        let root = join(self.home, MOBILESYNC);
        if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
            return;
        }
        let rd = match std::fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(e) if denied(&e) => {
                let why = format!(
                    "{PERMISSION_REQUIRED}: macOS privacy protection (Full Disk Access) guards device backups ({e}). Grant Full Disk Access to the app that runs CleanManager in System Settings > Privacy & Security > Full Disk Access, or see the backups in Finder > Manage Backups"
                );
                return self.push(MacObject::DeviceBackups, None, &root, Measured::Unknown(why), None, None);
            }
            Err(e) => return self.push(MacObject::DeviceBackups, None, &root, Measured::Unknown(read_error(&root, &e)), None, None),
        };
        let count = rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).count() as u64;
        let size = match allocated_size(&root, WALK_LIMIT) {
            Ok((bytes, _)) => Measured::Bytes { bytes, basis: SizeBasis::Allocated },
            Err(why) => Measured::Unknown(why),
        };
        self.push(MacObject::DeviceBackups, None, &root, size, Some(count), None);
    }
}

/// Known objects below `home` (virtual machines, containers, device backups).
pub(crate) fn home_objects(home: &Path, now: &str) -> Vec<Insight> {
    let mut f = Found { home, now, items: Vec::new() };
    f.virtual_machines();
    f.device_backups();
    f.items
}

/// Read-only list of known large objects on this Mac, largest known size first.
pub fn list() -> Vec<Insight> {
    let now = cm_core::time::now_utc();
    let mut items = vec![local_snapshots(&now), swap(&now), purgeable(&now)];
    if let Some(home) = resolver_env().home {
        items.extend(home_objects(&home, &now));
    }
    cm_core::insights::sort(&mut items);
    items
}

// ---------------------------------------------------------------------------------------------------------------
// Overview cards and old downloads (parity 7.3, DIN-03/04).

/// Files below `root` (subfolders up to 3 levels; links not followed; dataless folders not listed), at most
/// [`DOWNLOADS_LIMIT`] entries. `Err` when the root itself cannot be read; the note says why the list may be incomplete.
pub(crate) fn download_entries(root: &Path) -> Result<(Vec<DownloadEntry>, Option<String>), std::io::Error> {
    let _guard = NoMaterialize::enter().map_err(std::io::Error::from_raw_os_error)?;
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut unreadable = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if depth == 0 => return Err(e),
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > DOWNLOADS_LIMIT {
                return Ok((out, Some(format!("Listing stopped after {DOWNLOADS_LIMIT} entries; open Downloads in disk analysis for the rest"))));
            }
            let Ok(m) = e.metadata() else { continue };
            if m.file_type().is_symlink() {
                continue;
            }
            if m.is_dir() {
                if depth < 3 && m.st_flags() & SF_DATALESS == 0 {
                    stack.push((e.path(), depth + 1));
                }
                continue;
            }
            if let Some(path) = raw(&e.path()) {
                out.push(DownloadEntry { path, logical_bytes: m.len(), modified_unix: m.mtime() });
            }
        }
    }
    Ok((out, (unreadable > 0).then(|| format!("{unreadable} folder(s) could not be read"))))
}

/// The old-downloads review of `downloads` (`None`: not resolved).
pub(crate) fn old_downloads_at(downloads: Option<&Path>, days: u32) -> OldDownloadsReview {
    let now = cm_core::time::now_utc();
    let now_unix = cm_core::plan::parse_utc(&now).unwrap_or(0);
    let review = |root: Option<RawPath>, entries: Vec<DownloadEntry>, note: Option<String>| cm_core::insights::old_downloads(root, entries, now_unix, days, &now, note);
    let Some(dir) = downloads else {
        return review(None, Vec::new(), Some("The Downloads folder could not be resolved".into()));
    };
    match download_entries(dir) {
        Ok((entries, note)) => review(raw(dir), entries, note),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => review(None, Vec::new(), Some(format!("No Downloads folder at {}", dir.display()))),
        Err(e) => {
            let note = if denied(&e) {
                format!(
                    "{PERMISSION_REQUIRED}: macOS denied listing {} ({e}). Allow the app that runs CleanManager to access the Downloads folder in System Settings > Privacy & Security > Files and Folders",
                    dir.display()
                )
            } else {
                format!("{} could not be read ({e})", dir.display())
            };
            let mut r = review(raw(dir), Vec::new(), Some(note));
            r.total_logical_bytes = None; // unknown, not 0
            r
        }
    }
}

/// The old-downloads review of this user's Downloads folder (the resolver's, with its fixture overrides).
pub fn old_downloads(days: u32) -> OldDownloadsReview {
    old_downloads_at(resolver_env().downloads.as_deref(), days)
}

/// Fixed tool directories below the home folder for the developer-tool and model-library cards.
const CARD_DIRS: &[(CardKind, &str, &[&str])] = {
    use cm_core::insights::CardKind::{DeveloperTools as D, ModelLibraries as M};
    &[
        (D, "Xcode DerivedData", &["Library", "Developer", "Xcode", "DerivedData"]),
        (D, "Xcode device support", &["Library", "Developer", "Xcode", "iOS DeviceSupport"]),
        (D, "Simulator devices", &["Library", "Developer", "CoreSimulator", "Devices"]),
        (D, "Simulator caches", &["Library", "Developer", "CoreSimulator", "Caches"]),
        (D, "Homebrew cache", &["Library", "Caches", "Homebrew"]),
        (D, "Cargo registry", &[".cargo", "registry"]),
        (D, "Rust toolchains", &[".rustup", "toolchains"]),
        (D, "npm cache", &[".npm"]),
        (D, "pip cache", &["Library", "Caches", "pip"]),
        (D, "uv cache", &[".cache", "uv"]),
        (D, "Gradle caches", &[".gradle", "caches"]),
        (D, "Maven repository", &[".m2", "repository"]),
        (D, "Yarn cache", &["Library", "Caches", "Yarn"]),
        (D, "pnpm store", &["Library", "pnpm", "store"]),
        (D, "Go module cache", &["go", "pkg", "mod"]),
        (D, "CocoaPods cache", &["Library", "Caches", "CocoaPods"]),
        (M, "Ollama models", &[".ollama", "models"]),
        (M, "Hugging Face cache", &[".cache", "huggingface"]),
        (M, "LM Studio models", &[".lmstudio", "models"]),
        (M, "LM Studio models (old location)", &[".cache", "lm-studio", "models"]),
    ]
};

/// Existing card directories below `home`, each measured by allocated size (unknown with a reason when it cannot be).
pub(crate) fn card_dirs_in(home: &Path) -> Vec<MeasuredDir> {
    let mut out = Vec::new();
    for (card, label, rest) in CARD_DIRS {
        let dir = join(home, rest);
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let Some(path) = raw(&dir) else { continue };
        let size = match allocated_size(&dir, WALK_LIMIT) {
            Ok((bytes, _)) => Measured::Bytes { bytes, basis: SizeBasis::Allocated },
            Err(why) => Measured::Unknown(why),
        };
        out.push(MeasuredDir { card: *card, label: (*label).into(), path, size, observed_at: cm_core::time::now_utc() });
    }
    out
}

/// Card directories of this user (the resolver's home, with its fixture override).
pub fn card_dirs() -> Vec<MeasuredDir> {
    resolver_env().home.map(|h| card_dirs_in(&h)).unwrap_or_default()
}

#[cfg(test)]
mod tests;
