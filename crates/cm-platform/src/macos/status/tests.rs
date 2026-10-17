use std::collections::BTreeMap;

use cm_core::status::network::{Classification, InterfaceClass, NetworkTotals, classify};
use cm_core::status::{Availability, names};

use super::*;

fn metric<'a>(s: &'a StatusSnapshot, name: &str) -> &'a Metric {
    s.metrics.iter().find(|m| m.name == name).unwrap_or_else(|| panic!("metric {name} missing"))
}

/// STS-01: every metric is a finite number, or explicitly unknown with a reason and no value (never 0 for unknown).
#[test]
fn every_metric_is_a_number_or_explicitly_unknown() {
    let s = snapshot();
    let mut seen = std::collections::BTreeSet::new();
    for m in &s.metrics {
        eprintln!("{:<52} {:<20?} {:>18} {:<16} {}", m.name, m.availability, m.value.map(|v| format!("{v}")).unwrap_or_else(|| "null".into()), m.unit, m.reason.as_deref().unwrap_or(""));
        assert!(seen.insert(m.name.clone()), "{} reported twice", m.name);
        match m.availability {
            Availability::Available => assert!(m.value.is_some_and(f64::is_finite), "{m:?}"),
            _ => {
                assert!(m.value.is_none(), "an unknown metric carries no value: {m:?}");
                assert!(m.reason.as_deref().is_some_and(|r| !r.trim().is_empty()), "an unknown metric says why: {m:?}");
            }
        }
    }
    for n in [names::CPU_TOTAL, names::MEMORY_USED, names::SYSTEM_FREE_PERCENT, names::SYSTEM_FREE_BYTES, "memory.total_bytes", "process.zombie_count", "network.received_bytes_per_sec"] {
        let m = metric(&s, n);
        assert_eq!(m.availability, Availability::Available, "{m:?}");
    }
    let cpu = metric(&s, names::CPU_TOTAL).value.unwrap();
    assert!((0.0..=100.0).contains(&cpu), "{cpu}");
    let used = metric(&s, names::MEMORY_USED).value.unwrap();
    assert!(used > 0.0 && used <= 100.0, "{used}");
    // Sensors are private on macOS: unknown with the reason, never a number.
    for n in [names::THERMAL_MAX, "fan.rpm", "gpu.temperature_celsius"] {
        let m = metric(&s, n);
        assert_eq!((m.availability, m.value, m.reason.as_deref()), (Availability::Unavailable, None, Some(SENSOR_REASON)));
    }
    for n in [names::BATTERY_PERCENT, names::BATTERY_HEALTH, "battery.cycle_count", names::BATTERY_ON_AC, "gpu.utilization_percent", names::DISK_BUSY] {
        let m = metric(&s, n);
        assert!(m.availability == Availability::Available || (m.value.is_none() && m.reason.is_some()), "{m:?}");
    }
    // The thermal dimension is unknown, so it is left out of the score instead of lowering it.
    let thermal = s.health.dimensions.iter().find(|d| d.metric == names::THERMAL_MAX).unwrap();
    assert_eq!((thermal.value, thermal.score), (None, None));
    assert!(s.health.score.is_some());
    let net = s.network.as_ref().expect("network totals");
    assert_eq!(net.classification, Classification::InterfaceType);
    assert!(net.loopback_excluded >= 1, "lo0 is excluded: {net:?}");
    assert!(net.counted.iter().chain(&net.separate).all(|r| !r.name.starts_with("lo")), "{net:?}");
    assert!(net.separate.iter().all(|r| matches!(r.class, InterfaceClass::Tunnel | InterfaceClass::Virtual)));
    eprintln!("network: {}", serde_json::to_string(net).unwrap());
    let wire = s.to_wire();
    assert!(wire["metrics"].as_array().unwrap().iter().all(|m| m["availability"] == "available" || m["value"].is_null()));
}

#[test]
fn a_second_sample_covers_the_interval_since_the_first() {
    let mut s = Sampler::new();
    let a = s.sample();
    let b = s.sample();
    assert!(b.sampled_at >= a.sampled_at);
    assert_eq!(metric(&b, names::CPU_TOTAL).availability, Availability::Available);
    // The busiest process needs two readings; a sample never invents it.
    let w = sample(Duration::from_millis(1000));
    assert_eq!(metric(&w, names::CPU_TOTAL).availability, Availability::Available);
}

