//! Moves to the user's Trash on macOS with identity checks before and after (parity D6).
//!
//! `-[NSFileManager trashItemAtURL:resultingItemURL:error:]` is the system's Trash interface (it picks the right Trash
//! for the volume and records Put Back); it works on a path. So:
//! 1. the target is verified like every other action (descriptor walk, namespace gate, live metadata, in use);
//! 2. an event-only, no-follow descriptor (`O_EVTONLY | O_NOFOLLOW`: no read access, no content) is held on the
//!    verified object and the name is looked at once more right before the call;
//! 3. afterwards, `fcntl(F_GETPATH)` on the held descriptor must name exactly the resulting item URL and `fstat` must
//!    still show the planned object. The move itself lists or examines nothing inside `~/.Trash` (TCC-protected).
//!
//! RESIDUAL RACE: the name can still be swapped between the last look and the system's own lookup. The item that went
//! to the Trash is then not the planned one; it stays recoverable there, and the outcome is `unknown` with put-back
//! guidance. A refused or impossible move (no Trash on the volume, `NSFeatureUnsupportedError`) is reported as
//! `trash_unavailable` and is never turned into a delete.
//!
//! Helpers of the manual-trash tool (`trash` CLI, MTR-01..05): [`trash_ready`] asks the system which Trash a path
//! would go to (`URLForDirectory:NSTrashDirectory`, `create:NO`; an error refuses the plan up front); [`open_holders`]
//! names the processes holding planned files open; [`list_trash_dir`] lists a Trash folder read-only for permanent
//! deletes inside it, skipping Finder's private `.DS_Store` record, so no original location is ever claimed, and
//! reports `EPERM`/`EACCES` as permission required, never as an empty Trash. Its first open, and the look at a
//! volume's Trash, go through [`privacy::bounded`]: while macOS shows a privacy prompt for that location the Trash is
//! reported `permission_required (permission_cause=privacy_prompt_pending)` after [`privacy::PROMPT_WAIT`], never as
//! empty (docs/platforms/macos.md §6).

use std::ffi::{CStr, CString, c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cm_core::analysis::{EntryFacts, IndexView, MemView};
use cm_core::capability::{CapabilityState, Platform};
use cm_core::execute::{ActionOutcome, ActionTarget, Recovery};
use cm_core::manual_trash::BinItem;
use cm_core::model::{CloudState, IdentityQuality, ObjectKind};
use cm_core::path::RawPath;

use super::{AtPath, MacActions, Stop, Verified, at_name, no_materialize, protected_flags, verify};
use crate::macos::ffi::objc::{AutoreleasePool, Bool, Id, Sel, class, file_url, file_url_path, ns_string_value, sel};
use crate::macos::{ids, privacy};
use crate::macos::procs::FdIndex;
use crate::macos::sys::{Fd, SF_DATALESS, describe, errno, read_names};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. The
/// executor's `execute.trash` row lives in `actions::CAPS`; this module backs the gated manual-trash tool.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "trash.manual",
    CapabilityState::Unverified,
    "Implemented on macOS: manual-selection plans move identity-verified files to the Trash through NSFileManager and confirm the result on a held descriptor; a volume without a Trash refuses the plan up front and nothing is ever deleted instead; whole folders are not moved in this build. The Trash list (~/.Trash and /Volumes/*/.Trashes/<uid>) never shows original locations (Finder's Put Back record is private) and reports permission_required where privacy protection blocks it; permanent deletes inside the Trash use the verified delete. Holders come from open descriptors of this user's processes; other users' processes cannot be inspected. Exercised only on a hosted CI runner, not on a reference Mac; gate G1 not passed",
)];

/// `NSCocoaErrorDomain` codes (`<Foundation/FoundationErrors.h>`).
const NS_FEATURE_UNSUPPORTED: isize = 3328;
const NS_FILE_READ_NO_PERMISSION: isize = 257;
const NS_FILE_WRITE_NO_PERMISSION: isize = 513;
const NS_FILE_WRITE_VOLUME_READ_ONLY: isize = 642;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's exact type before calling.
    fn objc_msgSend();
}

/// `objc_msgSend` as `unsafe extern "C" fn(id, SEL, args...) -> ret` (on arm64 it must be called with the method's
/// exact C type; it is not variadic).
macro_rules! msg_fn {
    (($($arg:ty),*) -> $ret:ty) => {
        // SAFETY: function pointers share one size and representation; the caller sends only selectors whose
        // implementation has exactly this C signature.
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $arg)*) -> $ret>(objc_msgSend as unsafe extern "C" fn())
        }
    };
}

#[cfg(target_arch = "aarch64")]
fn truthy(b: Bool) -> bool {
    b
}
#[cfg(not(target_arch = "aarch64"))]
fn truthy(b: Bool) -> bool {
    b != 0
}

