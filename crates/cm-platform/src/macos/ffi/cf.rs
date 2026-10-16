//! CoreFoundation: owned references, strings, URLs from raw path bytes, data, property lists
//! (XML / binary, both directions) and app preferences. Declarations follow
//! `CoreFoundation.framework/Headers/*.h`; ownership follows the Create/Copy (+1) and Get (+0)
//! rules, made explicit by [`CfRef::from_create`] / [`CfRef::from_get`].

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::{self, NonNull};

pub(crate) type CFTypeRef = *const c_void;
/// `typedef signed long CFIndex;`
pub(crate) type CFIndex = isize;
/// `typedef unsigned long CFTypeID;`
pub(crate) type CFTypeID = usize;
/// `typedef unsigned char Boolean;`
pub(crate) type Boolean = u8;
type CFAllocatorRef = *const c_void;
type CFStringEncoding = u32;
type CFOptionFlags = usize;
type CFNumberType = CFIndex;

/// `kCFStringEncodingUTF8`.
const UTF8: CFStringEncoding = 0x0800_0100;
const NUMBER_SINT64: CFNumberType = 4;
const NUMBER_FLOAT64: CFNumberType = 6;
/// `kCFPropertyListImmutable`.
const PLIST_IMMUTABLE: CFOptionFlags = 0;
/// Seconds from 1970-01-01 to CoreFoundation's reference date 2001-01-01 (`kCFAbsoluteTimeIntervalSince1970`).
const ABSOLUTE_TIME_1970: f64 = 978_307_200.0;
/// Nesting limit when converting a property list (defends against pathological inputs).
const MAX_DEPTH: usize = 64;

#[repr(C)]
struct CFRange {
    location: CFIndex,
    length: CFIndex,
}

/// `CFArrayCallBacks`: version + 4 callback pointers.
#[repr(C)]
pub(crate) struct CFArrayCallBacks {
    _version: CFIndex,
    _callbacks: [*const c_void; 4],
}

/// `CFDictionaryKeyCallBacks`: version + 5 callback pointers.
#[repr(C)]
pub(crate) struct CFDictionaryKeyCallBacks {
    _version: CFIndex,
    _callbacks: [*const c_void; 5],
}

/// `CFDictionaryValueCallBacks`: version + 4 callback pointers.
#[repr(C)]
pub(crate) struct CFDictionaryValueCallBacks {
    _version: CFIndex,
    _callbacks: [*const c_void; 4],
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    // CFBase.h
    /// `CFTypeID CFGetTypeID(CFTypeRef cf);`
    pub(crate) fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    /// `CFTypeRef CFRetain(CFTypeRef cf);`
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    /// `void CFRelease(CFTypeRef cf);`
    fn CFRelease(cf: CFTypeRef);

    // CFString.h
    pub(crate) safe fn CFStringGetTypeID() -> CFTypeID;
    /// `CFStringRef CFStringCreateWithBytes(CFAllocatorRef alloc, const UInt8 *bytes, CFIndex numBytes, CFStringEncoding encoding, Boolean isExternalRepresentation);`
    fn CFStringCreateWithBytes(alloc: CFAllocatorRef, bytes: *const u8, num_bytes: CFIndex, encoding: CFStringEncoding, external: Boolean) -> CFTypeRef;
    /// `CFIndex CFStringGetLength(CFStringRef theString);`
    fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
    /// `CFIndex CFStringGetBytes(CFStringRef theString, CFRange range, CFStringEncoding encoding, UInt8 lossByte, Boolean isExternalRepresentation, UInt8 *buffer, CFIndex maxBufLen, CFIndex *usedBufLen);`
    fn CFStringGetBytes(s: CFTypeRef, range: CFRange, encoding: CFStringEncoding, loss_byte: u8, external: Boolean, buffer: *mut u8, max_len: CFIndex, used: *mut CFIndex) -> CFIndex;

    // CFData.h
    pub(crate) safe fn CFDataGetTypeID() -> CFTypeID;
    /// `CFDataRef CFDataCreate(CFAllocatorRef allocator, const UInt8 *bytes, CFIndex length);`
    fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFTypeRef;
    /// `const UInt8 *CFDataGetBytePtr(CFDataRef theData);`
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    /// `CFIndex CFDataGetLength(CFDataRef theData);`
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;