#[test]
fn cpu_ticks_wrap_and_idle_share() {
    let prev = [[u32::MAX - 9, 0, 0, 0]];
    let now = [[10, 0, 10, 0]];
    // 20 user ticks across the wrap, 10 idle: two thirds busy.
    assert_eq!(busy_percent(&prev, &now), Some(66.7));
    assert_eq!(busy_percent(&prev, &prev), None, "no ticks elapsed");
    assert_eq!(busy_percent(&prev, &[[0; 4], [0; 4]]), None, "processor count changed");
    let host = Host::new();
    let ticks = cpu_ticks(&host).expect("host_processor_info");
    assert_eq!(ticks.len(), std::thread::available_parallelism().unwrap().get());
}

fn ifmsg(index: u16, flags: i32, ifi_type: u8, rx: u64, tx: u64, name: Option<&str>) -> Vec<u8> {
    // SAFETY: plain-old-data struct, all-zero is valid.
    let mut h: libc::if_msghdr2 = unsafe { std::mem::zeroed() };
    h.ifm_version = 5;
    h.ifm_type = RTM_IFINFO2;
    h.ifm_index = index;
    h.ifm_flags = flags;
    h.ifm_addrs = if name.is_some() { RTA_IFP } else { 0 };
    h.ifm_data.ifi_type = ifi_type;
    h.ifm_data.ifi_ibytes = rx;
    h.ifm_data.ifi_obytes = tx;
    let mut sdl = Vec::new();
    if let Some(n) = name {
        let len = (8 + n.len()).max(20);
        sdl = vec![0u8; len];
        sdl[0] = len as u8;
        sdl[1] = libc::AF_LINK as u8;
        sdl[2..4].copy_from_slice(&index.to_ne_bytes());
        sdl[4] = ifi_type;
        sdl[5] = n.len() as u8;
        sdl[8..8 + n.len()].copy_from_slice(n.as_bytes());
    }
    h.ifm_msglen = (IF_MSGHDR2_SIZE + sdl.len()) as u16;
    // SAFETY: viewing the POD header as bytes.
    let mut out = unsafe { std::slice::from_raw_parts((&h as *const libc::if_msghdr2).cast::<u8>(), IF_MSGHDR2_SIZE) }.to_vec();
    out.extend(sdl);
    out
}

/// `if_msghdr2` records: names from the link address, 64-bit counters, other message types skipped, truncation safe.
#[test]
fn iflist2_records_are_parsed_without_reading_past_the_buffer() {
    let mut buf = ifmsg(4, libc::IFF_UP, 6, 5_000_000_000_000, 7, Some("en0"));
    // An address message (RTM_NEWADDR = 0xc) between interfaces is skipped.
    buf.extend([8u8, 0, 5, 0x0c, 0, 0, 0, 0]);
    buf.extend(ifmsg(65_000, libc::IFF_UP | libc::IFF_POINTOPOINT, 1, 1, 2, None));
    let rows = parse_iflist2(&buf);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0], IfCounters { index: 4, name: "en0".into(), if_type: 6, flags: libc::IFF_UP, rx: 5_000_000_000_000, tx: 7 });
    assert_eq!((rows[1].name.as_str(), rows[1].rx, rows[1].tx), ("if65000", 1, 2), "no link address and no such index: a stable fallback name");
    // A truncated tail, a zero length and garbage end the walk without a panic.
    let mut cut = buf.clone();
    cut.extend(ifmsg(5, libc::IFF_UP, 6, 1, 1, Some("en1")));
    cut.truncate(cut.len() - 30);
    assert_eq!(parse_iflist2(&cut).len(), 2);
    assert!(parse_iflist2(&[0, 0, 0, 0, 9, 9]).is_empty());
    assert!(parse_iflist2(&[200, 0, 5]).is_empty());
    let mut lying = ifmsg(6, libc::IFF_UP, 6, 1, 1, Some("en2"));
    lying[IF_MSGHDR2_SIZE + 5] = 200; // sdl_nlen past the end
    assert_eq!(parse_iflist2(&lying)[0].name, index_name(6).unwrap_or_else(|| "if6".into()));
}