/// Why the system refused to move an item to the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrashError {
    pub domain: String,
    pub code: isize,
    pub message: String,
}

impl TrashError {
    fn cocoa(&self) -> bool {
        self.domain == "NSCocoaErrorDomain"
    }

    /// Outcome code for a refusal that left the object in place.
    pub(crate) fn outcome_code(&self) -> &'static str {
        match self.code {
            NS_FILE_READ_NO_PERMISSION | NS_FILE_WRITE_NO_PERMISSION if self.cocoa() => "access_denied",
            c if self.domain == "NSPOSIXErrorDomain" && (c == libc::EACCES as isize || c == libc::EPERM as isize) => "access_denied",
            _ => "trash_unavailable",
        }
    }

    pub(crate) fn describe(&self) -> String {
        let why = match self.code {
            NS_FEATURE_UNSUPPORTED if self.cocoa() => "this volume has no Trash",
            NS_FILE_WRITE_VOLUME_READ_ONLY if self.cocoa() => "the volume is read-only",
            _ => "",
        };
        let sep = if why.is_empty() { "" } else { "; " };
        format!("{why}{sep}{} ({} {})", self.message, self.domain, self.code)
    }
}

/// `-[NSFileManager trashItemAtURL:resultingItemURL:error:]` for raw path bytes: the resulting item's path bytes, or
/// the system's error. Nothing else is attempted on failure.
pub(crate) fn move_to_trash(path: &[u8]) -> Result<Option<Vec<u8>>, TrashError> {
    let internal = |message: &str| TrashError { domain: "CleanManager".into(), code: 0, message: message.into() };
    let _pool = AutoreleasePool::new();
    let manager_class = class(c"NSFileManager").ok_or_else(|| internal("NSFileManager is not available"))?;
    // `+ (NSFileManager *)defaultManager;`
    let get_manager = msg_fn!(() -> Id);
    // SAFETY: class receiver; returns the shared (+0) instance.
    let manager = unsafe { get_manager(manager_class, sel(c"defaultManager")) };
    if manager.is_null() {
        return Err(internal("no default file manager"));
    }
    let url = file_url(path, false).ok_or_else(|| internal("cannot build a file URL for the path"))?;
    // `- (BOOL)trashItemAtURL:(NSURL *)url resultingItemURL:(NSURL **)outResultingURL error:(NSError **)error;`
    let trash_item = msg_fn!((Id, *mut Id, *mut Id) -> Bool);
    let (mut resulting, mut error): (Id, Id) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: live manager and URL; both out-pointers are valid and receive autoreleased objects (kept alive by `_pool`).
    let ok = unsafe { trash_item(manager, sel(c"trashItemAtURL:resultingItemURL:error:"), url.as_ptr(), &mut resulting, &mut error) };
    if truthy(ok) {
        // SAFETY: nil or the autoreleased NSURL the call returned, alive until `_pool` drops.
        return Ok(unsafe { file_url_path(resulting) });
    }
    if error.is_null() {
        return Err(internal("the move to the Trash failed without an error"));
    }
    // SAFETY: `error` is the live autoreleased NSError the call returned, alive until `_pool` drops.
    Err(unsafe { ns_error(error) })
}

/// The domain, code and description of an `NSError`, copied out.
///
/// # Safety
/// `error` must be a live `NSError`, and an autorelease pool must be in place on this thread.
unsafe fn ns_error(error: Id) -> TrashError {
    // `- (NSInteger)code;` / `- (NSErrorDomain)domain;` / `- (NSString *)localizedDescription;`
    let get_code = msg_fn!(() -> isize);
    let get_object = msg_fn!(() -> Id);
    // SAFETY: live NSError per the caller; the strings are copied before the enclosing pool drops.
    let (code, domain, message) = unsafe {
        (
            get_code(error, sel(c"code")),
            ns_string_value(get_object(error, sel(c"domain"))).unwrap_or_default(),
            ns_string_value(get_object(error, sel(c"localizedDescription"))).unwrap_or_default(),
        )
    };
    TrashError { domain, code, message }
}

/// `NSTrashDirectory` / `NSUserDomainMask` (`<Foundation/NSPathUtilities.h>`).
const NS_TRASH_DIRECTORY: usize = 102;
const NS_USER_DOMAIN_MASK: usize = 1;
#[cfg(target_arch = "aarch64")]
const NO: Bool = false;
#[cfg(not(target_arch = "aarch64"))]
const NO: Bool = 0;

