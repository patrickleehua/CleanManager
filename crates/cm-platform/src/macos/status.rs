//! macOS status sampler (parity 12.1, STS-01..03, STS-07, N13).
//!
//! Interfaces, all public and read-only:
//! - CPU: `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` tick deltas per core between two readings; `getloadavg`.
//! - Memory: `host_statistics64(HOST_VM_INFO64)` page counts, `hw.memsize`, `kern.memorystatus_vm_pressure_level`
//!   and `vm.swapusage`. "Used" is app memory (internal minus purgeable pages) + wired + compressed, as Activity
//!   Monitor reports it.
//! - Disk: `statfs` on the data volume for capacity and free space, `CFURL`'s "available for important usage" for
//!   the space macOS can make free (purgeable); I/O from every `IOBlockStorageDriver`'s `Statistics` dictionary.
//! - Network (STS-07): `sysctl(NET_RT_IFLIST2)` `if_msghdr2` records with 64-bit byte counters, classified (loopback
//!   excluded; utun/ipsec/ppp/gif/stf tunnels and bridge/vmnet/anpi/awdl virtual adapters listed apart) and
//!   deduplicated by `cm_core::status::network`.
//! - Battery: `IOPSCopyPowerSourcesInfo` (charge, power source) and the `AppleSmartBattery` registry entry (cycle
//!   count, design and maximum capacity → health).
//! - GPU: the busiest `IOAccelerator`'s `PerformanceStatistics` utilization, or unknown.
//! - Zombie processes (N13): processes in state `SZOMB` from the libproc snapshot (macOS only).
//!
//! Temperatures and fan speeds are only published through the System Management Controller and HID sensor services,
//! which are private interfaces: those metrics are `unavailable` with that reason, never 0.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, c_int};
use std::time::{Duration, Instant};

use cm_core::capability::CapabilityState;
use cm_core::status::network::{Classification, InterfaceFacts, InterfaceRate, NetworkTotals, classify, if_type};
use cm_core::status::{Metric, StatusSnapshot, TopProcess, names};

use super::ffi::cf::{self, CFTypeRef, CfRef, Plist};
use super::ffi::iokit::{self, Service};
use super::processes;
use super::procs;

/// Capability rows this module backs (name, state, reason); concatenated by `HostPlatform::capabilities`.
pub(crate) const CAPS: &[(&str, CapabilityState, &str)] = &[(
    "status.metrics",
    CapabilityState::Unverified,
    "Host/VM statistics, statfs, IOKit block-storage and accelerator statistics, NET_RT_IFLIST2 counters, power sources and zombie count are implemented and tested on hosted runners only (no battery, sensors or real GPU there); temperatures and fans are unavailable (private SMC/HID interfaces)",
)];

/// Fixed window for the first sample of rate metrics (CPU, disk, network).
pub const FIRST_WINDOW: Duration = Duration::from_millis(1000);

/// Why temperatures and fans are never reported.
pub const SENSOR_REASON: &str = "macOS publishes temperatures and fan speeds only through the System Management Controller (SMC) and HID sensor services, which are private interfaces CleanManager does not use";

// ---------------------------------------------------------------------------------------------
// Mach

type MachPort = u32;

/// `PROCESSOR_CPU_LOAD_INFO` (`<mach/processor_info.h>`).
const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
/// `CPU_STATE_MAX` and `CPU_STATE_IDLE` (`<mach/machine.h>`).
const CPU_STATE_MAX: usize = 4;
const CPU_STATE_IDLE: usize = 2;
/// `HOST_VM_INFO64` (`<mach/host_info.h>`).
const HOST_VM_INFO64: c_int = 4;

/// `struct vm_statistics64` (`<mach/vm_statistics.h>`), revision 1: the fields every supported macOS fills.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Default)]
struct VmStatistics64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

const _: () = assert!(size_of::<VmStatistics64>() == 152);
/// `HOST_VM_INFO64_COUNT` for the struct above (in `integer_t` units).
const HOST_VM_INFO64_COUNT: u32 = (size_of::<VmStatistics64>() / size_of::<i32>()) as u32;

unsafe extern "C" {
    /// `mach_port_t mach_host_self(void);` (`<mach/mach_init.h>`): a send right to the host port.
    fn mach_host_self() -> MachPort;
    /// `extern mach_port_t mach_task_self_;` (`<mach/mach_init.h>`)
    static mach_task_self_: MachPort;
    /// `kern_return_t mach_port_deallocate(ipc_space_t task, mach_port_name_t name);`
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> c_int;
    /// `kern_return_t host_processor_info(host_t host, processor_flavor_t flavor, natural_t *out_processor_count,
    /// processor_info_array_t *out_processor_info, mach_msg_type_number_t *out_processor_infoCnt);`
    fn host_processor_info(host: MachPort, flavor: c_int, count: *mut u32, info: *mut *mut i32, info_count: *mut u32) -> c_int;
    /// `kern_return_t host_statistics64(host_t host_priv, host_flavor_t flavor, host_info64_t host_info64_out,
    /// mach_msg_type_number_t *host_info64_outCnt);`
    fn host_statistics64(host: MachPort, flavor: c_int, info: *mut i32, count: *mut u32) -> c_int;
    /// `kern_return_t vm_deallocate(vm_map_t target_task, vm_address_t address, vm_size_t size);`
    fn vm_deallocate(task: MachPort, address: usize, size: usize) -> c_int;
}