#[test]
fn counters_wrap_but_resets_have_no_rate() {
    assert_eq!(counter_delta(100, 250), Some(150));
    assert_eq!(counter_delta(u64::MAX - 9, 5), Some(15), "64-bit wrap");
    assert_eq!(counter_delta(9_000_000_000, 1_000), None, "counter reset (interface re-created)");
    let prev: IfOctets = [(4, ("en0".to_owned(), u64::MAX - 99, 10)), (7, ("utun2".to_owned(), 1_000_000_000, 0)), (9, ("en5".into(), 0, 0)), (11, ("old0".into(), 0, 0))].into_iter().collect();
    let rows = vec![
        IfCounters { index: 4, name: "en0".into(), if_type: 6, flags: libc::IFF_UP, rx: 100, tx: 30 },
        IfCounters { index: 7, name: "utun2".into(), if_type: 1, flags: libc::IFF_UP | libc::IFF_POINTOPOINT, rx: 5, tx: 0 },
        IfCounters { index: 9, name: "en5".into(), if_type: 6, flags: 0, rx: 10, tx: 10 },
        IfCounters { index: 11, name: "new0".into(), if_type: 6, flags: libc::IFF_UP, rx: 10, tx: 10 },
    ];
    let rates = interface_rates(&prev, &rows, 2.0);
    // en0 wrapped (200 bytes over 2 s); utun2 was reset; en5 is down; index 11 now names another interface.
    assert_eq!(rates.len(), 1, "{rates:?}");
    assert_eq!((rates[0].name.as_str(), rates[0].received_bytes_per_sec, rates[0].sent_bytes_per_sec), ("en0", 100.0, 10.0));
    assert_eq!(rates[0].class, InterfaceClass::Physical);
}

/// STS-07 on macOS: loopback excluded; utun/ipsec/ppp tunnels and bridge/vmnet/anpi virtual adapters apart.
#[test]
fn interfaces_are_classified_by_family_flags_and_type() {
    use InterfaceClass::*;
    let c = |name: &str, t: u8, flags: i32| classify(&interface_facts(name, t, flags));
    let up = libc::IFF_UP;
    assert_eq!(c("lo0", 24, up | libc::IFF_LOOPBACK), Loopback);
    assert_eq!(c("en0", 6, up), Physical);
    assert_eq!(c("en7", 71, up), Physical);
    assert_eq!(c("utun3", 1, up | libc::IFF_POINTOPOINT), Tunnel);
    assert_eq!(c("utun0", 0, up), Tunnel);
    assert_eq!(c("ipsec0", 1, up), Tunnel);
    assert_eq!(c("ppp0", 23, up | libc::IFF_POINTOPOINT), Tunnel);
    assert_eq!(c("gif0", 55, 0), Tunnel);
    assert_eq!(c("stf0", 57, 0), Tunnel);
    assert_eq!(c("bridge100", 209, up), Virtual);
    assert_eq!(c("bridge0", 209, up), Virtual);
    assert_eq!(c("vmnet8", 6, up), Virtual);
    assert_eq!(c("vmenet0", 6, up), Virtual);
    assert_eq!(c("anpi0", 6, up), Virtual);
    assert_eq!(c("awdl0", 6, up), Virtual);
    assert_eq!(c("llw0", 6, up), Virtual);
    assert_eq!(c("ap1", 6, up), Virtual);
    assert_eq!(c("XHC20", 1, 0), Virtual);
    assert_eq!(c("odd0", 1, up), Unknown);
    assert_eq!(c("en9", 1, up), Unknown, "an en* of another type is not assumed physical");
    // The VPN case: the tunnel's traffic also crosses en0, so the total counts en0 only.
    let rates = |name: &str, t: u8, flags: i32, rx: f64| cm_core::status::network::InterfaceRate { name: name.into(), class: c(name, t, flags), received_bytes_per_sec: rx, sent_bytes_per_sec: 0.0 };
    let t = NetworkTotals::from_rates(Classification::InterfaceType, vec![rates("en0", 6, up, 1000.0), rates("utun4", 1, up | libc::IFF_POINTOPOINT, 900.0), rates("lo0", 24, up | libc::IFF_LOOPBACK, 50.0), rates("bridge100", 209, up, 20.0)]);
    assert_eq!((t.received_bytes_per_sec, t.may_double_count, t.loopback_excluded), (1000.0, false, 1));
    assert_eq!(t.separate.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["utun4", "bridge100"]);
}

