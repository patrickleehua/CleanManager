//! Windows status sampler (parity 12.1/12.2, STS-01..03).
//!
//! Interfaces: PDH performance counters added by their English names (`PdhAddEnglishCounterW`, so localized
//! systems work) for CPU, per-core load, processor queue, GPU engines and memory, physical-disk throughput and
//! idle time, network interfaces, the paging file, ACPI thermal zones and per-process CPU; `GlobalMemoryStatusEx`;
//! `GetDiskFreeSpaceExW` per fixed volume; `GetSystemPowerStatus` and the battery class driver
//! (`IOCTL_BATTERY_QUERY_INFORMATION`, opened read-only) for charge, health and cycles; and
//! `IOCTL_STORAGE_PREDICT_FAILURE` on the system disk, opened with no data access.
//!
//! Network (STS-07): `GetIfTable2` gives each interface's type, tunnel type, `HardwareInterface` flag and octet
//! counters; rates are the counter differences over the sample interval. NDIS filter layers (rows flagged
//! `FilterInterface`, which repeat their adapter's counters) and interfaces that are not up are skipped. If the table
//! cannot be read, the PDH `Network Interface` sum is reported instead, marked as possibly double counting.
//!
//! Everything is read-only. A counter that does not exist, a device that does not answer or a value the hardware
//! does not report becomes an `unavailable` (or `permission_required`) metric with a reason, never 0.

use std::collections::BTreeMap;
use std::time::Duration;

use cm_core::status::network::{Classification, InterfaceClass, InterfaceFacts, InterfaceRate, NetworkTotals, classify};
use cm_core::status::{Metric, StatusSnapshot, TopProcess, names};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};

/// `PDH_FMT_NOCAP100` (pdh.h): do not cap percentages at 100 (multi-core per-process values).
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A PDH query; closed on drop.
struct Query(PDH_HQUERY);

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: handle from a successful PdhOpenQueryW, closed once.
        unsafe { PdhCloseQuery(self.0) };
    }
}

impl Query {
    fn open() -> Option<Query> {
        let mut q: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: live (real-time) query, valid out-pointer.
        (unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) } == 0).then_some(Query(q))
    }

    fn add(&self, path: &str) -> Option<PDH_HCOUNTER> {
        let w = wide(path);
        let mut c: PDH_HCOUNTER = std::ptr::null_mut();
        // SAFETY: NUL-terminated English counter path, valid out-pointer.
        (unsafe { PdhAddEnglishCounterW(self.0, w.as_ptr(), 0, &mut c) } == 0).then_some(c)
    }

    fn collect(&self) -> bool {
        // SAFETY: valid query handle.
        unsafe { PdhCollectQueryData(self.0) == 0 }
    }
}

fn valid(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

fn single(c: Option<PDH_HCOUNTER>) -> Option<f64> {
    let c = c?;
    let mut v = PDH_FMT_COUNTERVALUE { CStatus: 0, Anonymous: unsafe { std::mem::zeroed() } };
    // SAFETY: counter from this query; output struct is local.
    let rc = unsafe { PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, std::ptr::null_mut(), &mut v) };
    // SAFETY: PDH_FMT_DOUBLE selects the doubleValue member.
    (rc == 0 && valid(v.CStatus)).then_some(unsafe { v.Anonymous.doubleValue }).filter(|x| x.is_finite())
}

/// Wildcard counter values by instance name (instances with invalid data are dropped).
fn array(c: Option<PDH_HCOUNTER>) -> Option<Vec<(String, f64)>> {
    let c = c?;
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: size query with a null buffer.
    let rc = unsafe { PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, std::ptr::null_mut()) };
    if rc != PDH_MORE_DATA {
        return if rc == 0 { Some(Vec::new()) } else { None };
    }
    // u64 elements keep the item array 8-byte aligned; names are stored after the items in the same buffer.
    let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
    // SAFETY: buffer of at least `size` bytes, suitably aligned for PDH_FMT_COUNTERVALUE_ITEM_W.
    let rc = unsafe { PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, buf.as_mut_ptr().cast()) };
    if rc != 0 {
        return None;
    }
    let items = buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        // SAFETY: `count` items were written at the start of the buffer; names point into the same buffer.
        let item = unsafe { &*items.add(i) };
        if !valid(item.FmtValue.CStatus) {
            continue;
        }
        // SAFETY: szName is a NUL-terminated string inside `buf`.
        let name = unsafe {
            let len = (0..).take_while(|&k| *item.szName.add(k) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len))
        };
        // SAFETY: PDH_FMT_DOUBLE selects the doubleValue member.
        let v = unsafe { item.FmtValue.Anonymous.doubleValue };
        if v.is_finite() {
            out.push((name, v));
        }
    }
    Some(out)
}