/// The Trash the system would move `path` to: `-[NSFileManager URLForDirectory:NSTrashDirectory
/// inDomain:NSUserDomainMask appropriateForURL:<path> create:NO error:]` (the URL picks the volume). Nothing is created,
/// listed or moved. An error means the volume has no usable Trash for this user.
pub(crate) fn trash_dir_for(path: &[u8]) -> Result<Vec<u8>, TrashError> {
    let internal = |message: &str| TrashError { domain: "CleanManager".into(), code: 0, message: message.into() };
    let _pool = AutoreleasePool::new();
    let manager_class = class(c"NSFileManager").ok_or_else(|| internal("NSFileManager is not available"))?;
    // `+ (NSFileManager *)defaultManager;`
    let get_manager = msg_fn!(() -> Id);
    // SAFETY: class receiver; returns the shared (+0) instance.
    let manager = unsafe { get_manager(manager_class, sel(c"defaultManager")) };
    if manager.is_null() {
        return Err(internal("no default file manager"));
    }
    let url = file_url(path, false).ok_or_else(|| internal("cannot build a file URL for the path"))?;
    // `- (NSURL *)URLForDirectory:(NSSearchPathDirectory)directory inDomain:(NSSearchPathDomainMask)domain
    //    appropriateForURL:(NSURL *)url create:(BOOL)shouldCreate error:(NSError **)error;`
    let url_for = msg_fn!((usize, usize, Id, Bool, *mut Id) -> Id);
    let mut error: Id = std::ptr::null_mut();
    // SAFETY: live manager and URL; NSUInteger enum values; the error out-pointer is valid and receives an
    // autoreleased object (kept alive by `_pool`).
    let found = unsafe {
        url_for(manager, sel(c"URLForDirectory:inDomain:appropriateForURL:create:error:"), NS_TRASH_DIRECTORY, NS_USER_DOMAIN_MASK, url.as_ptr(), NO, &mut error)
    };
    if !found.is_null() {
        // SAFETY: the autoreleased NSURL the call returned, alive until `_pool` drops.
        return unsafe { file_url_path(found) }.ok_or_else(|| internal("the Trash location is not a file path"));
    }
    if error.is_null() {
        return Err(internal("the system reported no Trash for this volume"));
    }
    // SAFETY: the live autoreleased NSError the call returned; `_pool` is in place.
    Err(unsafe { ns_error(error) })
}

/// Readiness of the volume holding `path` for a manual Trash plan: the Trash folder the system would use, or why there
/// is none (the plan is then refused up front with `trash_unavailable`; nothing is ever deleted instead).
pub fn trash_ready(path: &RawPath) -> Result<PathBuf, String> {
    let Some(bytes) = super::unix_bytes(path) else { return Err("not a macOS path".into()) };
    match trash_dir_for(bytes) {
        Ok(dir) => Ok(super::os_path(&dir)),
        Err(e) if e.code == NS_FEATURE_UNSUPPORTED && e.cocoa() => Err(format!("this volume has no Trash ({} {})", e.domain, e.code)),
        Err(e) => Err(format!("the system has no Trash for this volume: {}", e.describe())),
    }
}

// ---------------------------------------------------------------------------------------------
// Who holds planned files open (MTR-04)

/// Processes holding some of the given objects open, and how many processes could not be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenHolders {
    /// `name (pid N)`, one per process, in pid order.
    pub holders: Vec<String>,
    /// Processes whose descriptors could not be read (other users' processes without privileges): a holder among
    /// them is not listed, so an empty `holders` with a non-zero count means "unknown", not "free".
    pub uninspected: usize,
}

/// Which processes hold any of `paths` open (a descriptor or the working directory), from one open-descriptor snapshot
/// (`procs::FdIndex`). Each path is examined without following links; a path that is gone is skipped. Read-only: no
/// process is signalled or ended.
pub fn open_holders(paths: &[RawPath]) -> Result<OpenHolders, String> {
    let index = FdIndex::build().map_err(|e| format!("cannot list processes: {}", describe(e)))?;
    let mut found: std::collections::BTreeMap<i32, String> = std::collections::BTreeMap::new();
    for p in paths {
        let Some(c) = super::unix_bytes(p).and_then(|b| CString::new(b).ok()) else { continue };
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: NUL-terminated path and valid out-pointer; a link reports itself.
        if unsafe { libc::lstat(c.as_ptr(), &mut st) } != 0 {
            continue;
        }
        for h in index.holders(st.st_dev, st.st_ino).holders {
            let name = if h.name.is_empty() {
                h.path.as_deref().and_then(|p| p.rsplit(|&b| b == b'/').next()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| "unknown process".into())
            } else {
                h.name.clone()
            };
            found.entry(h.pid).or_insert_with(|| format!("{name} (pid {})", h.pid));
        }
    }
    Ok(OpenHolders { holders: found.into_values().collect(), uninspected: index.uninspected.len() })
}