    // CFNumber.h
    safe fn CFNumberGetTypeID() -> CFTypeID;
    safe fn CFBooleanGetTypeID() -> CFTypeID;
    /// `CFNumberRef CFNumberCreate(CFAllocatorRef allocator, CFNumberType theType, const void *valuePtr);`
    fn CFNumberCreate(alloc: CFAllocatorRef, kind: CFNumberType, value: *const c_void) -> CFTypeRef;
    /// `Boolean CFNumberGetValue(CFNumberRef number, CFNumberType theType, void *valuePtr);`
    fn CFNumberGetValue(number: CFTypeRef, kind: CFNumberType, value: *mut c_void) -> Boolean;
    /// `Boolean CFNumberIsFloatType(CFNumberRef number);`
    fn CFNumberIsFloatType(number: CFTypeRef) -> Boolean;
    /// `Boolean CFBooleanGetValue(CFBooleanRef boolean);`
    fn CFBooleanGetValue(b: CFTypeRef) -> Boolean;
    /// `const CFBooleanRef kCFBooleanTrue;` / `kCFBooleanFalse`
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;

    // CFDate.h
    safe fn CFDateGetTypeID() -> CFTypeID;
    /// `CFDateRef CFDateCreate(CFAllocatorRef allocator, CFAbsoluteTime at);`
    fn CFDateCreate(alloc: CFAllocatorRef, at: f64) -> CFTypeRef;
    /// `CFAbsoluteTime CFDateGetAbsoluteTime(CFDateRef theDate);`
    fn CFDateGetAbsoluteTime(date: CFTypeRef) -> f64;

    // CFArray.h
    pub(crate) safe fn CFArrayGetTypeID() -> CFTypeID;
    /// `CFArrayRef CFArrayCreate(CFAllocatorRef allocator, const void **values, CFIndex numValues, const CFArrayCallBacks *callBacks);`
    fn CFArrayCreate(alloc: CFAllocatorRef, values: *const CFTypeRef, count: CFIndex, callbacks: *const CFArrayCallBacks) -> CFTypeRef;
    /// `CFIndex CFArrayGetCount(CFArrayRef theArray);`
    pub(crate) fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    /// `const void *CFArrayGetValueAtIndex(CFArrayRef theArray, CFIndex idx);`
    pub(crate) fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: CFIndex) -> CFTypeRef;
    static kCFTypeArrayCallBacks: CFArrayCallBacks;

    // CFDictionary.h
    pub(crate) safe fn CFDictionaryGetTypeID() -> CFTypeID;
    /// `CFDictionaryRef CFDictionaryCreate(CFAllocatorRef allocator, const void **keys, const void **values, CFIndex numValues, const CFDictionaryKeyCallBacks *keyCallBacks, const CFDictionaryValueCallBacks *valueCallBacks);`
    fn CFDictionaryCreate(
        alloc: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFTypeRef;
    /// `CFIndex CFDictionaryGetCount(CFDictionaryRef theDict);`
    fn CFDictionaryGetCount(dict: CFTypeRef) -> CFIndex;
    /// `void CFDictionaryGetKeysAndValues(CFDictionaryRef theDict, const void **keys, const void **values);`
    fn CFDictionaryGetKeysAndValues(dict: CFTypeRef, keys: *mut CFTypeRef, values: *mut CFTypeRef);
    /// `const void *CFDictionaryGetValue(CFDictionaryRef theDict, const void *key);`
    pub(crate) fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;

    // CFURL.h
    safe fn CFURLGetTypeID() -> CFTypeID;
    /// `CFURLRef CFURLCreateFromFileSystemRepresentation(CFAllocatorRef allocator, const UInt8 *buffer, CFIndex bufLen, Boolean isDirectory);`
    fn CFURLCreateFromFileSystemRepresentation(alloc: CFAllocatorRef, buffer: *const u8, len: CFIndex, is_dir: Boolean) -> CFTypeRef;
    /// `Boolean CFURLGetFileSystemRepresentation(CFURLRef url, Boolean resolveAgainstBase, UInt8 *buffer, CFIndex maxBufLen);`
    fn CFURLGetFileSystemRepresentation(url: CFTypeRef, resolve: Boolean, buffer: *mut u8, max_len: CFIndex) -> Boolean;

    // CFError.h
    /// `CFStringRef CFErrorCopyDescription(CFErrorRef err);`
    fn CFErrorCopyDescription(err: CFTypeRef) -> CFTypeRef;

    // CFPropertyList.h
    /// `CFPropertyListRef CFPropertyListCreateWithData(CFAllocatorRef allocator, CFDataRef data, CFOptionFlags options, CFPropertyListFormat *format, CFErrorRef *error);`
    fn CFPropertyListCreateWithData(alloc: CFAllocatorRef, data: CFTypeRef, options: CFOptionFlags, format: *mut CFIndex, error: *mut CFTypeRef) -> CFTypeRef;
    /// `CFDataRef CFPropertyListCreateData(CFAllocatorRef allocator, CFPropertyListRef propertyList, CFPropertyListFormat format, CFOptionFlags options, CFErrorRef *error);`
    fn CFPropertyListCreateData(alloc: CFAllocatorRef, plist: CFTypeRef, format: CFIndex, options: CFOptionFlags, error: *mut CFTypeRef) -> CFTypeRef;

    // CFPreferences.h
    /// `CFPropertyListRef CFPreferencesCopyAppValue(CFStringRef key, CFStringRef applicationID);`
    fn CFPreferencesCopyAppValue(key: CFTypeRef, app: CFTypeRef) -> CFTypeRef;
    /// `void CFPreferencesSetAppValue(CFStringRef key, CFPropertyListRef value, CFStringRef applicationID);`
    fn CFPreferencesSetAppValue(key: CFTypeRef, value: CFTypeRef, app: CFTypeRef);
    /// `Boolean CFPreferencesAppSynchronize(CFStringRef applicationID);`
    fn CFPreferencesAppSynchronize(app: CFTypeRef) -> Boolean;
}