struct Counters {
    cpu_total: Option<PDH_HCOUNTER>,
    cpu_cores: Option<PDH_HCOUNTER>,
    queue: Option<PDH_HCOUNTER>,
    gpu_engines: Option<PDH_HCOUNTER>,
    gpu_dedicated: Option<PDH_HCOUNTER>,
    disk_read: Option<PDH_HCOUNTER>,
    disk_write: Option<PDH_HCOUNTER>,
    disk_idle: Option<PDH_HCOUNTER>,
    net_rx: Option<PDH_HCOUNTER>,
    net_tx: Option<PDH_HCOUNTER>,
    pagefile: Option<PDH_HCOUNTER>,
    thermal: Option<PDH_HCOUNTER>,
    thermal_low: Option<PDH_HCOUNTER>,
    process_cpu: Option<PDH_HCOUNTER>,
}

/// Holds the counter query between samples so rates cover the interval since the previous sample.
pub struct Sampler {
    query: Option<Query>,
    counters: Counters,
    logical_cpus: usize,
    primed: std::time::Instant,
    /// Interface octet counters at the previous sample, by LUID.
    net_prev: Option<(std::time::Instant, Octets)>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed window for the first sample of rate counters (CPU, disk, network, GPU).
pub const FIRST_WINDOW: Duration = Duration::from_millis(1000);

impl Sampler {
    pub fn new() -> Sampler {
        let query = Query::open();
        let add = |p: &str| query.as_ref().and_then(|q| q.add(p));
        let counters = Counters {
            cpu_total: add(r"\Processor(_Total)\% Processor Time"),
            cpu_cores: add(r"\Processor(*)\% Processor Time"),
            queue: add(r"\System\Processor Queue Length"),
            gpu_engines: add(r"\GPU Engine(*)\Utilization Percentage"),
            gpu_dedicated: add(r"\GPU Adapter Memory(*)\Dedicated Usage"),
            disk_read: add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"),
            disk_write: add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"),
            disk_idle: add(r"\PhysicalDisk(_Total)\% Idle Time"),
            net_rx: add(r"\Network Interface(*)\Bytes Received/sec"),
            net_tx: add(r"\Network Interface(*)\Bytes Sent/sec"),
            pagefile: add(r"\Paging File(_Total)\% Usage"),
            thermal: add(r"\Thermal Zone Information(*)\High Precision Temperature"),
            thermal_low: add(r"\Thermal Zone Information(*)\Temperature"),
            process_cpu: add(r"\Process(*)\% Processor Time"),
        };
        let net_prev = interface_table().map(|rows| (std::time::Instant::now(), octets(&rows)));
        if let Some(q) = &query {
            q.collect();
        }
        let logical_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        Sampler { query, counters, logical_cpus, primed: std::time::Instant::now(), net_prev }
    }