/// The `plan apply` holder seam (plan/v2 backends of the CLI and the desktop): holder names, with the other-users limit
/// as the last entry when something was found. `None` = no holder could be determined: nothing visible is "unknown"
/// (some processes may be uninspectable), never "free".
pub fn apply_holders(paths: &[RawPath]) -> Option<Vec<String>> {
    let h = open_holders(paths).ok()?;
    if h.holders.is_empty() {
        return None;
    }
    let mut out = h.holders;
    if h.uninspected > 0 {
        out.push(format!("possibly others among {} process(es) of other users that could not be inspected", h.uninspected));
    }
    Some(out)
}

/// Objects the macOS Trash action cannot take beyond the core refusals: it moves single verified files, so a folder
/// (whose contents cannot be verified as one object in this build) is refused before anything is planned.
pub fn folder_refusal(ins: &cm_core::manual_trash::Inspection) -> Option<cm_core::manual_trash::TrashRefusal> {
    (ins.kind == ObjectKind::Directory).then(|| cm_core::manual_trash::TrashRefusal {
        code: "unsupported",
        reason: "folder_move_unavailable",
        message: "moving a whole folder to the Trash is not available on macOS in this build (its contents cannot be verified as one object); select the files inside instead".into(),
    })
}

// ---------------------------------------------------------------------------------------------
// Trash listing (MTR-03)

/// Finder's own record of the Trash (it holds the Put Back locations): never an item, never listed or deleted.
const FINDER_RECORD: &[u8] = b".DS_Store";

/// One Trash folder as an index-like view plus its top-level items. Original locations are never claimed:
/// `BinItem::original_path` and `deleted_at` stay `None` (Finder › Put Back knows them; its record is private).
#[derive(Debug)]
pub struct TrashListing {
    pub trash_dir: PathBuf,
    pub view: MemView,
    pub items: Vec<BinItem>,
    pub observed_at: String,
}

/// Why a Trash folder could not be listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    /// Refused by permissions (`EACCES`) or by privacy protection / another policy layer (`EPERM`, typically TCC:
    /// Full Disk Access or Removable Volumes). Never reported as an empty Trash.
    PermissionRequired { errno: i32, message: String },
    /// The first open did not return within [`privacy::PROMPT_WAIT`]: macOS is most likely showing a privacy prompt
    /// ([`privacy::pending_reason`]). Unknown, never an empty Trash.
    PromptPending(String),
    /// Any other failure to read it.
    Unreadable(String),
}

impl ListError {
    fn of(dir: &Path, what: &str, e: i32) -> ListError {
        let message = format!("cannot {what} {}: {}", dir.display(), describe(e));
        match e {
            libc::EPERM | libc::EACCES => ListError::PermissionRequired { errno: e, message },
            _ => ListError::Unreadable(message),
        }
    }

    /// The privacy gate gave no result: a pending prompt, or no worker thread.
    fn gate(why: String) -> ListError {
        if privacy::is_pending(&why) { ListError::PromptPending(why) } else { ListError::Unreadable(why) }
    }

    /// `EPERM`: not the folder's own permissions but privacy protection (TCC) or another policy layer; which one is
    /// not determinable from the error alone.
    pub fn privacy_blocked(&self) -> bool {
        matches!(self, ListError::PermissionRequired { errno, .. } if *errno == libc::EPERM)
    }

    pub fn message(&self) -> &str {
        match self {
            ListError::PermissionRequired { message, .. } | ListError::PromptPending(message) | ListError::Unreadable(message) => message,
        }
    }
}

struct Builder {
    view: MemView,
    next: u64,
    dev: i32,
    volume: String,
    quality: IdentityQuality,
    /// BSD file flags per entry (locked, restricted, append-only objects cannot be purged).
    flags: std::collections::HashMap<u64, u32>,
}

impl Builder {
    fn add(&mut self, parent: Option<u64>, name: RawPath, st: &libc::stat) -> u64 {
        let kind = match st.st_mode & libc::S_IFMT {
            libc::S_IFREG => ObjectKind::File,
            libc::S_IFDIR => ObjectKind::Directory,
            libc::S_IFLNK => ObjectKind::Symlink,
            _ => ObjectKind::Other,
        };
        let id = self.next;
        self.next += 1;
        let file_id = ids::file_id(st.st_ino);
        self.view.names.insert(id, name);
        self.view.parents.insert(id, parent);
        if let Some(p) = parent {
            self.view.kids.entry(p).or_default().push(id);
        }
        self.view.identities.insert((self.volume.clone(), file_id.clone()), id);
        self.flags.insert(id, st.st_flags);
        self.view.facts.insert(id, EntryFacts {
            kind,
            // Something mounted inside, or a link: recorded, never traversed.
            boundary: st.st_dev != self.dev || kind == ObjectKind::Symlink,
            object_id: id,
            logical: (kind == ObjectKind::File).then(|| u64::try_from(st.st_size).unwrap_or(0)),
            allocated: (kind == ObjectKind::File).then(|| u64::try_from(st.st_blocks).unwrap_or(0).saturating_mul(512)),
            volume_id: self.volume.clone(),
            file_id,
            quality: self.quality,
            cloud: if st.st_flags & SF_DATALESS != 0 { CloudState::Placeholder } else { CloudState::NotCloud },
            modified_unix: Some(st.st_mtime),
            link_count: (kind == ObjectKind::File).then_some(u32::from(st.st_nlink)),
            skipped_here: 0,
        });
        id
    }