/// An owned CoreFoundation reference, released once on drop.
#[derive(Debug)]
pub(crate) struct CfRef(NonNull<c_void>);

impl CfRef {
    /// Take over a +1 reference returned by a Create/Copy function. `None` for NULL.
    ///
    /// # Safety
    /// `p` must be NULL or a CoreFoundation object the caller owns one reference to.
    pub(crate) unsafe fn from_create(p: CFTypeRef) -> Option<CfRef> {
        NonNull::new(p.cast_mut()).map(CfRef)
    }

    /// Retain a +0 reference returned by a Get function. `None` for NULL.
    ///
    /// # Safety
    /// `p` must be NULL or a live CoreFoundation object.
    pub(crate) unsafe fn from_get(p: CFTypeRef) -> Option<CfRef> {
        let p = NonNull::new(p.cast_mut())?;
        // SAFETY: live object per the caller; the extra reference is released by Drop.
        unsafe { CFRetain(p.as_ptr()) };
        Some(CfRef(p))
    }

    pub(crate) fn as_ptr(&self) -> CFTypeRef {
        self.0.as_ptr()
    }

    pub(crate) fn type_id(&self) -> CFTypeID {
        // SAFETY: we own a live reference.
        unsafe { CFGetTypeID(self.as_ptr()) }
    }

    /// Hand the +1 reference to a caller that will release it (e.g. a consuming argument).
    pub(crate) fn into_raw(self) -> CFTypeRef {
        let p = self.as_ptr();
        std::mem::forget(self);
        p
    }
}

impl Drop for CfRef {
    fn drop(&mut self) {
        // SAFETY: this value owns exactly one reference.
        unsafe { CFRelease(self.as_ptr()) };
    }
}

fn len_index(n: usize) -> Option<CFIndex> {
    CFIndex::try_from(n).ok()
}

/// A new `CFString` from UTF-8 text.
pub(crate) fn string(s: &str) -> Option<CfRef> {
    let len = len_index(s.len())?;
    // SAFETY: valid byte range; the result follows the Create rule.
    unsafe { CfRef::from_create(CFStringCreateWithBytes(ptr::null(), s.as_ptr(), len, UTF8, 0)) }
}