    /// Take one snapshot. Rate counters cover the time since the previous call (or since `new`); if that is
    /// shorter than `FIRST_WINDOW` the call waits for the rest of the window first.
    pub fn sample(&mut self) -> StatusSnapshot {
        let elapsed = self.primed.elapsed();
        if elapsed < FIRST_WINDOW {
            std::thread::sleep(FIRST_WINDOW - elapsed);
        }
        let collected = self.query.as_ref().is_some_and(Query::collect);
        let rows = interface_table();
        self.primed = std::time::Instant::now();
        let sampled_at = cm_core::time::now_utc();
        let mut metrics = Vec::new();
        let mut top = None;
        if collected {
            self.counter_metrics(&mut metrics, &mut top);
        } else {
            metrics.push(Metric::unavailable(names::CPU_TOTAL, "percent", "performance counters could not be opened or collected"));
        }
        let network = self.network_metrics(rows, collected, &mut metrics);
        memory_metrics(&mut metrics);
        volume_metrics(&mut metrics);
        disk_health_metric(&mut metrics);
        battery_metrics(&mut metrics);
        metrics.push(Metric::unavailable(
            "fan.rpm",
            "rpm",
            "Windows has no documented user-mode fan-speed interface; vendor tools read the embedded controller directly",
        ));
        metrics.push(Metric::unavailable("gpu.temperature_celsius", "celsius", "No documented Windows interface reports GPU temperature without vendor software"));
        StatusSnapshot::new(sampled_at, metrics, top.as_ref()).with_network(network)
    }

    /// STS-07 totals: classified interface rates from the IP Helper table, else the unclassified PDH sum.
    fn network_metrics(&mut self, rows: Option<Vec<IfRow>>, collected: bool, out: &mut Vec<Metric>) -> Option<NetworkTotals> {
        let now = std::time::Instant::now();
        let classified = match (rows, self.net_prev.take()) {
            (Some(rows), prev) => {
                self.net_prev = Some((now, octets(&rows)));
                // Without an earlier reading (the table was unreadable before) there is nothing to difference yet.
                prev.map(|(then, prev)| NetworkTotals::from_rates(Classification::InterfaceType, interface_rates(&prev, &rows, now.duration_since(then).as_secs_f64())))
            }
            (None, _) => None,
        };
        let totals = classified.or_else(|| {
            let c = &self.counters;
            let (rx, tx) = (array(c.net_rx).filter(|_| collected)?, array(c.net_tx).filter(|_| collected)?);
            let txm: BTreeMap<String, f64> = tx.into_iter().collect();
            let rates = rx
                .into_iter()
                .map(|(name, r)| {
                    let sent = txm.get(&name).copied().unwrap_or(0.0);
                    InterfaceRate { name, class: InterfaceClass::Unknown, received_bytes_per_sec: r, sent_bytes_per_sec: sent }
                })
                .collect();
            Some(NetworkTotals::from_rates(Classification::Unavailable, rates))
        });
        match &totals {
            Some(t) => {
                out.push(Metric::noted("network.received_bytes_per_sec", t.received_bytes_per_sec, "bytes_per_second", &t.note));
                out.push(Metric::noted("network.sent_bytes_per_sec", t.sent_bytes_per_sec, "bytes_per_second", &t.note));
            }
            None => {
                let why = "neither the interface table nor the Network Interface performance counters could be read";
                out.push(Metric::unavailable("network.received_bytes_per_sec", "bytes_per_second", why));
                out.push(Metric::unavailable("network.sent_bytes_per_sec", "bytes_per_second", why));
            }
        }
        totals
    }