    /// Descendants of a trashed folder through no-follow descriptors (links and mount points are leaves; dataless
    /// folders are not opened; unreadable folders are marked).
    fn walk(&mut self, dir_entry: u64, dir: &Fd) {
        let names = match read_names(dir) {
            Ok(n) => n,
            Err(_) => {
                self.view.facts.get_mut(&dir_entry).expect("added").skipped_here += 1;
                return;
            }
        };
        for name in names {
            let Ok(raw) = RawPath::unix(name.clone()) else { continue };
            let st = match dir.stat_child(&name) {
                Ok(st) => st,
                Err(_) => {
                    self.view.facts.get_mut(&dir_entry).expect("added").skipped_here += 1;
                    continue;
                }
            };
            let id = self.add(Some(dir_entry), raw, &st);
            let f = &self.view.facts[&id];
            if f.kind != ObjectKind::Directory || f.boundary || f.cloud == CloudState::Placeholder {
                continue;
            }
            match CString::new(name).ok().map(|c| dir.open_child_dir(&c)) {
                Some(Ok(child)) => self.walk(id, &child),
                _ => self.view.facts.get_mut(&id).expect("added").skipped_here += 1,
            }
        }
    }

    /// Why an item cannot be purged by this tool, if it cannot.
    fn refusal(&self, item: u64) -> Option<String> {
        let mut stack = vec![item];
        while let Some(e) = stack.pop() {
            let f = &self.view.facts[&e];
            if f.boundary || !matches!(f.kind, ObjectKind::File | ObjectKind::Directory) {
                return Some("contains a link, a mount point or a special object; empty the Trash in Finder instead".into());
            }
            if f.cloud == CloudState::Placeholder {
                return Some("contains a cloud placeholder (dataless), which is never downloaded or touched; empty the Trash in Finder instead".into());
            }
            if f.skipped_here > 0 {
                return Some("part of it could not be read; empty the Trash in Finder instead".into());
            }
            if f.link_count.is_some_and(|n| n > 1) {
                return Some("contains a file with several hard links; deleting one name would not free its data".into());
            }
            if let Some(why) = self.flags.get(&e).and_then(|&fl| protected_flags(fl)) {
                return Some(format!("contains an object {why}; unlock it in Finder first"));
            }
            if f.kind == ObjectKind::Directory {
                stack.extend_from_slice(self.view.children(e));
            }
        }
        None
    }
}

/// Stable id of a trashed object from its identity (FNV-1a 64, as on Windows; selection only, re-verified on apply).
fn item_id(volume_id: &str, file_id: &str) -> String {
    let h = format!("{volume_id}|{file_id}").bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("tb_{h:016x}")
}

/// List one Trash folder (`~/.Trash`, `/Volumes/<v>/.Trashes/<uid>`, or a test folder laid out the same way) without
/// reading any content and without materializing cloud placeholders. Finder's `.DS_Store` record is skipped.
pub fn list_trash_dir(dir: &Path) -> Result<TrashListing, ListError> {
    list_trash_dir_gated(dir, privacy::PROMPT_WAIT, Fd::open_dir)
}