/// Text of a `CFString`; `None` for another type or text that is not valid Unicode
/// (an unpaired UTF-16 surrogate is never replaced by a guess).
///
/// # Safety
/// `p` must be NULL or a live CoreFoundation object.
pub(crate) unsafe fn string_value(p: CFTypeRef) -> Option<String> {
    // SAFETY: non-null live object per the caller.
    if p.is_null() || unsafe { CFGetTypeID(p) } != CFStringGetTypeID() {
        return None;
    }
    // SAFETY: `p` is a CFString.
    let length = unsafe { CFStringGetLength(p) };
    let range = || CFRange { location: 0, length };
    let mut needed: CFIndex = 0;
    // SAFETY: NULL buffer asks only for the byte count; out-pointer is valid.
    let converted = unsafe { CFStringGetBytes(p, range(), UTF8, 0, 0, ptr::null_mut(), 0, &mut needed) };
    if converted != length {
        return None;
    }
    let mut buf = vec![0u8; usize::try_from(needed).ok()?];
    let mut used: CFIndex = 0;
    // SAFETY: buffer of `needed` writable bytes.
    let converted = unsafe { CFStringGetBytes(p, range(), UTF8, 0, 0, buf.as_mut_ptr(), needed, &mut used) };
    if converted != length {
        return None;
    }
    buf.truncate(usize::try_from(used).ok()?);
    String::from_utf8(buf).ok()
}

/// A new `CFData` holding a copy of `bytes`.
pub(crate) fn data(bytes: &[u8]) -> Option<CfRef> {
    let len = len_index(bytes.len())?;
    // SAFETY: valid byte range; CFDataCreate copies it.
    unsafe { CfRef::from_create(CFDataCreate(ptr::null(), bytes.as_ptr(), len)) }
}

/// Bytes of a `CFData`; `None` for another type.
///
/// # Safety
/// `p` must be NULL or a live CoreFoundation object.
pub(crate) unsafe fn data_value(p: CFTypeRef) -> Option<Vec<u8>> {
    // SAFETY: non-null live object per the caller.
    if p.is_null() || unsafe { CFGetTypeID(p) } != CFDataGetTypeID() {
        return None;
    }
    // SAFETY: `p` is a CFData.
    let (bytes, len) = unsafe { (CFDataGetBytePtr(p), CFDataGetLength(p)) };
    let len = usize::try_from(len).ok()?;
    if len == 0 {
        return Some(Vec::new());
    }
    // SAFETY: CFData guarantees `len` readable bytes at `bytes` while the object lives.
    Some(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec())
}

/// A file `CFURL` for raw path bytes (no Unicode interpretation or normalization).
pub(crate) fn file_url(path: &[u8], is_dir: bool) -> Option<CfRef> {
    let len = len_index(path.len())?;
    // SAFETY: valid byte range; the result follows the Create rule.
    unsafe { CfRef::from_create(CFURLCreateFromFileSystemRepresentation(ptr::null(), path.as_ptr(), len, Boolean::from(is_dir))) }
}

/// The raw file-system path bytes of a file `CFURL`.
///
/// # Safety
/// `url` must be NULL or a live CoreFoundation object.
pub(crate) unsafe fn file_url_path(url: CFTypeRef) -> Option<Vec<u8>> {
    // SAFETY: non-null live object per the caller.
    if url.is_null() || unsafe { CFGetTypeID(url) } != CFURLGetTypeID() {
        return None;
    }
    let mut cap = 4096usize;
    while cap <= 1 << 16 {
        let mut buf = vec![0u8; cap];
        // SAFETY: `url` is a CFURL; the buffer holds `cap` writable bytes and receives a NUL-terminated path.
        if unsafe { CFURLGetFileSystemRepresentation(url, 1, buf.as_mut_ptr(), len_index(cap)?) } != 0 {
            let end = buf.iter().position(|&b| b == 0)?;
            buf.truncate(end);
            return Some(buf);
        }
        cap *= 4;
    }
    None
}