#[test]
fn the_real_interface_list_has_loopback() {
    let rows = parse_iflist2(&read_iflist2().expect("NET_RT_IFLIST2"));
    for r in &rows {
        eprintln!("{:<10} index={:<3} type={:<3} flags={:#06x} class={:?}", r.name, r.index, r.if_type, r.flags, classify(&interface_facts(&r.name, r.if_type, r.flags)));
    }
    let lo = rows.iter().find(|r| r.name == "lo0").expect("lo0");
    assert_eq!(classify(&interface_facts(&lo.name, lo.if_type, lo.flags)), InterfaceClass::Loopback);
    assert!(rows.iter().all(|r| index_name(r.index).as_deref() == Some(r.name.as_str())), "names agree with if_indextoname");
}

#[test]
fn disk_statistics_and_resets() {
    let stats = Plist::Dict(
        [("Bytes (Read)", 4096i64), ("Bytes (Write)", 8192), ("Total Time (Read)", 1_000_000), ("Total Time (Write)", 3_000_000), ("Operations (Read)", 1)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), Plist::Integer(v)))
            .collect(),
    );
    let c = disk_counters_from(&stats).unwrap();
    assert_eq!(c, DiskCounters { read_bytes: 4096, write_bytes: 8192, read_ns: 1_000_000, write_ns: 3_000_000 });
    assert_eq!(disk_counters_from(&Plist::Dict(BTreeMap::new())), None);
    let a: BTreeMap<u64, DiskCounters> = [(1, c), (2, DiskCounters { read_bytes: 1 << 40, ..c })].into_iter().collect();
    let b: BTreeMap<u64, DiskCounters> = [(1, DiskCounters { read_bytes: 4096 + 2000, write_bytes: 8192 + 4000, read_ns: 1_000_000 + 500_000_000, write_ns: 3_000_000 }), (2, c)].into_iter().collect();
    // Driver 2 went backwards (re-attached): no rate from it this interval.
    assert_eq!(disk_rates(&a, &b, 2.0), Some((1000.0, 2000.0, 25.0)));
    assert_eq!(disk_rates(&a, &BTreeMap::new(), 2.0), None);
    match disk_counters() {
        Some(d) => eprintln!("IOBlockStorageDriver statistics: {d:?}"),
        None => eprintln!("no IOBlockStorageDriver statistics on this machine"),
    }
}

#[test]
fn battery_health_prefers_raw_capacity() {
    assert_eq!(battery_health(Some(5000), Some(4500), Some(100)), Some(90.0), "Apple silicon: MaxCapacity is a percentage");
    assert_eq!(battery_health(Some(5000), None, Some(4000)), Some(80.0), "Intel: MaxCapacity in mAh");
    assert_eq!(battery_health(Some(5000), None, Some(100)), None, "a percentage is not a capacity");
    assert_eq!(battery_health(None, Some(4500), None), None);
    assert_eq!(battery_health(Some(0), Some(4500), None), None);
    assert_eq!(pressure_name(1), Some("normal"));
    assert_eq!(pressure_name(4), Some("critical"));
    assert_eq!(pressure_name(3), None);
}

/// N13: a zombie child (exited, not collected) is counted.
#[test]
fn zombie_count_detects_a_test_zombie() {
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id() as i32;
    // SAFETY: zeroed POD out-parameter; WNOWAIT leaves the child waitable, so it stays a zombie.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: our own child pid and a valid out-pointer.
    assert_eq!(unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) }, 0);
    assert!(zombies().unwrap().contains(&pid));
    let mut ms = Vec::new();
    process_metrics(None, 1, &mut ms);
    let z = ms.iter().find(|m| m.name == "process.zombie_count").unwrap();
    assert!(z.value.is_some_and(|v| v >= 1.0), "{z:?}");
    child.wait().unwrap();
    assert!(!zombies().unwrap().contains(&pid), "collected: no longer a zombie");
}

#[test]
fn capability_row_is_unverified_with_a_reason() {
    assert_eq!(CAPS.len(), 1);
    assert_eq!((CAPS[0].0, CAPS[0].1), ("status.metrics", CapabilityState::Unverified));
    assert!(CAPS[0].2.contains("hosted runners only"));
}