fn task_self() -> MachPort {
    // SAFETY: the process's task port name, set once by the runtime before main.
    unsafe { mach_task_self_ }
}

/// The host port send right, released once on drop.
struct Host(MachPort);

impl Host {
    fn new() -> Host {
        // SAFETY: no arguments; returns a send right we own.
        Host(unsafe { mach_host_self() })
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: our own send right, released once.
            unsafe { mach_port_deallocate(task_self(), self.0) };
        }
    }
}

/// Cumulative `[user, system, idle, nice]` ticks per logical CPU.
type Ticks = Vec<[u32; CPU_STATE_MAX]>;

fn cpu_ticks(host: &Host) -> Option<Ticks> {
    let (mut count, mut info, mut words) = (0u32, std::ptr::null_mut::<i32>(), 0u32);
    // SAFETY: valid out-pointers; on success `info` is a vm allocation of `words` integers we must deallocate.
    if unsafe { host_processor_info(host.0, PROCESSOR_CPU_LOAD_INFO, &mut count, &mut info, &mut words) } != 0 || info.is_null() {
        return None;
    }
    let n = count as usize;
    let ticks: Ticks = if (words as usize) >= n * CPU_STATE_MAX {
        // SAFETY: the kernel returned `words` >= n * CPU_STATE_MAX integers at `info`.
        let all = unsafe { std::slice::from_raw_parts(info.cast::<u32>(), n * CPU_STATE_MAX) };
        all.chunks_exact(CPU_STATE_MAX).map(|c| [c[0], c[1], c[2], c[3]]).collect()
    } else {
        Vec::new()
    };
    // SAFETY: the allocation returned above, released once; `ticks` holds copies.
    unsafe { vm_deallocate(task_self(), info as usize, words as usize * size_of::<i32>()) };
    (!ticks.is_empty()).then_some(ticks)
}

/// Busy share (0..=100) of the ticks between two readings of one CPU (or of all CPUs summed). Tick counters are
/// 32-bit and wrap; a wrapped counter still gives the right difference. `None` when no tick elapsed.
pub(crate) fn busy_percent(prev: &[[u32; CPU_STATE_MAX]], now: &[[u32; CPU_STATE_MAX]]) -> Option<f64> {
    if prev.len() != now.len() || now.is_empty() {
        return None;
    }
    let (mut total, mut idle) = (0u64, 0u64);
    for (a, b) in prev.iter().zip(now) {
        for s in 0..CPU_STATE_MAX {
            let d = u64::from(b[s].wrapping_sub(a[s]));
            total += d;
            if s == CPU_STATE_IDLE {
                idle += d;
            }
        }
    }
    (total > 0).then(|| round1((total - idle) as f64 * 100.0 / total as f64))
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

// ---------------------------------------------------------------------------------------------
// sysctl

fn sysctl_bytes(name: &CStr, buf: &mut [u8]) -> Option<usize> {
    let mut len = buf.len();
    // SAFETY: NUL-terminated name; `buf` holds `len` writable bytes; read-only query.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0).then_some(len)
}

/// An integer sysctl of 4 or 8 bytes.
fn sysctl_int(name: &CStr) -> Option<u64> {
    let mut b = [0u8; 8];
    match sysctl_bytes(name, &mut b)? {
        8 => Some(u64::from_ne_bytes(b)),
        4 => Some(u64::from(u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))),
        _ => None,
    }
}

fn swap_usage() -> Option<libc::xsw_usage> {
    // SAFETY: zeroed POD.
    let mut x: libc::xsw_usage = unsafe { std::mem::zeroed() };
    let mut len = size_of::<libc::xsw_usage>();
    // SAFETY: NUL-terminated name; out-buffer is a properly sized xsw_usage.
    let rc = unsafe { libc::sysctlbyname(c"vm.swapusage".as_ptr(), (&mut x as *mut libc::xsw_usage).cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && len == size_of::<libc::xsw_usage>()).then_some(x)
}

fn vm_statistics(host: &Host) -> Option<VmStatistics64> {
    let mut v = VmStatistics64::default();
    let mut count = HOST_VM_INFO64_COUNT;
    // SAFETY: `v` is a revision-1 vm_statistics64 of `count` integers; the kernel writes at most that many.
    let kr = unsafe { host_statistics64(host.0, HOST_VM_INFO64, (&mut v as *mut VmStatistics64).cast(), &mut count) };
    (kr == 0).then_some(v)
}

fn page_size() -> u64 {
    // SAFETY: plain query.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(p).ok().filter(|&p| p > 0).unwrap_or(4096)
}

/// `kern.memorystatus_vm_pressure_level` (`<sys/kern_memorystatus.h>`): 1 normal, 2 warning, 4 critical.
pub(crate) fn pressure_name(level: u64) -> Option<&'static str> {
    match level {
        1 => Some("normal"),
        2 => Some("warning"),
        4 => Some("critical"),
        _ => None,
    }
}