    fn counter_metrics(&self, out: &mut Vec<Metric>, top: &mut Option<TopProcess>) {
        let c = &self.counters;
        let missing = |name: &str, unit: &str, what: &str| Metric::unavailable(name, unit, format!("performance counter {what} is not available"));
        match single(c.cpu_total) {
            Some(v) => out.push(Metric::value(names::CPU_TOTAL, v.min(100.0), "percent")),
            None => out.push(missing(names::CPU_TOTAL, "percent", r"\Processor(_Total)\% Processor Time")),
        }
        out.push(Metric::value("cpu.logical_count", self.logical_cpus as f64, "count"));
        if let Some(cores) = array(c.cpu_cores) {
            let mut cores: Vec<(usize, f64)> = cores.into_iter().filter_map(|(n, v)| n.parse::<usize>().ok().map(|i| (i, v))).collect();
            cores.sort_by_key(|(i, _)| *i);
            for (i, v) in cores {
                out.push(Metric::value(format!("cpu.core.{i}.percent"), v.min(100.0), "percent"));
            }
        }
        match single(c.queue) {
            Some(v) => out.push(Metric::noted("cpu.queue_length", v, "count", "Threads ready but waiting for a processor; Windows has no Unix load average")),
            None => out.push(missing("cpu.queue_length", "count", r"\System\Processor Queue Length")),
        }
        match array(c.gpu_engines) {
            Some(engines) if !engines.is_empty() => out.push(Metric::noted("gpu.utilization_percent", gpu_utilization(&engines), "percent", "Busiest engine type across adapters, as Task Manager reports")),
            _ => out.push(Metric::unavailable("gpu.utilization_percent", "percent", "GPU engine counters are not published (driver without WDDM 2 performance data)")),
        }
        match array(c.gpu_dedicated) {
            Some(a) if !a.is_empty() => out.push(Metric::value("gpu.dedicated_memory_used_bytes", a.iter().map(|(_, v)| v).sum(), "bytes")),
            _ => out.push(missing("gpu.dedicated_memory_used_bytes", "bytes", r"\GPU Adapter Memory(*)\Dedicated Usage")),
        }
        match (single(c.disk_read), single(c.disk_write)) {
            (Some(r), Some(w)) => {
                out.push(Metric::value("disk.read_bytes_per_sec", r, "bytes_per_second"));
                out.push(Metric::value("disk.write_bytes_per_sec", w, "bytes_per_second"));
            }
            _ => {
                out.push(missing("disk.read_bytes_per_sec", "bytes_per_second", r"\PhysicalDisk(_Total)"));
                out.push(missing("disk.write_bytes_per_sec", "bytes_per_second", r"\PhysicalDisk(_Total)"));
            }
        }
        match single(c.disk_idle) {
            Some(idle) => out.push(Metric::noted(names::DISK_BUSY, (100.0 - idle.min(100.0)).max(0.0), "percent", "100 minus physical-disk idle time, averaged over disks")),
            None => out.push(missing(names::DISK_BUSY, "percent", r"\PhysicalDisk(_Total)\% Idle Time")),
        }
        match single(c.pagefile) {
            Some(v) => out.push(Metric::value("memory.pagefile_used_percent", v.min(100.0), "percent")),
            None => out.push(Metric::unavailable("memory.pagefile_used_percent", "percent", "no paging file is configured or the counter is not available")),
        }
        let zones: Option<Vec<(String, f64)>> = match array(c.thermal) {
            Some(z) if !z.is_empty() => Some(z.into_iter().map(|(n, v)| (n, v / 10.0 - 273.15)).collect()),
            _ => array(c.thermal_low).filter(|z| !z.is_empty()).map(|z| z.into_iter().map(|(n, v)| (n, v - 273.15)).collect()),
        };
        let zone_note = "ACPI thermal zone reported by the firmware; not necessarily a CPU core sensor";
        match zones.map(|z| z.into_iter().filter(|(_, c)| (-40.0..150.0).contains(c)).collect::<Vec<_>>()) {
            Some(z) if !z.is_empty() => {
                let max = z.iter().map(|(_, c)| *c).fold(f64::MIN, f64::max);
                out.push(Metric::noted(names::THERMAL_MAX, round1(max), "celsius", zone_note));
                for (n, c) in z {
                    out.push(Metric::noted(format!("thermal.zone.{}.celsius", n.trim_start_matches('\\')), round1(c), "celsius", zone_note));
                }
            }
            _ => out.push(Metric::unavailable(names::THERMAL_MAX, "celsius", "The firmware publishes no ACPI thermal zone with a plausible reading")),
        }
        if let Some(procs) = array(c.process_cpu) {
            *top = top_process(&procs, self.logical_cpus);
        }
    }
}

/// One interface from the IP Helper table (filter layers and interfaces that are not up already dropped).
pub(crate) struct IfRow {
    pub luid: u64,
    pub name: String,
    pub facts: InterfaceFacts,
    pub received: u64,
    pub sent: u64,
}

/// (received, sent) byte counters by interface LUID.
type Octets = BTreeMap<u64, (u64, u64)>;

fn octets(rows: &[IfRow]) -> Octets {
    rows.iter().map(|r| (r.luid, (r.received, r.sent))).collect()
}

/// Rates over `secs` for interfaces seen in both readings. An interface that is new since the previous reading or
/// whose counters went backwards (reset) has no rate for this interval and is left out.
pub(crate) fn interface_rates(prev: &Octets, rows: &[IfRow], secs: f64) -> Vec<InterfaceRate> {
    if secs <= 0.0 {
        return Vec::new();
    }
    rows.iter()
        .filter_map(|r| {
            let &(rx0, tx0) = prev.get(&r.luid)?;
            let (rx, tx) = (r.received.checked_sub(rx0)?, r.sent.checked_sub(tx0)?);
            Some(InterfaceRate { name: r.name.clone(), class: classify(&r.facts), received_bytes_per_sec: rx as f64 / secs, sent_bytes_per_sec: tx as f64 / secs })
        })
        .collect()
}

fn wide_field(w: &[u16]) -> String {
    String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
}

/// `GetIfTable2`, read-only. `None` when the table cannot be read.
fn interface_table() -> Option<Vec<IfRow>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2};
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    // MIB_IF_ROW2 InterfaceAndOperStatusFlags bitfield: bit 0 HardwareInterface, bit 1 FilterInterface.
    const HARDWARE: u8 = 0b01;
    const FILTER: u8 = 0b10;
    let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: valid out-pointer; the table is freed with FreeMibTable below.
    if unsafe { GetIfTable2(&mut t) } != 0 || t.is_null() {
        return None;
    }
    // SAFETY: GetIfTable2 returned a table of NumEntries rows starting at Table.
    let rows = unsafe { std::slice::from_raw_parts(std::ptr::addr_of!((*t).Table).cast::<MIB_IF_ROW2>(), (*t).NumEntries as usize) };
    let out = rows
        .iter()
        .filter(|r| r.InterfaceAndOperStatusFlags._bitfield & FILTER == 0 && r.OperStatus == IfOperStatusUp)
        .map(|r| {
            let alias = wide_field(&r.Alias);
            IfRow {
                // SAFETY: Value spans the whole 64-bit LUID union.
                luid: unsafe { r.InterfaceLuid.Value },
                name: if alias.is_empty() { wide_field(&r.Description) } else { alias },
                facts: InterfaceFacts { if_type: r.Type, tunnel_type: r.TunnelType as u32, hardware: Some(r.InterfaceAndOperStatusFlags._bitfield & HARDWARE != 0) },
                received: r.InOctets,
                sent: r.OutOctets,
            }
        })
        .collect();
    // SAFETY: table from GetIfTable2, freed once; `rows` is not used after this.
    unsafe { FreeMibTable(t.cast()) };
    Some(out)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Task Manager's GPU figure: per adapter and engine type, sum the per-process engine utilizations; report the
