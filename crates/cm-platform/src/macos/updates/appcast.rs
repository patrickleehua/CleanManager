//! Sparkle appcast parsing with Foundation's `NSXMLDocument` (no XML crate): `/rss/channel/item` with Sparkle's
//! namespaced elements, falling back to the legacy `<enclosure sparkle:...>` attributes. External entities are
//! never loaded (`NSXMLNodeLoadExternalEntitiesNever`), the input is bounded and so is the item count.
//!
//! Every object below comes from `NSXMLDocument` methods that return +0 (autoreleased or owned by the document);
//! they are used only inside the function's autorelease pool while the document is alive, and only copied text
//! leaves it.

use std::ffi::c_void;
use std::ptr;

use cm_core::updates::mac::{APPCAST_MAX, AppcastItem, SPARKLE_NS};

use crate::macos::ffi::cf;
use crate::macos::ffi::objc::{AutoreleasePool, Id, Retained, Sel, class, ns_string, ns_string_value, sel};

/// `NSXMLNodeLoadExternalEntitiesNever` (`NSXMLNodeOptions.h`).
const LOAD_EXTERNAL_ENTITIES_NEVER: usize = 1 << 19;
/// Items read from one appcast.
const MAX_ITEMS: usize = 2000;
/// Characters kept of one text value.
const MAX_TEXT: usize = 256;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// `void objc_msgSend(void);` (`objc/message.h`): cast to the method's exact type before calling.
    fn objc_msgSend();
}

/// `objc_msgSend` as `unsafe extern "C" fn(id, SEL, args...) -> ret` (on arm64 it must be called through the
/// method's exact C type; see `ffi::objc`).
macro_rules! send_fn {
    (($($arg:ty),*) -> $ret:ty) => {
        // SAFETY: function pointers share one size and representation; each call site sends only a selector whose
        // implementation has exactly this C signature (declared next to it).
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $arg)*) -> $ret>(objc_msgSend as unsafe extern "C" fn())
        }
    };
}

/// `- (NSUInteger)count;` and `- (id)objectAtIndex:(NSUInteger)index;` of an `NSArray`.
///
/// # Safety
/// `array` must be nil or a live `NSArray`.
unsafe fn array_items(array: Id, limit: usize) -> Vec<Id> {
    if array.is_null() {
        return Vec::new();
    }
    let count = send_fn!(() -> usize);
    let at = send_fn!((usize) -> Id);
    // SAFETY: live NSArray per the caller.
    let n = unsafe { count(array, sel(c"count")) };
    // SAFETY: index below `count`; the element is +0, owned by the array.
    (0..n.min(limit)).map(|i| unsafe { at(array, sel(c"objectAtIndex:"), i) }).filter(|o| !o.is_null()).collect()
}

/// `- (NSString *)stringValue;` of an `NSXMLNode`, trimmed and bounded; empty is `None`.
///
/// # Safety
/// `node` must be nil or a live `NSXMLNode`.
unsafe fn node_text(node: Id) -> Option<String> {
    if node.is_null() {
        return None;
    }
    let f = send_fn!(() -> Id);
    // SAFETY: live node; the string is +0 and copied at once.
    let s = unsafe { ns_string_value(f(node, sel(c"stringValue"))) }?;
    let t: String = s.trim().chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
    (!t.is_empty()).then_some(t)
}

/// `- (NSString *)name;` of an `NSXMLNode`.
///
/// # Safety
/// `node` must be a live `NSXMLNode`.
unsafe fn node_name(node: Id) -> Option<String> {
    let f = send_fn!(() -> Id);
    // SAFETY: live node; +0 string copied at once.
    unsafe { ns_string_value(f(node, sel(c"name"))) }
}

/// Child elements named `name` (no namespace): `- (NSArray<NSXMLElement *> *)elementsForName:(NSString *)name;`
///
/// # Safety
/// `el` must be a live `NSXMLElement`.
unsafe fn children(el: Id, name: &str, limit: usize) -> Vec<Id> {
    let Some(n) = ns_string(name) else { return Vec::new() };
    let f = send_fn!((Id) -> Id);
    // SAFETY: live element and NSString; the array is +0.
    unsafe { array_items(f(el, sel(c"elementsForName:"), n.as_ptr()), limit) }
}

/// Text of the first Sparkle child `<sparkle:local>`, by namespace URI, else by the qualified name.
///
/// # Safety
/// `el` must be a live `NSXMLElement`.
unsafe fn sparkle_child(el: Id, local: &str) -> Option<String> {
    let (Some(l), Some(uri)) = (ns_string(local), ns_string(SPARKLE_NS)) else { return None };
    // `- (NSArray<NSXMLElement *> *)elementsForLocalName:(NSString *)localName URI:(NSString *)URI;`
    let f = send_fn!((Id, Id) -> Id);
    // SAFETY: live element and NSStrings; the array and its elements are +0.
    let by_ns = unsafe { array_items(f(el, sel(c"elementsForLocalName:URI:"), l.as_ptr(), uri.as_ptr()), 1) };
    // SAFETY: live element.
    let by_name = || unsafe { children(el, &format!("sparkle:{local}"), 1) };
    let first = by_ns.first().copied().or_else(|| by_name().first().copied())?;
    // SAFETY: live child element.
    unsafe { node_text(first) }
}

