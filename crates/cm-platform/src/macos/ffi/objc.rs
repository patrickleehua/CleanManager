//! Objective-C runtime: classes, selectors, exactly typed `objc_msgSend`, owned object references,
//! an autorelease pool guard, and `NSString` / `NSURL` helpers. Links libobjc, Foundation and
//! AppKit (later work packages use `NSWorkspace` / `NSRunningApplication`).
//!
//! On arm64 `objc_msgSend` must be called through a function pointer of the method's exact C
//! type (it is not variadic), so every send goes through [`msg_fn!`] with the signature spelled
//! out next to the selector's header declaration.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{self, NonNull};

/// `id`
pub(crate) type Id = *mut c_void;
/// `SEL`
pub(crate) type Sel = *const c_void;
/// `BOOL`: C `bool` on arm64, `signed char` on x86_64 (`objc/objc.h`).
#[cfg(target_arch = "aarch64")]
pub(crate) type Bool = bool;
#[cfg(not(target_arch = "aarch64"))]
pub(crate) type Bool = i8;
/// `NSUInteger`
type NSUInteger = usize;

#[cfg(target_arch = "aarch64")]
fn truthy(b: Bool) -> bool {
    b
}
#[cfg(not(target_arch = "aarch64"))]
fn truthy(b: Bool) -> bool {
    b != 0
}
#[cfg(target_arch = "aarch64")]
fn objc_bool(b: bool) -> Bool {
    b
}
#[cfg(not(target_arch = "aarch64"))]
fn objc_bool(b: bool) -> Bool {
    i8::from(b)
}

/// `NSUTF8StringEncoding`
const NS_UTF8: NSUInteger = 4;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `Class objc_getClass(const char *name);` (`objc/runtime.h`)
    fn objc_getClass(name: *const c_char) -> Id;
    /// `SEL sel_registerName(const char *str);` (`objc/runtime.h`)
    fn sel_registerName(name: *const c_char) -> Sel;
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's type before calling.
    fn objc_msgSend();
    /// `id objc_retain(id value);` / `void objc_release(id value);`: the exported ARC entry points.
    fn objc_retain(obj: Id) -> Id;
    fn objc_release(obj: Id);
    /// `void *objc_autoreleasePoolPush(void);` / `void objc_autoreleasePoolPop(void *context);`
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(context: *mut c_void);
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

/// `objc_msgSend` as `unsafe extern "C" fn(id, SEL, args...) -> ret`.
macro_rules! msg_fn {
    (($($arg:ty),*) -> $ret:ty) => {
        // SAFETY: function pointers share one size and representation; the caller sends only
        // selectors whose implementation has exactly this C signature.
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $arg)*) -> $ret>(
                objc_msgSend as unsafe extern "C" fn(),
            )
        }
    };
}

/// A registered class, `None` when no loaded image defines it.
pub(crate) fn class(name: &CStr) -> Option<Id> {
    // SAFETY: NUL-terminated name; lookup only.
    let c = unsafe { objc_getClass(name.as_ptr()) };
    (!c.is_null()).then_some(c)
}