fn memory_metrics(host: &Host, out: &mut Vec<Metric>) {
    let total = sysctl_int(c"hw.memsize").filter(|&t| t > 0);
    match (total, vm_statistics(host)) {
        (Some(total), Some(v)) => {
            let page = page_size();
            let pages = |n: u32| u64::from(n) * page;
            let app = pages(v.internal_page_count.saturating_sub(v.purgeable_count));
            let wired = pages(v.wire_count);
            let compressed = pages(v.compressor_page_count);
            let used = app.saturating_add(wired).saturating_add(compressed).min(total);
            out.push(Metric::value("memory.total_bytes", total as f64, "bytes"));
            out.push(Metric::noted("memory.available_bytes", (total - used) as f64, "bytes", "Physical memory minus app, wired and compressed memory (cached files count as available)"));
            out.push(Metric::noted(names::MEMORY_USED, round1(used as f64 * 100.0 / total as f64), "percent", "App memory + wired + compressed, as Activity Monitor's \"Memory Used\""));
            out.push(Metric::value("memory.app_bytes", app as f64, "bytes"));
            out.push(Metric::value("memory.wired_bytes", wired as f64, "bytes"));
            out.push(Metric::noted("memory.compressed_bytes", compressed as f64, "bytes", "Physical memory held by the memory compressor"));
            out.push(Metric::noted("memory.cached_bytes", pages(v.external_page_count.saturating_add(v.purgeable_count)) as f64, "bytes", "File-backed and purgeable pages the system can reclaim at once"));
            out.push(Metric::value("memory.free_bytes", pages(v.free_count.saturating_sub(v.speculative_count)) as f64, "bytes"));
        }
        (None, _) => out.push(Metric::unavailable(names::MEMORY_USED, "percent", "hw.memsize could not be read")),
        (_, None) => out.push(Metric::unavailable(names::MEMORY_USED, "percent", "host_statistics64(HOST_VM_INFO64) failed")),
    }
    match sysctl_int(c"kern.memorystatus_vm_pressure_level") {
        Some(l) if pressure_name(l).is_some() => {
            let note = format!("Kernel memory pressure level: 1 normal, 2 warning, 4 critical (now {})", pressure_name(l).unwrap_or("unknown"));
            out.push(Metric::noted("memory.pressure_level", l as f64, "level", &note));
        }
        Some(l) => out.push(Metric::unavailable("memory.pressure_level", "level", format!("unrecognized kernel memory pressure level {l}"))),
        None => out.push(Metric::unavailable("memory.pressure_level", "level", "kern.memorystatus_vm_pressure_level could not be read")),
    }
    match swap_usage() {
        Some(s) => {
            out.push(Metric::noted("memory.swap_total_bytes", s.xsu_total as f64, "bytes", "Swap files grow and shrink on demand; 0 means none is in use right now"));
            out.push(Metric::value("memory.swap_used_bytes", s.xsu_used as f64, "bytes"));
        }
        None => {
            out.push(Metric::unavailable("memory.swap_total_bytes", "bytes", "vm.swapusage could not be read"));
            out.push(Metric::unavailable("memory.swap_used_bytes", "bytes", "vm.swapusage could not be read"));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Volumes

/// The volume user data lives on (the system volume itself is a sealed snapshot sharing the same APFS container).
fn data_volume() -> &'static CStr {
    if std::path::Path::new("/System/Volumes/Data").is_dir() { c"/System/Volumes/Data" } else { c"/" }
}

/// (total, available to this user) bytes from `statfs`.
fn statfs(path: &CStr) -> Option<(u64, u64)> {
    // SAFETY: zeroed POD out-parameter.
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path and a valid out-pointer.
    if unsafe { libc::statfs(path.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let bs = u64::from(s.f_bsize);
    let total = s.f_blocks.saturating_mul(bs);
    (total > 0).then(|| (total, s.f_bavail.saturating_mul(bs)))
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// `Boolean CFURLCopyResourcePropertyForKey(CFURLRef url, CFStringRef key, void *propertyValueTypeRefPtr, CFErrorRef *error);`
    fn CFURLCopyResourcePropertyForKey(url: CFTypeRef, key: CFTypeRef, value: *mut CFTypeRef, error: *mut CFTypeRef) -> u8;
    /// `const CFStringRef kCFURLVolumeAvailableCapacityForImportantUsageKey;` (macOS 10.13+)
    static kCFURLVolumeAvailableCapacityForImportantUsageKey: CFTypeRef;
}

/// Bytes available for "important" use: free space plus what macOS can purge (caches, local snapshots, evictable
/// cloud files).
fn important_capacity(path: &CStr) -> Option<u64> {
    let url = cf::file_url(path.to_bytes(), true)?;
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: live CFURL, the framework's constant key, valid out-pointer; no error object requested.
    let ok = unsafe { CFURLCopyResourcePropertyForKey(url.as_ptr(), kCFURLVolumeAvailableCapacityForImportantUsageKey, &mut value, std::ptr::null_mut()) };
    // SAFETY: Copy rule: `value` is +1 or NULL.
    let value = unsafe { CfRef::from_create(value) }?;
    if ok == 0 {
        return None;
    }
    // SAFETY: live CF object.
    match unsafe { cf::to_plist(value.as_ptr()) } {
        Ok(Plist::Integer(n)) => u64::try_from(n).ok(),
        _ => None,
    }
}

fn volume_metrics(out: &mut Vec<Metric>) {
    let vol = data_volume();
    let Some((total, avail)) = statfs(vol) else {
        out.push(Metric::unavailable(names::SYSTEM_FREE_PERCENT, "percent", "statfs on the data volume failed"));
        out.push(Metric::unavailable(names::SYSTEM_FREE_BYTES, "bytes", "statfs on the data volume failed"));
        return;
    };
    let note = "Free space on the system's APFS container (statfs), without purgeable space";
    out.push(Metric::value("disk.system.total_bytes", total as f64, "bytes"));
    out.push(Metric::noted(names::SYSTEM_FREE_BYTES, avail as f64, "bytes", note));
    out.push(Metric::noted(names::SYSTEM_FREE_PERCENT, round1(avail as f64 * 100.0 / total as f64), "percent", note));
    match important_capacity(vol) {
        Some(important) => {
            out.push(Metric::noted("disk.system.available_for_important_usage_bytes", important as f64, "bytes", "Free plus purgeable space, as Finder's \"Available\""));
            out.push(Metric::noted(
                "disk.system.purgeable_bytes",
                important.saturating_sub(avail) as f64,
                "bytes",
                "Space macOS can free on demand (caches, local snapshots, evictable cloud files); estimated by the system",
            ));
        }
        None => out.push(Metric::unavailable("disk.system.purgeable_bytes", "bytes", "the volume does not report its capacity for important usage")),
    }
    out.push(Metric::unavailable(
        names::DISK_FAILURE_PREDICTED,
        "boolean",
        "The drive's SMART/NVMe health log is only reachable through a storage-family user client; CleanManager does not open the device",
    ));
}

// ---------------------------------------------------------------------------------------------
// Disk I/O (IOBlockStorageDriver statistics)

/// Cumulative counters of one block storage driver.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DiskCounters {
    pub read_bytes: u64,
    pub write_bytes: u64,
    /// Time spent in requests, nanoseconds.
    pub read_ns: u64,
    pub write_ns: u64,
}

fn stat_u64(stats: &Plist, key: &str) -> Option<u64> {
    match stats.get(key)? {
        Plist::Integer(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

/// Counters from one driver's `Statistics` dictionary (`IOBlockStorageDriver.h` key names).
pub(crate) fn disk_counters_from(stats: &Plist) -> Option<DiskCounters> {
    Some(DiskCounters {
        read_bytes: stat_u64(stats, "Bytes (Read)")?,
        write_bytes: stat_u64(stats, "Bytes (Write)")?,
        read_ns: stat_u64(stats, "Total Time (Read)").unwrap_or(0),
        write_ns: stat_u64(stats, "Total Time (Write)").unwrap_or(0),
    })
}

/// Every block storage driver's counters by registry entry ID; `None` when no driver publishes statistics.
fn disk_counters() -> Option<BTreeMap<u64, DiskCounters>> {
    let mut out = BTreeMap::new();
    for s in Service::all_matching(c"IOBlockStorageDriver") {
        if let (Some(id), Some(Ok(stats))) = (s.registry_id(), s.property("Statistics"))
            && let Some(c) = disk_counters_from(&stats)
        {
            out.insert(id, c);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// (read B/s, write B/s, busy %) over `secs` for drivers present in both readings. A driver whose counters went
/// backwards (re-attached) has no rate for this interval. Busy is request time over the interval, capped at 100.
pub(crate) fn disk_rates(prev: &BTreeMap<u64, DiskCounters>, now: &BTreeMap<u64, DiskCounters>, secs: f64) -> Option<(f64, f64, f64)> {
    if secs <= 0.0 {
        return None;
    }
    let (mut r, mut w, mut busy_ns, mut seen) = (0u64, 0u64, 0u64, 0usize);
    for (id, b) in now {
        let Some(a) = prev.get(id) else { continue };
        let (Some(dr), Some(dw)) = (b.read_bytes.checked_sub(a.read_bytes), b.write_bytes.checked_sub(a.write_bytes)) else { continue };
        r += dr;
        w += dw;
        busy_ns += b.read_ns.saturating_sub(a.read_ns) + b.write_ns.saturating_sub(a.write_ns);
        seen += 1;
    }
    (seen > 0).then(|| (r as f64 / secs, w as f64 / secs, round1((busy_ns as f64 / (secs * 1e9) * 100.0).min(100.0))))
}

// ---------------------------------------------------------------------------------------------
// Network (NET_RT_IFLIST2)

/// `RTM_IFINFO2` (`<net/route.h>`).
const RTM_IFINFO2: u8 = 0x12;
/// `RTA_IFP`: a `sockaddr_dl` with the interface name follows the header.
const RTA_IFP: i32 = 0x10;
const IF_MSGHDR2_SIZE: usize = size_of::<libc::if_msghdr2>();
const _: () = assert!(IF_MSGHDR2_SIZE == 160);

/// One interface from the routing socket's interface list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IfCounters {
    pub index: u16,
    pub name: String,
    /// IANA ifType (`if_data64.ifi_type`).
    pub if_type: u8,
    /// `IFF_*` flags.
    pub flags: i32,
    pub rx: u64,
    pub tx: u64,
}

/// Parse a `NET_RT_IFLIST2` buffer: every `RTM_IFINFO2` message, its 64-bit counters and the interface name from the
/// `sockaddr_dl` that follows the header. Truncated or malformed messages end the walk; nothing is read out of bounds.
pub(crate) fn parse_iflist2(buf: &[u8]) -> Vec<IfCounters> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 4 <= buf.len() {
        let len = usize::from(u16::from_ne_bytes([buf[off], buf[off + 1]]));
        if len < 4 || off + len > buf.len() {
            break;
        }
        let msg = &buf[off..off + len];
        off += len;
        if msg[3] != RTM_IFINFO2 || len < IF_MSGHDR2_SIZE {
            continue;
        }
        // SAFETY: `msg` holds at least size_of::<if_msghdr2>() bytes; an unaligned read of a plain-old-data struct.
        let h: libc::if_msghdr2 = unsafe { std::ptr::read_unaligned(msg.as_ptr().cast()) };
        let data = h.ifm_data;
        let (addrs, flags, index) = (h.ifm_addrs, h.ifm_flags, h.ifm_index);
        let name = if addrs & RTA_IFP != 0 { sdl_name(&msg[IF_MSGHDR2_SIZE..]) } else { None };
        let name = name.or_else(|| index_name(index)).unwrap_or_else(|| format!("if{index}"));
        out.push(IfCounters { index, name, if_type: data.ifi_type, flags, rx: data.ifi_ibytes, tx: data.ifi_obytes });
    }
    out
}

/// Name from a `struct sockaddr_dl` (`sdl_len`, `sdl_family`, `sdl_index`, `sdl_type`, `sdl_nlen`, `sdl_alen`,
/// `sdl_slen`, then `sdl_data` starting with the name).
fn sdl_name(sa: &[u8]) -> Option<String> {
    if sa.len() < 8 || sa[1] != libc::AF_LINK as u8 {
        return None;
    }
    let (sa_len, nlen) = (usize::from(sa[0]), usize::from(sa[5]));
    let end = 8 + nlen;
    if nlen == 0 || end > sa.len() || end > sa_len.max(8) {
        return None;
    }
    std::str::from_utf8(&sa[8..end]).ok().map(str::to_owned)
}

fn index_name(index: u16) -> Option<String> {
    let mut buf = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: buffer of IF_NAMESIZE bytes as the call requires.
    let p = unsafe { libc::if_indextoname(u32::from(index), buf.as_mut_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: on success the buffer holds a NUL-terminated name.
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().ok().map(str::to_owned)
}

/// The kernel's interface list (`sysctl {CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0}`), read-only.
fn read_iflist2() -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    for _ in 0..4 {
        let mut len = 0usize;
        // SAFETY: size query with a NULL buffer.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        // Room for interfaces that appear between the two calls.
        len += len / 8 + 1024;
        let mut buf = vec![0u8; len];
        // SAFETY: `buf` holds `len` writable bytes.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0 {
            buf.truncate(len);
            return Some(buf);
        }
        if super::sys::errno() != libc::ENOMEM {
            return None;
        }
    }
    None
}

/// Interface kind from the name family, flags and type. macOS reports most virtual adapters as Ethernet, so the
/// driver's naming (one family per driver) is the hardware signal; `en*` Ethernet/Wi-Fi ports are the only
/// interfaces taken as physical.
pub(crate) fn interface_facts(name: &str, ifi_type: u8, flags: i32) -> InterfaceFacts {
    let family = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let t = u32::from(ifi_type);
    let facts = |if_type: u32, tunnel_type: u32, hardware: Option<bool>| InterfaceFacts { if_type, tunnel_type, hardware };
    if flags & libc::IFF_LOOPBACK != 0 || family == "lo" {
        return facts(if_type::SOFTWARE_LOOPBACK, 0, Some(false));
    }
    // Tunnels: VPN (utun, ipsec), PPP dial-up/VPN, IPv6 transition (gif, stf); point-to-point links likewise.
    if matches!(family, "utun" | "ipsec" | "ppp" | "gif" | "stf") || flags & libc::IFF_POINTOPOINT != 0 {
        return facts(if t == 0 { if_type::TUNNEL } else { t }, 1, Some(false));
    }
    // Virtual: bridges (Thunderbolt bridge, VM NAT), VM adapters, Apple's peer-to-peer and companion links, USB capture.
    if matches!(family, "bridge" | "vmnet" | "vmenet" | "vnic" | "vboxnet" | "feth" | "tap" | "anpi" | "awdl" | "llw" | "ap" | "XHC") {
        return facts(t, 0, Some(false));
    }
    if family == "en" && matches!(t, 6 | 71) {
        return facts(t, 0, Some(true));
    }
    facts(t, 0, None)
}

/// Bytes between two readings of a 64-bit counter. A smaller reading is a wrap only when the wrapped difference is
/// plausible (less than half the range); otherwise the counter was reset and there is no rate for this interval.
pub(crate) fn counter_delta(old: u64, new: u64) -> Option<u64> {
    let d = new.wrapping_sub(old);
    (d <= u64::MAX / 2).then_some(d)
}

/// (name, rx, tx) by interface index.
type IfOctets = BTreeMap<u16, (String, u64, u64)>;

fn if_octets(rows: &[IfCounters]) -> IfOctets {
    rows.iter().map(|r| (r.index, (r.name.clone(), r.rx, r.tx))).collect()
}

/// Rates over `secs` for up interfaces seen in both readings under the same name.
pub(crate) fn interface_rates(prev: &IfOctets, rows: &[IfCounters], secs: f64) -> Vec<InterfaceRate> {
    if secs <= 0.0 {
        return Vec::new();
    }
    rows.iter()
        .filter(|r| r.flags & libc::IFF_UP != 0)
        .filter_map(|r| {
            let (name, rx0, tx0) = prev.get(&r.index)?;
            if *name != r.name {
                return None;
            }
            let (rx, tx) = (counter_delta(*rx0, r.rx)?, counter_delta(*tx0, r.tx)?);
            Some(InterfaceRate {
                name: r.name.clone(),
                class: classify(&interface_facts(&r.name, r.if_type, r.flags)),
                received_bytes_per_sec: rx as f64 / secs,
                sent_bytes_per_sec: tx as f64 / secs,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// GPU, battery

fn plist_f64(p: Option<&Plist>) -> Option<f64> {
    match p? {
        Plist::Integer(n) => Some(*n as f64),
        Plist::Real(r) => Some(*r),
        _ => None,
    }
}

fn gpu_metrics(out: &mut Vec<Metric>) {
    let mut busiest: Option<f64> = None;
    let mut accelerators = 0usize;
    for s in Service::all_matching(c"IOAccelerator") {
        accelerators += 1;
        let Some(Ok(stats)) = s.property("PerformanceStatistics") else { continue };
        let u = plist_f64(stats.get("Device Utilization %")).or_else(|| plist_f64(stats.get("GPU Activity(%)")));
        if let Some(u) = u.filter(|u| (0.0..=100.0).contains(u)) {
            busiest = Some(busiest.map_or(u, |b: f64| b.max(u)));
        }
    }
    match busiest {
        Some(u) => out.push(Metric::noted("gpu.utilization_percent", u, "percent", "Busiest GPU's device utilization from its IOAccelerator performance statistics")),
        None if accelerators == 0 => out.push(Metric::unavailable("gpu.utilization_percent", "percent", "No IOAccelerator service is registered (no GPU driver, as in a virtual machine)")),
        None => out.push(Metric::unavailable("gpu.utilization_percent", "percent", "The GPU driver publishes no utilization in its IOAccelerator performance statistics")),
    }
    out.push(Metric::unavailable("gpu.temperature_celsius", "celsius", SENSOR_REASON));
}

fn int_of(p: &Plist, key: &str) -> Option<i64> {
    match p.get(key)? {
        Plist::Integer(n) => Some(*n),
        _ => None,
    }
}

/// Battery health (full-charge over design capacity, percent) from the `AppleSmartBattery` registry entry.
/// Apple silicon reports `MaxCapacity` as a percentage and the mAh value as `AppleRawMaxCapacity`.
pub(crate) fn battery_health(design: Option<i64>, raw_max: Option<i64>, max: Option<i64>) -> Option<f64> {
    let design = design.filter(|&d| d > 0)?;
    let full = raw_max.filter(|&m| m > 0).or(max.filter(|&m| m > 100))?;
    Some(round1(full as f64 * 100.0 / design as f64))
}

fn battery_metrics(out: &mut Vec<Metric>) {
    let Some((providing, sources)) = iokit::power_sources() else {
        out.push(Metric::unavailable(names::BATTERY_ON_AC, "boolean", "IOPSCopyPowerSourcesInfo returned no power-source snapshot"));
        for (n, u) in [(names::BATTERY_PERCENT, "percent"), (names::BATTERY_HEALTH, "percent"), ("battery.cycle_count", "count")] {
            out.push(Metric::unavailable(n, u, "the power-source snapshot is unavailable"));
        }
        return;
    };
    match providing.as_deref() {
        Some("AC Power") => out.push(Metric::value(names::BATTERY_ON_AC, 1.0, "boolean")),
        Some("Battery Power") => out.push(Metric::value(names::BATTERY_ON_AC, 0.0, "boolean")),
        Some(other) => out.push(Metric::unavailable(names::BATTERY_ON_AC, "boolean", format!("power comes from {other}"))),
        None => out.push(Metric::unavailable(names::BATTERY_ON_AC, "boolean", "the power-source snapshot names no providing source")),
    }
    let battery = sources.iter().find(|s| s.get("Type").and_then(Plist::as_str) == Some("InternalBattery"));
    let Some(b) = battery else {
        for (n, u) in [(names::BATTERY_PERCENT, "percent"), (names::BATTERY_HEALTH, "percent"), ("battery.cycle_count", "count")] {
            out.push(Metric::unavailable(n, u, "no internal battery"));
        }
        return;
    };
    match (int_of(b, "Current Capacity"), int_of(b, "Max Capacity")) {
        (Some(c), Some(m)) if m > 0 && c >= 0 => out.push(Metric::value(names::BATTERY_PERCENT, round1((c as f64 * 100.0 / m as f64).min(100.0)), "percent")),
        _ => out.push(Metric::unavailable(names::BATTERY_PERCENT, "percent", "the battery reports no current and maximum capacity")),
    }
    let smart = Service::matching(c"AppleSmartBattery");
    let prop = |k: &str| smart.as_ref().and_then(|s| s.property(k)).and_then(Result::ok).and_then(|p| if let Plist::Integer(n) = p { Some(n) } else { None });
    match battery_health(prop("DesignCapacity"), prop("AppleRawMaxCapacity"), prop("MaxCapacity")) {
        Some(h) => out.push(Metric::noted(names::BATTERY_HEALTH, h, "percent", "Full-charge capacity over design capacity (AppleSmartBattery)")),
        None => out.push(Metric::unavailable(names::BATTERY_HEALTH, "percent", "the battery does not report its design and full-charge capacity")),
    }
    match prop("CycleCount").filter(|&c| c >= 0) {
        Some(c) => out.push(Metric::value("battery.cycle_count", c as f64, "count")),
        None => out.push(Metric::unavailable("battery.cycle_count", "count", "the battery does not report a cycle count")),
    }
}

// ---------------------------------------------------------------------------------------------
// Processes

/// Pids of zombie processes (`SZOMB`: exited, not yet collected by their parent).
pub fn zombies() -> Result<Vec<i32>, String> {
    let snap = procs::snapshot().map_err(|e| format!("the process list could not be read: {}", super::sys::describe(e)))?;
    Ok(snap.procs.iter().filter(|p| p.zombie).map(|p| p.pid).collect())
}

/// Per-process CPU readings between samples: (pid, start) → CPU nanoseconds, with the name for the top process.
type CpuTimes = HashMap<(i32, Option<(u64, u64)>), (u64, String)>;

fn process_metrics(prev: Option<&(Instant, CpuTimes)>, ncpu: usize, out: &mut Vec<Metric>) -> (Option<(Instant, CpuTimes)>, Option<TopProcess>) {
    let snap = match procs::snapshot() {
        Ok(s) => s,
        Err(e) => {
            let why = format!("the process list could not be read: {}", super::sys::describe(e));
            out.push(Metric::unavailable("process.count", "count", why.clone()));
            out.push(Metric::unavailable("process.zombie_count", "count", why));
            return (None, None);
        }
    };
    let now = Instant::now();
    out.push(Metric::value("process.count", snap.procs.len() as f64, "count"));
    out.push(Metric::noted(
        "process.zombie_count",
        snap.procs.iter().filter(|p| p.zombie).count() as f64,
        "count",
        "Processes that exited but whose parent has not collected them yet (SZOMB); they use no CPU or memory",
    ));
    let times: CpuTimes = snap
        .procs
        .iter()
        .filter(|p| !p.zombie && p.start.is_some())
        .filter_map(|p| processes::usage(p.pid).ok().map(|u| ((p.pid, p.start), (u.cpu_ns, p.name.clone()))))
        .collect();
    let top = prev.and_then(|(then, before)| {
        let secs = now.duration_since(*then).as_secs_f64();
        if secs <= 0.0 {
            return None;
        }
        times
            .iter()
            .filter_map(|(k, (ns, name))| before.get(k).map(|(ns0, _)| (name, ns.saturating_sub(*ns0))))
            .max_by_key(|(_, d)| *d)
            .map(|(name, d)| TopProcess { name: name.clone(), cpu_percent: round1((d as f64 / 1e9 / secs / ncpu.max(1) as f64 * 100.0).min(100.0)) })
    });
    (Some((now, times)), top)
}

// ---------------------------------------------------------------------------------------------
// Sampler

/// Holds the previous readings between samples so rates cover the interval since the previous sample.
pub struct Sampler {
    host: Host,
    logical_cpus: usize,
    primed: Instant,
    cpu_prev: Option<Ticks>,
    disk_prev: Option<(Instant, BTreeMap<u64, DiskCounters>)>,
    net_prev: Option<(Instant, IfOctets)>,
    proc_prev: Option<(Instant, CpuTimes)>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    /// Take the first readings of every rate metric.
    pub fn new() -> Sampler {
        let host = Host::new();
        let cpu_prev = cpu_ticks(&host);
        let disk_prev = disk_counters().map(|c| (Instant::now(), c));
        let net_prev = read_iflist2().map(|b| (Instant::now(), if_octets(&parse_iflist2(&b))));
        let logical_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let (proc_prev, _) = process_metrics(None, logical_cpus, &mut Vec::new());
        Sampler { host, logical_cpus, primed: Instant::now(), cpu_prev, disk_prev, net_prev, proc_prev }
    }

    /// Take one snapshot. Rates cover the time since the previous call (or since `new`); if that is shorter than
    /// [`FIRST_WINDOW`] the call waits for the rest of the window first.
    pub fn sample(&mut self) -> StatusSnapshot {
        let elapsed = self.primed.elapsed();
        if elapsed < FIRST_WINDOW {
            std::thread::sleep(FIRST_WINDOW - elapsed);
        }
        self.primed = Instant::now();
        let sampled_at = cm_core::time::now_utc();
        let mut metrics = Vec::new();
        self.cpu_metrics(&mut metrics);
        memory_metrics(&self.host, &mut metrics);
        volume_metrics(&mut metrics);
        self.disk_metrics(&mut metrics);
        let network = self.network_metrics(&mut metrics);
        gpu_metrics(&mut metrics);
        battery_metrics(&mut metrics);
        metrics.push(Metric::unavailable(names::THERMAL_MAX, "celsius", SENSOR_REASON));
        metrics.push(Metric::unavailable("fan.rpm", "rpm", SENSOR_REASON));
        let (proc_prev, top) = process_metrics(self.proc_prev.as_ref(), self.logical_cpus, &mut metrics);
        self.proc_prev = proc_prev;
        StatusSnapshot::new(sampled_at, metrics, top.as_ref()).with_network(network)
    }

    fn cpu_metrics(&mut self, out: &mut Vec<Metric>) {
        let now = cpu_ticks(&self.host);
        out.push(Metric::value("cpu.logical_count", self.logical_cpus as f64, "count"));
        match (self.cpu_prev.as_deref(), now.as_deref()) {
            (Some(prev), Some(cur)) if prev.len() == cur.len() => {
                match busy_percent(prev, cur) {
                    Some(v) => out.push(Metric::value(names::CPU_TOTAL, v, "percent")),
                    None => out.push(Metric::unavailable(names::CPU_TOTAL, "percent", "no processor ticks elapsed between the two readings")),
                }
                for (i, (a, b)) in prev.iter().zip(cur).enumerate() {
                    if let Some(v) = busy_percent(std::slice::from_ref(a), std::slice::from_ref(b)) {
                        out.push(Metric::value(format!("cpu.core.{i}.percent"), v, "percent"));
                    }
                }
            }
            (Some(_), Some(_)) => out.push(Metric::unavailable(names::CPU_TOTAL, "percent", "the number of processors changed between readings")),
            _ => out.push(Metric::unavailable(names::CPU_TOTAL, "percent", "host_processor_info(PROCESSOR_CPU_LOAD_INFO) failed")),
        }
        if now.is_some() {
            self.cpu_prev = now;
        }
        let mut load = [0f64; 3];
        // SAFETY: buffer of 3 doubles as requested.
        if unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } == 3 {
            for (n, v) in ["1m", "5m", "15m"].iter().zip(load) {
                out.push(Metric::noted(format!("cpu.load_average_{n}"), (v * 100.0).round() / 100.0, "count", "Average number of runnable threads (Unix load average)"));
            }
        } else {
            out.push(Metric::unavailable("cpu.load_average_1m", "count", "getloadavg failed"));
        }
    }

    fn disk_metrics(&mut self, out: &mut Vec<Metric>) {
        let now = disk_counters().map(|c| (Instant::now(), c));
        let rates = match (&self.disk_prev, &now) {
            (Some((t0, a)), Some((t1, b))) => disk_rates(a, b, t1.duration_since(*t0).as_secs_f64()),
            _ => None,
        };
        let why = if now.is_none() { "No IOBlockStorageDriver publishes I/O statistics" } else { "No disk was present in both readings" };
        match rates {
            Some((r, w, busy)) => {
                out.push(Metric::value("disk.read_bytes_per_sec", r, "bytes_per_second"));
                out.push(Metric::value("disk.write_bytes_per_sec", w, "bytes_per_second"));
                out.push(Metric::noted(names::DISK_BUSY, busy, "percent", "Time block storage drivers spent on reads and writes over the interval, summed over disks and capped at 100 (concurrent requests overlap)"));
            }
            None => {
                for (n, u) in [("disk.read_bytes_per_sec", "bytes_per_second"), ("disk.write_bytes_per_sec", "bytes_per_second"), (names::DISK_BUSY, "percent")] {
                    out.push(Metric::unavailable(n, u, why));
                }
            }
        }
        if now.is_some() {
            self.disk_prev = now;
        }
    }

    fn network_metrics(&mut self, out: &mut Vec<Metric>) -> Option<NetworkTotals> {
        let rows = read_iflist2().map(|b| parse_iflist2(&b));
        let now = Instant::now();
        let totals = match (&rows, self.net_prev.take()) {
            (Some(rows), Some((then, prev))) => Some(NetworkTotals::from_rates(Classification::InterfaceType, interface_rates(&prev, rows, now.duration_since(then).as_secs_f64()))),
            _ => None,
        };
        if let Some(rows) = &rows {
            self.net_prev = Some((now, if_octets(rows)));
        }
        match &totals {
            Some(t) => {
                out.push(Metric::noted("network.received_bytes_per_sec", t.received_bytes_per_sec, "bytes_per_second", &t.note));
                out.push(Metric::noted("network.sent_bytes_per_sec", t.sent_bytes_per_sec, "bytes_per_second", &t.note));
            }
            None => {
                let why = "the interface list (sysctl NET_RT_IFLIST2) could not be read twice";
                out.push(Metric::unavailable("network.received_bytes_per_sec", "bytes_per_second", why));
                out.push(Metric::unavailable("network.sent_bytes_per_sec", "bytes_per_second", why));
            }
        }
        totals
    }
}

/// One snapshot with the fixed first window (about one second).
pub fn snapshot() -> StatusSnapshot {
    Sampler::new().sample()
}

/// One snapshot whose rate metrics (CPU, disk, network, busiest process) cover `window` (at least [`FIRST_WINDOW`]).
/// For callers that need a single diagnosis (for example the maintenance diagnosis): no state is kept.
pub fn sample(window: Duration) -> StatusSnapshot {
    let mut s = Sampler::new();
    std::thread::sleep(window.saturating_sub(FIRST_WINDOW));
    s.sample()
}

#[cfg(test)]
mod tests;