/// [`list_trash_dir`] with the first open (`open`, run through [`privacy::bounded`]) and its deadline injected.
pub(crate) fn list_trash_dir_gated(dir: &Path, wait: Duration, open: impl FnOnce(&CStr) -> Result<Fd, i32> + Send + 'static) -> Result<TrashListing, ListError> {
    let _guard = no_materialize().map_err(|_| ListError::Unreadable("cannot turn off dataless materialization".into()))?;
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| ListError::Unreadable("path contains NUL".into()))?;
    let first = privacy::bounded(dir, wait, move || {
        // This thread's own policy: opening a folder does not materialize it, its listing (on the caller) is guarded.
        let _policy = no_materialize();
        open(&c)
    });
    let root_fd = first.map_err(ListError::gate)?.map_err(|e| ListError::of(dir, "open", e))?;
    let st = root_fd.stat().map_err(|e| ListError::of(dir, "examine", e))?;
    let names = read_names(&root_fd).map_err(|e| ListError::of(dir, "list", e))?;
    let volume = ids::volume_id(&root_fd, st.st_dev).map_err(|e| ListError::of(dir, "identify the volume of", e))?;
    let quality = if ids::is_stable_volume_id(&volume) { IdentityQuality::Strong } else { IdentityQuality::Weak };
    let root_path = RawPath::unix(dir.as_os_str().as_bytes().to_vec()).map_err(|e| ListError::Unreadable(e.to_string()))?;
    let mut b = Builder { view: MemView { platform: Some(Platform::Macos), ..Default::default() }, next: 0, dev: st.st_dev, volume, quality, flags: Default::default() };
    let root = b.add(None, root_path, &st);
    b.view.root = root;
    let observed_at = cm_core::time::now_utc();
    let mut names: Vec<Vec<u8>> = names.into_iter().filter(|n| n.as_slice() != FINDER_RECORD).collect();
    names.sort();
    let mut tops = Vec::new();
    for name in names {
        let Ok(raw) = RawPath::unix(name.clone()) else { continue };
        let Ok(cst) = root_fd.stat_child(&name) else {
            b.view.facts.get_mut(&root).expect("added").skipped_here += 1;
            continue;
        };
        let id = b.add(Some(root), raw, &cst);
        let f = &b.view.facts[&id];
        if f.kind == ObjectKind::Directory && !f.boundary && f.cloud != CloudState::Placeholder {
            match CString::new(name).ok().map(|c| root_fd.open_child_dir(&c)) {
                Some(Ok(child)) => b.walk(id, &child),
                _ => b.view.facts.get_mut(&id).expect("added").skipped_here += 1,
            }
        }
        tops.push(id);
    }
    let mut items = Vec::new();
    for id in tops {
        let f = &b.view.facts[&id];
        let mut total = Some(0u64);
        let mut stack = vec![id];
        while let Some(e) = stack.pop() {
            let ef = &b.view.facts[&e];
            match ef.kind {
                ObjectKind::Directory => stack.extend_from_slice(b.view.children(e)),
                ObjectKind::File => total = total.zip(ef.logical).map(|(t, s)| t.saturating_add(s)),
                _ => {}
            }
        }
        items.push(BinItem {
            trash_item_id: item_id(&f.volume_id, &f.file_id),
            original_path: None,
            deleted_at: None,
            kind: f.kind,
            logical_bytes: total,
            data_entry: id,
            info_entry: None,
            refusal: b.refusal(id),
        });
    }
    Ok(TrashListing { trash_dir: dir.to_path_buf(), view: b.view, items, observed_at })
}

/// The per-user Trash folders on mounted volumes other than the boot/data volume: `/Volumes/<v>/.Trashes/<uid>` where
/// one exists, plus those that could not be examined (for example blocked by Removable Volumes privacy protection).
/// Links in `/Volumes` (the boot volume's own entry) are not followed.
pub fn volume_trash_dirs() -> (Vec<PathBuf>, Vec<(PathBuf, ListError)>) {
    let (mut found, mut failed) = (Vec::new(), Vec::new());
    let Ok(rd) = std::fs::read_dir("/Volumes") else { return (found, failed) };
    let uid = super::euid();
    let mut vols: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    vols.sort();
    for v in vols {
        let dir = v.join(".Trashes").join(uid.to_string());
        // Looking inside a volume can wait for the Removable / Network Volumes prompt.
        let probe = dir.clone();
        let meta = match privacy::bounded(&dir, privacy::PROMPT_WAIT, move || std::fs::symlink_metadata(&probe)) {
            Ok(meta) => meta,
            Err(why) => {
                failed.push((dir, ListError::gate(why)));
                continue;
            }
        };
        match meta {
            Ok(md) if md.is_dir() => found.push(dir),
            Ok(_) => {}
            Err(e) => match e.raw_os_error() {
                Some(libc::ENOENT | libc::ENOTDIR) | None => {}
                Some(code) => failed.push((dir.clone(), ListError::of(&dir, "examine", code))),
            },
        }
    }
    (found, failed)
}

/// The current path of an open descriptor (`fcntl(F_GETPATH)`), as raw bytes.
pub(crate) fn fd_path(fd: c_int) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: a descriptor and a buffer of MAXPATHLEN bytes, as F_GETPATH requires; an invalid fd only fails.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr().cast::<c_char>()) } == -1 {
        return None;
    }
    // SAFETY: on success the kernel wrote a NUL-terminated path into `buf`.
    let path = unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) };
    Some(path.to_bytes().to_vec())
}

/// An event-only, no-follow descriptor held on the verified object: it reads nothing and does not block unmounts.
struct Held(c_int);