/// busiest (adapter, engine type), capped at 100.
pub(crate) fn gpu_utilization(engines: &[(String, f64)]) -> f64 {
    let mut groups: BTreeMap<(String, String), f64> = BTreeMap::new();
    for (name, v) in engines {
        let luid = name.split("_luid_").nth(1).and_then(|r| r.split("_phys_").next()).unwrap_or("").to_owned();
        let eng = name.split("_engtype_").nth(1).unwrap_or("").to_owned();
        *groups.entry((luid, eng)).or_default() += v;
    }
    groups.values().copied().fold(0.0, f64::max).min(100.0)
}

/// Busiest process as a share of the whole machine. `% Processor Time` per process is relative to one CPU.
pub(crate) fn top_process(procs: &[(String, f64)], logical_cpus: usize) -> Option<TopProcess> {
    let mut by_name: BTreeMap<&str, f64> = BTreeMap::new();
    for (n, v) in procs {
        let base = n.split('#').next().unwrap_or(n);
        if base == "_Total" || base == "Idle" {
            continue;
        }
        *by_name.entry(base).or_default() += v;
    }
    by_name
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, v)| TopProcess { name: n.to_owned(), cpu_percent: round1((v / logical_cpus.max(1) as f64).min(100.0)) })
}

fn memory_metrics(out: &mut Vec<Metric>) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..unsafe { std::mem::zeroed() } };
    // SAFETY: dwLength is set; output struct is local.
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 || m.ullTotalPhys == 0 {
        out.push(Metric::unavailable(names::MEMORY_USED, "percent", "GlobalMemoryStatusEx failed"));
        return;
    }
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
    out.push(Metric::value("memory.total_bytes", m.ullTotalPhys as f64, "bytes"));
    out.push(Metric::value("memory.available_bytes", m.ullAvailPhys as f64, "bytes"));
    out.push(Metric::value(names::MEMORY_USED, round1(used as f64 * 100.0 / m.ullTotalPhys as f64), "percent"));
    out.push(Metric::value("memory.commit_used_bytes", m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile) as f64, "bytes"));
    out.push(Metric::value("memory.commit_limit_bytes", m.ullTotalPageFile as f64, "bytes"));
    out.push(Metric::unavailable("memory.compressed_bytes", "bytes", "Windows does not publish the memory compression store size through a documented interface"));
}