/// Value of the Sparkle attribute `sparkle:local` of `el`, by namespace URI, else by the qualified name.
///
/// # Safety
/// `el` must be nil or a live `NSXMLElement`.
unsafe fn sparkle_attr(el: Id, local: &str) -> Option<String> {
    if el.is_null() {
        return None;
    }
    let (Some(l), Some(uri), Some(q)) = (ns_string(local), ns_string(SPARKLE_NS), ns_string(&format!("sparkle:{local}"))) else { return None };
    // `- (NSXMLNode *)attributeForLocalName:(NSString *)localName URI:(NSString *)URI;`
    let by_ns = send_fn!((Id, Id) -> Id);
    // `- (NSXMLNode *)attributeForName:(NSString *)name;`
    let by_name = send_fn!((Id) -> Id);
    // SAFETY: live element and NSStrings; the attribute node is +0, owned by the element.
    let node = unsafe { by_ns(el, sel(c"attributeForLocalName:URI:"), l.as_ptr(), uri.as_ptr()) };
    // SAFETY: as above.
    let node = if node.is_null() { unsafe { by_name(el, sel(c"attributeForName:"), q.as_ptr()) } } else { node };
    // SAFETY: nil or a live attribute node.
    unsafe { node_text(node) }
}

/// `localizedDescription` of an `NSError` (nil: a generic text).
///
/// # Safety
/// `err` must be nil or a live `NSError`.
unsafe fn error_text(err: Id) -> String {
    if err.is_null() {
        return "unknown parser error".into();
    }
    let f = send_fn!(() -> Id);
    // SAFETY: live NSError; +0 string copied at once.
    unsafe { ns_string_value(f(err, sel(c"localizedDescription"))) }.map(|s| s.chars().take(300).collect()).unwrap_or_else(|| "unknown parser error".into())
}

/// One `<item>`.
///
/// # Safety
/// `item` must be a live `NSXMLElement`.
unsafe fn read_item(item: Id) -> AppcastItem {
    // SAFETY: live element; every helper only reads it.
    unsafe {
        let enclosure = children(item, "enclosure", 1).first().copied().unwrap_or(ptr::null_mut());
        AppcastItem {
            title: children(item, "title", 1).first().and_then(|&t| node_text(t)),
            version: sparkle_child(item, "version").or_else(|| sparkle_attr(enclosure, "version")),
            short_version: sparkle_child(item, "shortVersionString").or_else(|| sparkle_attr(enclosure, "shortVersionString")),
            minimum_system_version: sparkle_child(item, "minimumSystemVersion"),
            maximum_system_version: sparkle_child(item, "maximumSystemVersion"),
            channel: sparkle_child(item, "channel"),
            os: sparkle_attr(enclosure, "os"),
            ed_signature: sparkle_attr(enclosure, "edSignature").is_some(),
        }
    }
}

/// Parse a Sparkle appcast (RSS 2.0 with Sparkle's namespace). Not well-formed XML, another root than `<rss>`, or
/// no `<channel>` is an error; an appcast without items is an empty list.
pub fn parse_appcast(bytes: &[u8]) -> Result<Vec<AppcastItem>, String> {
    if bytes.len() as u64 > APPCAST_MAX {
        return Err(format!("the appcast is larger than {APPCAST_MAX} bytes"));
    }
    let _pool = AutoreleasePool::new();
    let data = cf::data(bytes).ok_or("the appcast could not be copied")?;
    let cls = class(c"NSXMLDocument").ok_or("NSXMLDocument is not available")?;
    let alloc = send_fn!(() -> Id);
    // `- (instancetype)initWithData:(NSData *)data options:(NSXMLNodeOptions)mask error:(NSError **)error;`
    let init = send_fn!((*mut c_void, usize, *mut Id) -> Id);
    // SAFETY: `+alloc` on a class object returns a +1 uninitialized instance (or nil).
    let obj = unsafe { alloc(cls, sel(c"alloc")) };
    if obj.is_null() {
        return Err("NSXMLDocument could not be allocated".into());
    }
    let mut err: Id = ptr::null_mut();
    // SAFETY: the fresh instance is consumed by `init` (+1 result, or nil after releasing it); `data` is a live CFData,
    // toll-free bridged to NSData; `err` is a valid out-pointer that receives an autoreleased NSError on failure.
    let doc = unsafe { Retained::from_owned(init(obj, sel(c"initWithData:options:error:"), data.as_ptr().cast_mut(), LOAD_EXTERNAL_ENTITIES_NEVER, &mut err)) };
    let Some(doc) = doc else {
        // SAFETY: nil or the autoreleased NSError from `init`, alive in `_pool`.
        return Err(format!("the appcast is not well-formed XML: {}", unsafe { error_text(err) }));
    };
    // `- (NSXMLElement *)rootElement;`
    let root_of = send_fn!(() -> Id);
    // SAFETY: live document; the root is +0, owned by the document.
    let root = unsafe { root_of(doc.as_ptr(), sel(c"rootElement")) };
    if root.is_null() {
        return Err("the appcast has no root element".into());
    }
    // SAFETY: live root element.
    let name = unsafe { node_name(root) }.unwrap_or_default();
    if name != "rss" {
        return Err(format!("not an RSS appcast (root element <{name}>)"));
    }
    // SAFETY: live root element.
    let channel = unsafe { children(root, "channel", 1) }.first().copied().ok_or("the appcast has no <channel>")?;
    // SAFETY: live channel element; each item is a live element of it.
    let items = unsafe { children(channel, "item", MAX_ITEMS) };
    // SAFETY: as above.
    Ok(items.into_iter().map(|i| unsafe { read_item(i) }).collect())
}