impl Held {
    fn open(parent: &Fd, name: &CStr) -> Result<Held, i32> {
        let flags: c_int = libc::O_EVTONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: valid directory fd and NUL-terminated single-component name; no mode without O_CREAT.
        let fd = unsafe { libc::openat(parent.raw(), name.as_ptr(), flags) };
        if fd < 0 { Err(errno()) } else { Ok(Held(fd)) }
    }

    fn stat(&self) -> Result<libc::stat, i32> {
        // SAFETY: zeroed POD out-parameter of the declared type.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and out-pointer.
        if unsafe { libc::fstat(self.0, &mut st) } != 0 { Err(errno()) } else { Ok(st) }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: this value owns the descriptor and closes it exactly once.
        unsafe { libc::close(self.0) };
    }
}

pub(super) fn trash(actions: &MacActions, t: &ActionTarget<'_>) -> ActionOutcome {
    let _guard = match no_materialize() {
        Ok(g) => g,
        Err(stop) => return stop.outcome(),
    };
    let v = match verify(t, false) {
        Ok(v) => v,
        Err(stop) => return stop.outcome(),
    };
    if let Err(stop) = actions.in_use(&v.live) {
        return stop.outcome();
    }
    let Some(path) = super::unix_bytes(t.path) else { return Stop::Unsupported("not a macOS path".into()).outcome() };
    move_verified(&v, path)
}

/// Steps 2 and 3 of a Trash move for an object the caller verified (`v`: its held parent, name and live metadata;
/// `path` the same object's absolute path): a held event-only descriptor, a last look at the name, the system move,
/// then the held descriptor must name the resulting item. Used by [`trash`] and by the app-bundle uninstall, which
/// verifies a bundle directory by its own planned identity.
pub(crate) fn move_verified(v: &Verified, path: &[u8]) -> ActionOutcome {
    let held = match Held::open(&v.parent, &v.name) {
        Ok(fd) => fd,
        Err(libc::ELOOP) => return Stop::Reparse.outcome(),
        Err(e) => return Stop::from_target(e).outcome(),
    };
    match held.stat() {
        Ok(st) if (st.st_dev, st.st_ino) == (v.live.st_dev, v.live.st_ino) => {}
        Ok(_) => return Stop::Identity.outcome(),
        Err(e) => return Stop::Error(format!("cannot query the held file: {}", describe(e))).outcome(),
    }
    // Last look at the name before the path-based system call.
    match at_name(&v.parent, &v.name, &v.live) {
        AtPath::Planned => {}
        AtPath::Other => return Stop::Identity.outcome(),
        AtPath::Missing => return Stop::Missing.outcome(),
        AtPath::Error(e) => return Stop::Error(format!("cannot examine the target: {e}")).outcome(),
    }
    #[cfg(test)]
    super::tests::before_trash();
    let moved = move_to_trash(path);
    let held_path = fd_path(held.0);
    let still_planned = held.stat().is_ok_and(|st| (st.st_dev, st.st_ino) == (v.live.st_dev, v.live.st_ino) && st.st_nlink >= 1);
    let now = at_name(&v.parent, &v.name, &v.live);
    let planned_at = || held_path.as_deref().map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_else(|| "an unknown location".into());
    match (moved, now) {
        (Ok(_), AtPath::Planned) => ActionOutcome::Unknown { message: "the system reported the move to the Trash but the file is still at its path".into() },
        (Err(e), AtPath::Planned) => ActionOutcome::Failed { code: e.outcome_code().into(), message: format!("not moved to the Trash: {}", e.describe()) },
        (Ok(Some(resulting)), AtPath::Missing | AtPath::Other) if still_planned && held_path.as_deref() == Some(resulting.as_slice()) => {
            ActionOutcome::Succeeded { recovery: Recovery::Trash }
        }
        (Ok(Some(resulting)), AtPath::Missing | AtPath::Other) => ActionOutcome::Unknown {
            message: format!(
                "the item moved to the Trash ({}) could not be confirmed as the planned file, whose name was replaced just before the move; the planned file is now at {}. Open the Trash, check the item and use Put Back if it should not be removed",
                String::from_utf8_lossy(&resulting),
                planned_at()
            ),
        },
        (Ok(None), AtPath::Missing | AtPath::Other) => {
            ActionOutcome::Unknown { message: format!("the file left its path but the system did not report where it went (now at {}); check the Trash", planned_at()) }
        }
        (Err(e), AtPath::Missing | AtPath::Other) => ActionOutcome::Unknown {
            message: format!("the file left its path although the move to the Trash reported an error ({}); it is now at {}; check the Trash", e.describe(), planned_at()),
        },
        (_, AtPath::Error(e)) => ActionOutcome::Unknown { message: format!("cannot confirm the Trash result: {e}; check the Trash") },
    }
}

