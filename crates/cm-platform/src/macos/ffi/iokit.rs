//! IOKit registry basics (`IOKit/IOKitLib.h`): find the first service matching a class name and
//! read one of its properties as a [`Plist`]. Read-only; later status work builds on this.

use std::ffi::{CStr, c_char};
use std::ptr;

use super::cf::{self, CFTypeRef, CfRef, Plist};

/// `typedef mach_port_t io_object_t;` (`natural_t`, 32-bit)
type IoObject = u32;
/// `kIOMainPortDefault` / `kIOMasterPortDefault` are `MACH_PORT_NULL`.
const MAIN_PORT_DEFAULT: u32 = 0;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// `CFMutableDictionaryRef IOServiceMatching(const char *name);` (+1, consumed below)
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    /// `io_service_t IOServiceGetMatchingService(mach_port_t mainPort, CFDictionaryRef CF_RELEASES_ARGUMENT matching);`
    fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> IoObject;
    /// `CFTypeRef IORegistryEntryCreateCFProperty(io_registry_entry_t entry, CFStringRef key, CFAllocatorRef allocator, IOOptionBits options);`
    fn IORegistryEntryCreateCFProperty(entry: IoObject, key: CFTypeRef, allocator: CFTypeRef, options: u32) -> CFTypeRef;
    /// `kern_return_t IOObjectRelease(io_object_t object);`
    fn IOObjectRelease(object: IoObject) -> i32;
}

/// An owned registry entry, released once on drop.
#[derive(Debug)]
pub(crate) struct Service(IoObject);

impl Service {
    /// The first service whose class matches `class` (e.g. `IOPlatformExpertDevice`).
    pub(crate) fn matching(class: &CStr) -> Option<Service> {
        // SAFETY: NUL-terminated class name; the matching dictionary is +1.
        let matching = unsafe { IOServiceMatching(class.as_ptr()) };
        if matching.is_null() {
            return None;
        }
        // SAFETY: the call consumes the matching dictionary in every case; the result is +1 or 0.
        let service = unsafe { IOServiceGetMatchingService(MAIN_PORT_DEFAULT, matching) };
        (service != 0).then_some(Service(service))
    }

    /// Property `key` of this entry. `None` when absent; `Some(Err)` when it is not a property list value.
    pub(crate) fn property(&self, key: &str) -> Option<Result<Plist, String>> {
        let key = cf::string(key)?;
        // SAFETY: owned entry, live CFString key, default allocator; Create rule.
        let value = unsafe { CfRef::from_create(IORegistryEntryCreateCFProperty(self.0, key.as_ptr(), ptr::null(), 0)) }?;
        // SAFETY: live CF object.
        Some(unsafe { cf::to_plist(value.as_ptr()) })
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: this value owns the entry reference and releases it once.
        unsafe { IOObjectRelease(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_expert_reports_a_platform_uuid() {
        let expert = Service::matching(c"IOPlatformExpertDevice").expect("platform expert");
        let uuid = expert.property("IOPlatformUUID").expect("present").unwrap();
        assert_eq!(uuid.as_str().map(str::len), Some(36), "{uuid:?}");
        assert!(expert.property("CMNoSuchProperty").is_none());
        assert!(Service::matching(c"CMNoSuchIOClass").is_none());
    }
}
