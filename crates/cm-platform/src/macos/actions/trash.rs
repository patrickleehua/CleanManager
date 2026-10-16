//! Moves to the user's Trash on macOS with identity checks before and after (parity D6).
//!
//! `-[NSFileManager trashItemAtURL:resultingItemURL:error:]` is the system's Trash interface (it picks the right Trash
//! for the volume and records Put Back); it works on a path. So:
//! 1. the target is verified like every other action (descriptor walk, namespace gate, live metadata, in use);
//! 2. an event-only, no-follow descriptor (`O_EVTONLY | O_NOFOLLOW`: no read access, no content) is held on the
//!    verified object and the name is looked at once more right before the call;
//! 3. afterwards, `fcntl(F_GETPATH)` on the held descriptor must name exactly the resulting item URL and `fstat` must
//!    still show the planned object. Nothing inside `~/.Trash` (TCC-protected) is listed or examined by path.
//!
//! RESIDUAL RACE: the name can still be swapped between the last look and the system's own lookup. The item that went
//! to the Trash is then not the planned one; it stays recoverable there, and the outcome is `unknown` with put-back
//! guidance. A refused or impossible move (no Trash on the volume, `NSFeatureUnsupportedError`) is reported as
//! `trash_unavailable` and is never turned into a delete.

use std::ffi::{CStr, c_char, c_int};

use cm_core::capability::CapabilityState;
use cm_core::execute::{ActionOutcome, ActionTarget, Recovery};

use super::{AtPath, MacActions, Stop, at_name, no_materialize, verify};
use crate::macos::ffi::objc::{AutoreleasePool, Bool, Id, Sel, class, file_url, file_url_path, ns_string_value, sel};
use crate::macos::sys::{Fd, describe, errno};

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`. The
/// executor's `execute.trash` row lives in `actions::CAPS`; the gated manual-trash tool (`trash.manual`) is declared
/// by its command module once that exists on macOS.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[];

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
    // `- (NSInteger)code;` / `- (NSErrorDomain)domain;` / `- (NSString *)localizedDescription;`
    let get_code = msg_fn!(() -> isize);
    let get_object = msg_fn!(() -> Id);
    // SAFETY: `error` is the live autoreleased NSError the call returned; the strings are copied before `_pool` drops.
    let (code, domain, message) = unsafe {
        (
            get_code(error, sel(c"code")),
            ns_string_value(get_object(error, sel(c"domain"))).unwrap_or_default(),
            ns_string_value(get_object(error, sel(c"localizedDescription"))).unwrap_or_default(),
        )
    };
    Err(TrashError { domain, code, message })
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
    let Some(path) = super::unix_bytes(t.path) else { return Stop::Unsupported("not a macOS path".into()).outcome() };
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