/// The selector for `name` (registered on first use; never NULL).
pub(crate) fn sel(name: &CStr) -> Sel {
    // SAFETY: NUL-terminated name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// An owned (+1) Objective-C object reference, released once on drop.
#[derive(Debug)]
pub(crate) struct Retained(NonNull<c_void>);

impl Retained {
    /// Take over a +1 reference (`alloc`/`init`, `new`, `copy`). `None` for nil.
    ///
    /// # Safety
    /// `obj` must be nil or an object the caller owns one reference to.
    pub(crate) unsafe fn from_owned(obj: Id) -> Option<Retained> {
        NonNull::new(obj).map(Retained)
    }

    /// Retain a +0 (autoreleased or borrowed) reference. `None` for nil.
    ///
    /// # Safety
    /// `obj` must be nil or a live object.
    pub(crate) unsafe fn retain(obj: Id) -> Option<Retained> {
        let obj = NonNull::new(obj)?;
        // SAFETY: live object per the caller; balanced by Drop.
        unsafe { objc_retain(obj.as_ptr()) };
        Some(Retained(obj))
    }

    pub(crate) fn as_ptr(&self) -> Id {
        self.0.as_ptr()
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: this value owns exactly one reference.
        unsafe { objc_release(self.as_ptr()) };
    }
}

/// `@autoreleasepool { ... }` for the guard's lifetime. Must be dropped on the creating thread
/// in LIFO order with other pools, which `!Send` plus scope-based drop ensure.
pub(crate) struct AutoreleasePool {
    context: *mut c_void,
}

impl AutoreleasePool {
    #[allow(clippy::new_without_default)]
    pub(crate) fn new() -> AutoreleasePool {
        // SAFETY: no preconditions; popped by Drop on this thread.
        AutoreleasePool { context: unsafe { objc_autoreleasePoolPush() } }
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        // SAFETY: the context came from the matching push on this thread (the raw pointer makes the guard !Send).
        unsafe { objc_autoreleasePoolPop(self.context) };
    }
}

/// `[[cls alloc] init...]` helper: `+ (instancetype)alloc;`
fn alloc(cls: &CStr) -> Option<Id> {
    let cls = class(cls)?;
    let f = msg_fn!(() -> Id);
    // SAFETY: `alloc` on a class object returns a +1 uninitialized instance (or nil).
    let obj = unsafe { f(cls, sel(c"alloc")) };
    (!obj.is_null()).then_some(obj)
}

/// `- (BOOL)isKindOfClass:(Class)aClass;`
///
/// # Safety
/// `obj` must be a live object.
pub(crate) unsafe fn is_kind_of(obj: Id, cls: &CStr) -> bool {
    let Some(cls) = class(cls) else { return false };
    let f = msg_fn!((Id) -> Bool);
    // SAFETY: live receiver per the caller; NSObject protocol method with this signature.
    truthy(unsafe { f(obj, sel(c"isKindOfClass:"), cls) })
}

/// A new `NSString` from UTF-8 text.
pub(crate) fn ns_string(s: &str) -> Option<Retained> {
    let obj = alloc(c"NSString")?;
    // `- (instancetype)initWithBytes:(const void *)bytes length:(NSUInteger)len encoding:(NSStringEncoding)encoding;`
    let f = msg_fn!((*const c_void, NSUInteger, NSUInteger) -> Id);
    // SAFETY: fresh +1 instance; `init` consumes it and returns +1 (or nil after releasing it).
    unsafe { Retained::from_owned(f(obj, sel(c"initWithBytes:length:encoding:"), s.as_ptr().cast(), s.len(), NS_UTF8)) }
}

/// Text of an `NSString`; `None` for nil or another class.
///
/// # Safety
/// `obj` must be nil or a live object.
pub(crate) unsafe fn ns_string_value(obj: Id) -> Option<String> {
    // SAFETY: live object per the caller.
    if obj.is_null() || !unsafe { is_kind_of(obj, c"NSString") } {
        return None;
    }
    let _pool = AutoreleasePool::new();
    // `@property(readonly) const char *UTF8String;` (lifetime: the enclosing pool)
    let f = msg_fn!(() -> *const c_char);
    // SAFETY: live NSString receiver.
    let p = unsafe { f(obj, sel(c"UTF8String")) };
    // SAFETY: non-null NUL-terminated buffer valid until `_pool` drops; copied now.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// A file `NSURL` for raw path bytes (any bytes except NUL; no Unicode interpretation).
pub(crate) fn file_url(path: &[u8], is_dir: bool) -> Option<Retained> {
    let c = CString::new(path).ok()?;
    let obj = alloc(c"NSURL")?;
    // `- (instancetype)initFileURLWithFileSystemRepresentation:(const char *)path isDirectory:(BOOL)isDir relativeToURL:(NSURL *)baseURL;`
    let f = msg_fn!((*const c_char, Bool, Id) -> Id);
    let is_dir = objc_bool(is_dir);
    // SAFETY: fresh +1 instance consumed by `init`; NUL-terminated path; nil base URL.
    unsafe { Retained::from_owned(f(obj, sel(c"initFileURLWithFileSystemRepresentation:isDirectory:relativeToURL:"), c.as_ptr(), is_dir, ptr::null_mut())) }
}

/// The raw file-system path bytes of a file `NSURL`.
///
/// # Safety
/// `url` must be nil or a live object.
pub(crate) unsafe fn file_url_path(url: Id) -> Option<Vec<u8>> {
    // SAFETY: live object per the caller.
    if url.is_null() || !unsafe { is_kind_of(url, c"NSURL") } {
        return None;
    }
    // `- (BOOL)getFileSystemRepresentation:(char *)buffer maxLength:(NSUInteger)maxBufferLength;`
    let f = msg_fn!((*mut c_char, NSUInteger) -> Bool);
    let mut cap = 4096usize;
    while cap <= 1 << 16 {
        let mut buf = vec![0u8; cap];
        // SAFETY: live NSURL; buffer of `cap` writable bytes, NUL-terminated on success.
        let ok = unsafe { f(url, sel(c"getFileSystemRepresentation:maxLength:"), buf.as_mut_ptr().cast(), cap) };
        if truthy(ok) {
            let end = buf.iter().position(|&b| b == 0)?;
            buf.truncate(end);
            return Some(buf);
        }
        cap *= 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_and_appkit_classes_are_loaded() {
        assert!(class(c"NSString").is_some());
        assert!(class(c"NSURL").is_some());
        assert!(class(c"NSWorkspace").is_some(), "AppKit is linked");
        assert!(class(c"CMNoSuchClass").is_none());
    }

    #[test]
    fn ns_string_round_trips_text() {
        let _pool = AutoreleasePool::new();
        let s = ns_string("Grüße ✓").unwrap();
        // SAFETY: live objects.
        unsafe {
            assert_eq!(ns_string_value(s.as_ptr()).as_deref(), Some("Grüße ✓"));
            assert!(!is_kind_of(s.as_ptr(), c"NSURL"));
        }
    }

    #[test]
    fn ns_url_round_trips_non_utf8_name_bytes() {
        let _pool = AutoreleasePool::new();
        let raw = b"/private/var/tmp/cm-\xff\x80-\xe9t\xe9/f\xc0.bin".to_vec();
        let url = file_url(&raw, false).unwrap();
        // SAFETY: live NSURL; NSURL is toll-free bridged to CFURL.
        unsafe {
            assert_eq!(file_url_path(url.as_ptr()), Some(raw.clone()));
            assert_eq!(crate::macos::ffi::cf::file_url_path(url.as_ptr().cast_const()), Some(raw));
            assert_eq!(ns_string_value(url.as_ptr()), None);
        }
        assert!(file_url(b"/a\0b", false).is_none());
    }
}