#[cfg(test)]
mod list_tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use super::*;
    use crate::macos::testfs::Fixture;

    fn raw(p: &Path) -> RawPath {
        RawPath::unix(p.as_os_str().as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn a_trash_folder_lists_items_without_original_locations() {
        let fx = Fixture::new("trash-list");
        let bin = fx.mkdir("home/.Trash");
        fx.write("home/.Trash/.DS_Store", b"finder record");
        fx.write("home/.Trash/old.mov", &[7u8; 900]);
        fx.write("home/.Trash/renders/a.png", b"111");
        fx.write("home/.Trash/renders/sub/b.png", b"2222");
        let outside = fx.write("outside.txt", b"x");
        std::os::unix::fs::symlink(&outside, bin.join("link")).unwrap();
        let hard = fx.write("home/.Trash/twice.bin", b"hl");
        std::fs::hard_link(&hard, fx.path().join("other-name.bin")).unwrap();
        let l = list_trash_dir(&bin).unwrap();
        let names: Vec<String> = l.items.iter().map(|i| l.view.name(i.data_entry).lossy()).collect();
        assert_eq!(names, ["link", "old.mov", "renders", "twice.bin"], "Finder's record is never an item");
        assert!(l.items.iter().all(|i| i.original_path.is_none() && i.deleted_at.is_none() && i.info_entry.is_none()));
        let by = |n: &str| l.items.iter().find(|i| l.view.name(i.data_entry).lossy() == n).unwrap();
        assert_eq!((by("renders").kind, by("renders").logical_bytes, by("renders").refusal.as_deref()), (ObjectKind::Directory, Some(7), None));
        assert!(by("link").refusal.as_deref().is_some_and(|r| r.contains("link")));
        assert!(by("twice.bin").refusal.as_deref().is_some_and(|r| r.contains("hard links")));
        assert!(by("old.mov").trash_item_id.starts_with("tb_"));
        let input = cm_core::manual_trash::ManualInput {
            plan_id: "mp", scope_id: None, scan_id: None, scan_revision: None, created_unix: 1_800_000_000, observed_at: &l.observed_at,
            policy_digest: cm_core::plan::v2::NO_RULES_DIGEST,
        };
        let plan = cm_core::manual_trash::build_purge_plan(&l.view, &[by("renders"), by("old.mov")], &input).unwrap();
        assert_eq!(plan.body.items.len(), 2 + 2 + 1, "folder: two files, sub and itself; the file");
        assert_eq!(l.view.path(by("old.mov").data_entry), raw(&bin.join("old.mov")));
        assert_eq!(l.view.path(l.view.root()), raw(&bin));
    }

    #[test]
    fn an_unreadable_trash_is_permission_required_never_empty() {
        let fx = Fixture::new("trash-perm");
        let bin = fx.mkdir("home/.Trash");
        fx.write("home/.Trash/x.bin", b"x");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o000)).unwrap();
        // SAFETY: plain getter.
        let root = unsafe { libc::geteuid() } == 0;
        let out = list_trash_dir(&bin);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        if root {
            eprintln!("skipped: root reads a mode-000 folder");
            return;
        }
        match out {
            Err(ListError::PermissionRequired { errno, .. }) => assert_eq!(errno, libc::EACCES),
            other => panic!("{other:?}"),
        }
        assert!(matches!(list_trash_dir(&fx.path().join("missing")), Err(ListError::Unreadable(_))));
    }

    #[test]
    fn the_trash_for_a_temporary_file_is_resolved_without_creating_anything() {
        let fx = Fixture::new("trash-ready");
        let f = fx.write("x.bin", b"x");
        match trash_ready(&raw(&f)) {
            Ok(dir) => assert!(dir.to_string_lossy().contains(".Trash"), "{}", dir.display()),
            // A runner without a home Trash yet: refused, never anything else.
            Err(why) => assert!(!why.is_empty()),
        }
        assert!(trash_ready(&RawPath::windows(r"C:\x".encode_utf16().collect()).unwrap()).is_err());
    }

    #[test]
    fn holders_name_the_process_and_count_what_could_not_be_inspected() {
        let fx = Fixture::new("trash-holders");
        let f = fx.write("held.log", b"log");
        let mut child = Command::new("/usr/bin/tail").arg("-f").arg(&f).spawn().unwrap();
        let mut seen = OpenHolders::default();
        for _ in 0..50 {
            seen = open_holders(&[raw(&f)]).unwrap();
            if !seen.holders.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(seen.holders.iter().any(|h| h.starts_with("tail (pid ")), "{seen:?}");
        assert!(open_holders(&[raw(&fx.path().join("gone"))]).unwrap().holders.is_empty());
    }
}
