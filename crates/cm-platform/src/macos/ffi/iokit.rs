//! IOKit registry basics (`IOKit/IOKitLib.h`): find the first (or every) service matching a class
//! name, read one of its properties as a [`Plist`] and its registry entry ID; and the power-source
//! snapshot (`IOKit/ps/IOPowerSources.h`). Read-only.

use std::ffi::{CStr, c_char};
use std::ptr;

use super::cf::{self, CFTypeRef, CfRef, Plist};

/// `typedef mach_port_t io_object_t;` (`natural_t`, 32-bit)
type IoObject = u32;
/// Upper bound on services taken from one matching iterator (defends against a runaway registry).
const MAX_SERVICES: usize = 4096;
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
    /// `kern_return_t IOServiceGetMatchingServices(mach_port_t mainPort, CFDictionaryRef CF_RELEASES_ARGUMENT matching, io_iterator_t *existing);`
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFTypeRef, existing: *mut IoObject) -> i32;
    /// `io_object_t IOIteratorNext(io_iterator_t iterator);`
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    /// `kern_return_t IORegistryEntryGetRegistryEntryID(io_registry_entry_t entry, uint64_t *entryID);`
    fn IORegistryEntryGetRegistryEntryID(entry: IoObject, id: *mut u64) -> i32;
    /// `CFTypeRef IOPSCopyPowerSourcesInfo(void);` (+1)
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    /// `CFArrayRef IOPSCopyPowerSourcesList(CFTypeRef blob);` (+1)
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    /// `CFDictionaryRef IOPSGetPowerSourceDescription(CFTypeRef blob, CFTypeRef ps);` (+0, owned by `blob`)
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    /// `CFStringRef IOPSGetProvidingPowerSourceType(CFTypeRef snapshot);` (+0, owned by `snapshot`)
    fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFTypeRef;
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

    /// Every service whose class matches `class` (subclasses included), in registry order.
    pub(crate) fn all_matching(class: &CStr) -> Vec<Service> {
        // SAFETY: NUL-terminated class name; the matching dictionary is +1.
        let matching = unsafe { IOServiceMatching(class.as_ptr()) };
        if matching.is_null() {
            return Vec::new();
        }
        let mut iter: IoObject = 0;
        // SAFETY: the call consumes the matching dictionary in every case; `iter` is +1 on success.
        if unsafe { IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, matching, &mut iter) } != 0 || iter == 0 {
            return Vec::new();
        }
        let iter = Service(iter);
        let mut out = Vec::new();
        while out.len() < MAX_SERVICES {
            // SAFETY: live iterator; each returned object is +1 (0 at the end).
            let next = unsafe { IOIteratorNext(iter.0) };
            if next == 0 {
                break;
            }
            out.push(Service(next));
        }
        out
    }

    /// The registry entry ID: stable for the entry's lifetime, never reused while the system runs.
    pub(crate) fn registry_id(&self) -> Option<u64> {
        let mut id = 0u64;
        // SAFETY: owned entry and a valid out-pointer.
        (unsafe { IORegistryEntryGetRegistryEntryID(self.0, &mut id) } == 0).then_some(id)
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

/// The power-source snapshot: which source powers the machine (`"AC Power"`, `"Battery Power"`,
/// `"UPS Power"`) and each source's description dictionary. `None` when no snapshot is available.
pub(crate) fn power_sources() -> Option<(Option<String>, Vec<Plist>)> {
    // SAFETY: no arguments; Create rule.
    let blob = unsafe { CfRef::from_create(IOPSCopyPowerSourcesInfo()) }?;
    // SAFETY: live snapshot; the providing type is borrowed from it and converted before `blob` drops.
    let providing = unsafe { cf::string_value(IOPSGetProvidingPowerSourceType(blob.as_ptr())) };
    // SAFETY: live snapshot; Create rule.
    let list = unsafe { CfRef::from_create(IOPSCopyPowerSourcesList(blob.as_ptr())) };
    let mut sources = Vec::new();
    if let Some(list) = list
        && list.type_id() == cf::CFArrayGetTypeID()
    {
        // SAFETY: live CFArray.
        let n = unsafe { cf::CFArrayGetCount(list.as_ptr()) };
        for i in 0..n {
            // SAFETY: index within the array; elements live as long as the array, descriptions as long as `blob`.
            let desc = unsafe { IOPSGetPowerSourceDescription(blob.as_ptr(), cf::CFArrayGetValueAtIndex(list.as_ptr(), i)) };
            if !desc.is_null()
                // SAFETY: live CF object borrowed from `blob`.
                && let Ok(p) = unsafe { cf::to_plist(desc) }
            {
                sources.push(p);
            }
        }
    }
    Some((providing, sources))
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
        assert!(expert.registry_id().is_some_and(|id| id != 0));
        let all = Service::all_matching(c"IOPlatformExpertDevice");
        assert!(!all.is_empty());
        assert!(all.iter().any(|s| s.registry_id() == expert.registry_id()));
        assert!(Service::all_matching(c"CMNoSuchIOClass").is_empty());
    }

    #[test]
    fn power_source_snapshot_names_the_providing_source() {
        let (providing, sources) = power_sources().expect("power source snapshot");
        eprintln!("providing={providing:?} sources={}", sources.len());
        assert!(providing.is_some() || sources.is_empty(), "{providing:?} {sources:?}");
    }
}