fn volume_metrics(out: &mut Vec<Metric>) {
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).trim_end_matches('\\').to_ascii_uppercase();
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut system_seen = false;
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = wide(&format!(r"{letter}:\"));
        // SAFETY: NUL-terminated root path.
        let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
        // Fixed (3), removable (2) and RAM disks (6); network (4) and optical (5) volumes are not touched.
        if !matches!(kind, 2 | 3 | 6) {
            continue;
        }
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: NUL-terminated root, valid out-pointers.
        let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0 && total > 0;
        let prefix = format!("disk.volume.{letter}");
        if !ok {
            if kind == 3 {
                out.push(Metric::unavailable(format!("{prefix}.free_bytes"), "bytes", "the volume did not report its capacity"));
            }
            continue; // removable drive without media
        }
        out.push(Metric::value(format!("{prefix}.total_bytes"), total as f64, "bytes"));
        out.push(Metric::value(format!("{prefix}.free_bytes"), free as f64, "bytes"));
        if format!("{letter}:") == system {
            system_seen = true;
            out.push(Metric::value("disk.system.total_bytes", total as f64, "bytes"));
            out.push(Metric::value(names::SYSTEM_FREE_BYTES, free as f64, "bytes"));
            out.push(Metric::value(names::SYSTEM_FREE_PERCENT, round1(free as f64 * 100.0 / total as f64), "percent"));
        }
    }
    if !system_seen {
        out.push(Metric::unavailable(names::SYSTEM_FREE_PERCENT, "percent", "the system volume did not report its capacity"));
    }
}