/// Description of a `CFErrorRef` out-parameter, releasing it.
///
/// # Safety
/// `err` must be NULL or an owned (+1) `CFErrorRef`.
unsafe fn take_error(err: CFTypeRef) -> String {
    // SAFETY: owned error per the caller; released by the wrapper.
    let Some(err) = (unsafe { CfRef::from_create(err) }) else { return "unknown CoreFoundation error".into() };
    // SAFETY: live CFError; the description follows the Copy rule.
    let desc = unsafe { CfRef::from_create(CFErrorCopyDescription(err.as_ptr())) };
    // SAFETY: live CFString or None.
    desc.and_then(|d| unsafe { string_value(d.as_ptr()) }).unwrap_or_else(|| "unknown CoreFoundation error".into())
}

/// A property list value. Dates are Unix seconds (CoreFoundation keeps seconds since 2001, so
/// sub-second values may differ in the last bit after a round trip).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plist {
    String(String),
    Integer(i64),
    Real(f64),
    Bool(bool),
    Date(f64),
    Data(Vec<u8>),
    Array(Vec<Plist>),
    Dict(BTreeMap<String, Plist>),
}

impl Plist {
    /// Value of `key` when this is a dictionary.
    pub(crate) fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(d) => d.get(key),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Plist::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Serialization format (`CFPropertyListFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlistFormat {
    /// Legacy text; readable, never written.
    OpenStep,
    Xml,
    Binary,
}

impl PlistFormat {
    fn raw(self) -> CFIndex {
        match self {
            PlistFormat::OpenStep => 1,
            PlistFormat::Xml => 100,
            PlistFormat::Binary => 200,
        }
    }

    fn from_raw(v: CFIndex) -> Option<PlistFormat> {
        match v {
            1 => Some(PlistFormat::OpenStep),
            100 => Some(PlistFormat::Xml),
            200 => Some(PlistFormat::Binary),
            _ => None,
        }
    }
}

/// Convert a live property list object (Get rule: not consumed) into a [`Plist`].
///
/// # Safety
/// `p` must be NULL or a live CoreFoundation object.
pub(crate) unsafe fn to_plist(p: CFTypeRef) -> Result<Plist, String> {
    // SAFETY: forwarded caller contract.
    unsafe { convert(p, 0) }
}

/// # Safety
/// `p` must be NULL or a live CoreFoundation object.
unsafe fn convert(p: CFTypeRef, depth: usize) -> Result<Plist, String> {
    if p.is_null() {
        return Err("null property list value".into());
    }
    if depth > MAX_DEPTH {
        return Err("property list nested too deeply".into());
    }
    // SAFETY: non-null live object per the caller.
    let ty = unsafe { CFGetTypeID(p) };
    if ty == CFStringGetTypeID() {
        // SAFETY: live CFString.
        return unsafe { string_value(p) }.map(Plist::String).ok_or_else(|| "string is not valid Unicode".into());
    }
    if ty == CFBooleanGetTypeID() {
        // SAFETY: live CFBoolean.
        return Ok(Plist::Bool(unsafe { CFBooleanGetValue(p) } != 0));
    }
    if ty == CFNumberGetTypeID() {
        // SAFETY: live CFNumber.
        if unsafe { CFNumberIsFloatType(p) } != 0 {
            let mut v = 0f64;
            // SAFETY: out-pointer matches kCFNumberFloat64Type.
            let ok = unsafe { CFNumberGetValue(p, NUMBER_FLOAT64, (&mut v as *mut f64).cast()) };
            return if ok != 0 { Ok(Plist::Real(v)) } else { Err("real number not representable".into()) };
        }
        let mut v = 0i64;
        // SAFETY: out-pointer matches kCFNumberSInt64Type.
        let ok = unsafe { CFNumberGetValue(p, NUMBER_SINT64, (&mut v as *mut i64).cast()) };
        return if ok != 0 { Ok(Plist::Integer(v)) } else { Err("integer outside the signed 64-bit range".into()) };
    }
    if ty == CFDateGetTypeID() {
        // SAFETY: live CFDate.
        return Ok(Plist::Date(unsafe { CFDateGetAbsoluteTime(p) } + ABSOLUTE_TIME_1970));
    }
    if ty == CFDataGetTypeID() {
        // SAFETY: live CFData.
        return unsafe { data_value(p) }.map(Plist::Data).ok_or_else(|| "unreadable data".into());
    }
    if ty == CFArrayGetTypeID() {
        // SAFETY: live CFArray.
        let n = unsafe { CFArrayGetCount(p) };
        let mut out = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
        for i in 0..n {
            // SAFETY: index within the array's count; elements live as long as the array.
            out.push(unsafe { convert(CFArrayGetValueAtIndex(p, i), depth + 1) }?);
        }
        return Ok(Plist::Array(out));
    }
    if ty == CFDictionaryGetTypeID() {
        // SAFETY: live CFDictionary.
        let n = usize::try_from(unsafe { CFDictionaryGetCount(p) }).unwrap_or(0);
        let mut keys: Vec<CFTypeRef> = vec![ptr::null(); n];
        let mut values: Vec<CFTypeRef> = vec![ptr::null(); n];
        // SAFETY: both arrays hold `count` slots; entries live as long as the dictionary.
        unsafe { CFDictionaryGetKeysAndValues(p, keys.as_mut_ptr(), values.as_mut_ptr()) };
        let mut out = BTreeMap::new();
        for (k, v) in keys.into_iter().zip(values) {
            // SAFETY: live key object owned by the dictionary.
            let key = unsafe { string_value(k) }.ok_or("dictionary key is not a string")?;
            // SAFETY: live value object owned by the dictionary.
            out.insert(key, unsafe { convert(v, depth + 1) }?);
        }
        return Ok(Plist::Dict(out));
    }
    Err(format!("unsupported CoreFoundation type {ty}"))
}

/// A new CoreFoundation property list object for `value`.
pub(crate) fn from_plist(value: &Plist) -> Result<CfRef, String> {
    from_plist_at(value, 0)
}

fn from_plist_at(value: &Plist, depth: usize) -> Result<CfRef, String> {
    if depth > MAX_DEPTH {
        return Err("property list nested too deeply".into());
    }
    let created = match value {
        Plist::String(s) => string(s),
        // SAFETY: the value pointers match the declared number types; Create rule.
        Plist::Integer(i) => unsafe { CfRef::from_create(CFNumberCreate(ptr::null(), NUMBER_SINT64, (i as *const i64).cast())) },
        // SAFETY: as above.
        Plist::Real(f) => unsafe { CfRef::from_create(CFNumberCreate(ptr::null(), NUMBER_FLOAT64, (f as *const f64).cast())) },
        // SAFETY: the boolean singletons are live for the process; Get rule.
        Plist::Bool(b) => unsafe { CfRef::from_get(if *b { kCFBooleanTrue } else { kCFBooleanFalse }) },
        // SAFETY: plain value argument; Create rule.
        Plist::Date(unix) => unsafe { CfRef::from_create(CFDateCreate(ptr::null(), unix - ABSOLUTE_TIME_1970)) },
        Plist::Data(d) => data(d),
        Plist::Array(items) => {
            let owned = items.iter().map(|v| from_plist_at(v, depth + 1)).collect::<Result<Vec<_>, _>>()?;
            let ptrs: Vec<CFTypeRef> = owned.iter().map(CfRef::as_ptr).collect();
            // SAFETY: `ptrs` holds `len` live objects, retained by the CFType callbacks; Create rule.
            unsafe { CfRef::from_create(CFArrayCreate(ptr::null(), ptrs.as_ptr(), len_index(ptrs.len()).ok_or("array too large")?, &raw const kCFTypeArrayCallBacks)) }
        }
        Plist::Dict(map) => {
            let mut keys = Vec::with_capacity(map.len());
            let mut values = Vec::with_capacity(map.len());
            for (k, v) in map {
                keys.push(string(k).ok_or("dictionary key cannot be converted")?);
                values.push(from_plist_at(v, depth + 1)?);
            }
            let kp: Vec<CFTypeRef> = keys.iter().map(CfRef::as_ptr).collect();
            let vp: Vec<CFTypeRef> = values.iter().map(CfRef::as_ptr).collect();
            let n = len_index(kp.len()).ok_or("dictionary too large")?;
            // SAFETY: both arrays hold `n` live objects, retained by the CFType callbacks; Create rule.
            unsafe {
                CfRef::from_create(CFDictionaryCreate(
                    ptr::null(),
                    kp.as_ptr(),
                    vp.as_ptr(),
                    n,
                    &raw const kCFTypeDictionaryKeyCallBacks,
                    &raw const kCFTypeDictionaryValueCallBacks,
                ))
            }
        }
    };
    created.ok_or_else(|| "CoreFoundation could not create the value".into())
}

/// Parse a property list in any format CoreFoundation reads (XML, binary, OpenStep).
pub(crate) fn plist_from_bytes(bytes: &[u8]) -> Result<(Plist, PlistFormat), String> {
    let data = data(bytes).ok_or("cannot wrap the bytes")?;
    let mut format: CFIndex = 0;
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: live CFData and valid out-pointers; the result follows the Create rule, the error too.
    let parsed = unsafe { CFPropertyListCreateWithData(ptr::null(), data.as_ptr(), PLIST_IMMUTABLE, &mut format, &mut err) };
    // SAFETY: owned (+1) result or NULL.
    let Some(parsed) = (unsafe { CfRef::from_create(parsed) }) else {
        // SAFETY: owned (+1) error or NULL.
        return Err(format!("not a property list: {}", unsafe { take_error(err) }));
    };
    let format = PlistFormat::from_raw(format).ok_or("unknown property list format")?;
    // SAFETY: live property list object.
    Ok((unsafe { to_plist(parsed.as_ptr()) }?, format))
}

/// Serialize `value` as XML or binary. OpenStep cannot be written.
pub(crate) fn plist_to_bytes(value: &Plist, format: PlistFormat) -> Result<Vec<u8>, String> {
    if format == PlistFormat::OpenStep {
        return Err("OpenStep property lists cannot be written".into());
    }
    let object = from_plist(value)?;
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: live property list object and valid error out-pointer; Create rule for both.
    let data = unsafe { CFPropertyListCreateData(ptr::null(), object.as_ptr(), format.raw(), 0, &mut err) };
    // SAFETY: owned (+1) data or NULL.
    let Some(data) = (unsafe { CfRef::from_create(data) }) else {
        // SAFETY: owned (+1) error or NULL.
        return Err(format!("cannot serialize the property list: {}", unsafe { take_error(err) }));
    };
    // SAFETY: live CFData.
    unsafe { data_value(data.as_ptr()) }.ok_or_else(|| "serialized data unreadable".into())
}

/// Current user's value of `key` in preference domain `domain` (an application ID such as
/// `com.apple.dock`, or an absolute plist path without `.plist`). `Ok(None)` when unset.
pub(crate) fn pref_get(domain: &str, key: &str) -> Result<Option<Plist>, String> {
    let (d, k) = (string(domain).ok_or("bad domain")?, string(key).ok_or("bad key")?);
    // SAFETY: live CFStrings; Copy rule.
    let value = unsafe { CfRef::from_create(CFPreferencesCopyAppValue(k.as_ptr(), d.as_ptr())) };
    // SAFETY: live property list object.
    value.map(|v| unsafe { to_plist(v.as_ptr()) }).transpose()
}

/// Set (`Some`) or remove (`None`) the current user's `key` in `domain`, then synchronize so the
/// change is written. A failed synchronization is an error: the value may not have persisted.
pub(crate) fn pref_set(domain: &str, key: &str, value: Option<&Plist>) -> Result<(), String> {
    let (d, k) = (string(domain).ok_or("bad domain")?, string(key).ok_or("bad key")?);
    let v = value.map(from_plist).transpose()?;
    // SAFETY: live CFStrings and a live value or NULL (removal); CFPreferences retains what it keeps.
    unsafe { CFPreferencesSetAppValue(k.as_ptr(), v.as_ref().map_or(ptr::null(), CfRef::as_ptr), d.as_ptr()) };
    // SAFETY: live CFString.
    if unsafe { CFPreferencesAppSynchronize(d.as_ptr()) } != 0 { Ok(()) } else { Err(format!("preferences domain {domain} could not be synchronized")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::testfs::Fixture;

    fn sample() -> Plist {
        let mut inner = BTreeMap::new();
        inner.insert("Label".to_owned(), Plist::String("com.example.agent — ü".into()));
        inner.insert("RunAtLoad".to_owned(), Plist::Bool(true));
        inner.insert("KeepAlive".to_owned(), Plist::Bool(false));
        let mut top = BTreeMap::new();
        top.insert("dict".to_owned(), Plist::Dict(inner));
        top.insert("args".to_owned(), Plist::Array(vec![Plist::String("/usr/bin/true".into()), Plist::String(String::new())]));
        top.insert("neg".to_owned(), Plist::Integer(-42));
        top.insert("big".to_owned(), Plist::Integer(i64::MAX));
        top.insert("real".to_owned(), Plist::Real(1.5));
        top.insert("date".to_owned(), Plist::Date(1_700_000_000.0));
        top.insert("data".to_owned(), Plist::Data(vec![0, 0xff, 0x80, b'\n']));
        top.insert("empty".to_owned(), Plist::Data(Vec::new()));
        Plist::Dict(top)
    }

    #[test]
    fn plist_round_trips_as_xml_and_binary() {
        for format in [PlistFormat::Xml, PlistFormat::Binary] {
            let bytes = plist_to_bytes(&sample(), format).unwrap();
            match format {
                PlistFormat::Xml => assert!(bytes.starts_with(b"<?xml"), "{}", String::from_utf8_lossy(&bytes)),
                _ => assert!(bytes.starts_with(b"bplist00")),
            }
            let (back, read_as) = plist_from_bytes(&bytes).unwrap();
            assert_eq!(read_as, format);
            assert_eq!(back, sample());
            assert_eq!(back.get("dict").and_then(|d| d.get("Label")).and_then(Plist::as_str), Some("com.example.agent — ü"));
        }
        assert!(plist_to_bytes(&sample(), PlistFormat::OpenStep).is_err());
    }

    #[test]
    fn malformed_and_foreign_plists_are_errors() {
        assert!(plist_from_bytes(b"<?xml version=\"1.0\"?><plist><dict><key>a</key>").is_err());
        assert!(plist_from_bytes(b"bplist00\xff\xff").is_err());
        let (v, f) = plist_from_bytes(b"{ a = (1, 2); }").unwrap();
        assert_eq!(f, PlistFormat::OpenStep);
        assert_eq!(v.get("a"), Some(&Plist::Array(vec![Plist::String("1".into()), Plist::String("2".into())])));
    }

    #[test]
    fn strings_and_urls_keep_raw_bytes() {
        let s = string("naïve ✓").unwrap();
        // SAFETY: live CFString.
        assert_eq!(unsafe { string_value(s.as_ptr()) }.as_deref(), Some("naïve ✓"));
        // SAFETY: a CFString is not CFData.
        assert_eq!(unsafe { data_value(s.as_ptr()) }, None);
        let raw = b"/private/tmp/cm-\xff\xfe-name".to_vec();
        let url = file_url(&raw, false).unwrap();
        // SAFETY: live CFURL.
        assert_eq!(unsafe { file_url_path(url.as_ptr()) }, Some(raw));
    }

    #[test]
    fn preferences_set_read_and_remove_in_a_private_domain() {
        let fx = Fixture::new("cf-prefs");
        // An absolute domain keeps the test out of ~/Library/Preferences.
        let domain = fx.path().join("com.cleanmanager.test").to_str().unwrap().to_owned();
        assert_eq!(pref_get(&domain, "Key").unwrap(), None);
        pref_set(&domain, "Key", Some(&Plist::Integer(7))).unwrap();
        assert_eq!(pref_get(&domain, "Key").unwrap(), Some(Plist::Integer(7)));
        let written = std::fs::read(format!("{domain}.plist")).unwrap();
        assert_eq!(plist_from_bytes(&written).unwrap().0.get("Key"), Some(&Plist::Integer(7)));
        pref_set(&domain, "Key", None).unwrap();
        assert_eq!(pref_get(&domain, "Key").unwrap(), None);
    }
}