/// Open a device path with no data access (attribute/IOCTL-only); `None` on failure with the Win32 error.
fn open_device(path: &str, access: u32) -> Result<isize, u32> {
    let w = wide(path);
    // SAFETY: NUL-terminated path; no data access unless `access` asks for read.
    let h = unsafe { CreateFileW(w.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: reads the calling thread's last error.
        Err(unsafe { windows_sys::Win32::Foundation::GetLastError() })
    } else {
        Ok(h as isize)
    }
}

fn close(h: isize) {
    // SAFETY: handle from a successful CreateFileW, closed once.
    unsafe { CloseHandle(h as _) };
}

fn ioctl(h: isize, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, u32> {
    let mut got = 0u32;
    // SAFETY: buffers are sized by their slices; synchronous handle.
    let ok = unsafe {
        DeviceIoControl(h as _, code, input.as_ptr().cast(), input.len() as u32, output.as_mut_ptr().cast(), output.len() as u32, &mut got, std::ptr::null_mut())
    };
    if ok == 0 { Err(unsafe { windows_sys::Win32::Foundation::GetLastError() }) } else { Ok(got as usize) }
}

fn disk_health_metric(out: &mut Vec<Metric>) {
    use windows_sys::Win32::Storage::FileSystem::IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS;
    use windows_sys::Win32::System::Ioctl::IOCTL_STORAGE_PREDICT_FAILURE;
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let name = names::DISK_FAILURE_PREDICTED;
    let vol = match open_device(&format!(r"\\.\{}", system.trim_end_matches('\\')), 0) {
        Ok(h) => h,
        Err(e) => return out.push(Metric::unavailable(name, "boolean", format!("system volume could not be opened (error {e})"))),
    };
    let mut ext = [0u8; 256];
    let disk = ioctl(vol, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut ext);
    close(vol);
    // VOLUME_DISK_EXTENTS: u32 count, padding, then DISK_EXTENT { u32 DiskNumber, i64 offset, i64 length }.
    let disk_number = match disk {
        Ok(n) if n >= 16 && u32::from_le_bytes([ext[0], ext[1], ext[2], ext[3]]) == 1 => u32::from_le_bytes([ext[8], ext[9], ext[10], ext[11]]),
        Ok(_) => return out.push(Metric::unavailable(name, "boolean", "the system volume spans several disks")),
        Err(e) => return out.push(Metric::unavailable(name, "boolean", format!("system disk could not be identified (error {e})"))),
    };
    let dev = match open_device(&format!(r"\\.\PhysicalDrive{disk_number}"), 0) {
        Ok(h) => h,
        Err(5) => return out.push(Metric::permission_required(name, "boolean", "opening the system disk was denied")),
        Err(e) => return out.push(Metric::unavailable(name, "boolean", format!("system disk could not be opened (error {e})"))),
    };
    let mut pf = [0u8; 516];
    let r = ioctl(dev, IOCTL_STORAGE_PREDICT_FAILURE, &[], &mut pf);
    close(dev);
    match r {
        Ok(n) if n >= 4 => {
            let predicted = u32::from_le_bytes([pf[0], pf[1], pf[2], pf[3]]) != 0;
            out.push(Metric::noted(name, if predicted { 1.0 } else { 0.0 }, "boolean", "Drive's own failure prediction (SMART / NVMe critical warning) via IOCTL_STORAGE_PREDICT_FAILURE; 0 = no failure predicted"));
        }
        Ok(_) => out.push(Metric::unavailable(name, "boolean", "the drive returned no prediction data")),
        Err(5) => out.push(Metric::permission_required(name, "boolean", "failure prediction query was denied")),
        Err(e) => out.push(Metric::unavailable(name, "boolean", format!("the drive or its driver does not support failure prediction (error {e})"))),
    }
}

fn battery_metrics(out: &mut Vec<Metric>) {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: output struct is local.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
        out.push(Metric::unavailable(names::BATTERY_PERCENT, "percent", "GetSystemPowerStatus failed"));
        return;
    }
    match s.ACLineStatus {
        0 | 1 => out.push(Metric::value(names::BATTERY_ON_AC, f64::from(s.ACLineStatus), "boolean")),
        _ => out.push(Metric::unavailable(names::BATTERY_ON_AC, "boolean", "power source is reported as unknown")),
    }
    if s.BatteryFlag & 128 != 0 {
        for (n, u) in [(names::BATTERY_PERCENT, "percent"), (names::BATTERY_HEALTH, "percent"), ("battery.cycle_count", "count")] {
            out.push(Metric::unavailable(n, u, "no system battery"));
        }
        return;
    }
    if s.BatteryLifePercent <= 100 {
        out.push(Metric::value(names::BATTERY_PERCENT, f64::from(s.BatteryLifePercent), "percent"));
    } else {
        out.push(Metric::unavailable(names::BATTERY_PERCENT, "percent", "charge level is reported as unknown"));
    }
    match battery_information() {
        Ok((designed, full, cycles)) => {
            if designed > 0 && designed != u32::MAX && full != u32::MAX {
                out.push(Metric::value(names::BATTERY_HEALTH, round1(f64::from(full) * 100.0 / f64::from(designed)), "percent"));
            } else {
                out.push(Metric::unavailable(names::BATTERY_HEALTH, "percent", "the battery does not report its design and full-charge capacity"));
            }
            if cycles > 0 {
                out.push(Metric::value("battery.cycle_count", f64::from(cycles), "count"));
            } else {
                out.push(Metric::unavailable("battery.cycle_count", "count", "the battery reports 0 cycles, which its driver uses for 'not supported'"));
            }
        }
        Err(why) => {
            out.push(Metric::unavailable(names::BATTERY_HEALTH, "percent", why.clone()));
            out.push(Metric::unavailable("battery.cycle_count", "count", why));
        }
    }
}

/// (DesignedCapacity, FullChargedCapacity, CycleCount) of the first system battery, read through the battery class
/// driver with a read-only handle.
fn battery_information() -> Result<(u32, u32, u32), String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, GUID_DEVCLASS_BATTERY, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows_sys::Win32::System::Power::{
        BATTERY_INFORMATION, BATTERY_QUERY_INFORMATION, BATTERY_SYSTEM_BATTERY, BatteryInformation, IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_TAG,
    };
    // SAFETY: class enumeration with documented flags; the list is destroyed below.
    let devs = unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_BATTERY, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
    if devs == -1 {
        return Err("battery devices could not be enumerated".into());
    }
    let result = (|| {
        let mut did = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..unsafe { std::mem::zeroed() } };
        // SAFETY: first interface of the battery class.
        if unsafe { SetupDiEnumDeviceInterfaces(devs, std::ptr::null(), &GUID_DEVCLASS_BATTERY, 0, &mut did) } == 0 {
            return Err("no battery device interface is present".to_owned());
        }
        let mut need = 0u32;
        // SAFETY: size query.
        unsafe { SetupDiGetDeviceInterfaceDetailW(devs, &did, std::ptr::null_mut(), 0, &mut need, std::ptr::null_mut()) };
        if need == 0 {
            return Err("battery device path is unavailable".to_owned());
        }
        let mut buf = vec![0u32; (need as usize).div_ceil(4) + 1];
        let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: cbSize is the fixed struct size, as the API requires; buffer holds `need` bytes.
        unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
        // SAFETY: as above.
        if unsafe { SetupDiGetDeviceInterfaceDetailW(devs, &did, detail, need, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return Err("battery device path is unavailable".to_owned());
        }
        // SAFETY: DevicePath is a NUL-terminated string inside `buf`.
        let path_ptr = unsafe { std::ptr::addr_of!((*detail).DevicePath) }.cast::<u16>();
        let len = (0..).take_while(|&i| unsafe { *path_ptr.add(i) } != 0).count();
        // SAFETY: `len` units precede the terminator.
        let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path_ptr, len) });
        let h = open_device(&path, GENERIC_READ).map_err(|e| format!("battery device could not be opened (error {e})"))?;
        let r = (|| {
            let mut tag = [0u8; 4];
            ioctl(h, IOCTL_BATTERY_QUERY_TAG, &0u32.to_le_bytes(), &mut tag).map_err(|e| format!("battery tag query failed (error {e})"))?;
            let q = BATTERY_QUERY_INFORMATION { BatteryTag: u32::from_le_bytes(tag), InformationLevel: BatteryInformation, AtRate: 0 };
            // SAFETY: plain-old-data view of the query struct.
            let qb = unsafe { std::slice::from_raw_parts((&q as *const BATTERY_QUERY_INFORMATION).cast::<u8>(), size_of::<BATTERY_QUERY_INFORMATION>()) };
            let mut info: BATTERY_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: plain-old-data output view.
            let ib = unsafe { std::slice::from_raw_parts_mut((&mut info as *mut BATTERY_INFORMATION).cast::<u8>(), size_of::<BATTERY_INFORMATION>()) };
            ioctl(h, IOCTL_BATTERY_QUERY_INFORMATION, qb, ib).map_err(|e| format!("battery information query failed (error {e})"))?;
            if info.Capabilities & BATTERY_SYSTEM_BATTERY == 0 {
                return Err("the battery device is not a system battery (UPS)".to_owned());
            }
            Ok((info.DesignedCapacity, info.FullChargedCapacity, info.CycleCount))
        })();
        close(h);
        r
    })();
    // SAFETY: list from SetupDiGetClassDevsW, destroyed once.
    unsafe { SetupDiDestroyDeviceInfoList(devs) };
    result
}

/// One snapshot with the fixed first window (about one second).
pub fn snapshot() -> StatusSnapshot {
    Sampler::new().sample()
}

#[cfg(test)]
mod tests;
