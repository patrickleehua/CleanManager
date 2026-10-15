use std::collections::BTreeSet;

use cm_core::status::{Availability, names};

use super::*;

const MODULE_SOURCE: &str = include_str!("../status.rs");

#[test]
fn module_never_writes_or_executes() {
    let forbidden = [
        ["GENERIC_", "WRITE"].concat(),
        ["FILE_", "WRITE_DATA"].concat(),
        ["Create", "Process"].concat(),
        ["std::process", "::Command"].concat(),
        ["Terminate", "Process"].concat(),
        ["Set", "SystemPowerState"].concat(),
    ];
    for token in &forbidden {
        assert!(!MODULE_SOURCE.contains(token.as_str()), "status.rs must not reference {token}");
    }
}

#[test]
fn gpu_utilization_groups_like_task_manager() {
    let engines = vec![
        ("pid_10_luid_0x00000000_0x0000D1C5_phys_0_eng_0_engtype_3D".to_owned(), 20.0),
        ("pid_11_luid_0x00000000_0x0000D1C5_phys_0_eng_0_engtype_3D".to_owned(), 15.0),
        ("pid_10_luid_0x00000000_0x0000D1C5_phys_0_eng_3_engtype_VideoDecode".to_owned(), 30.0),
        ("pid_12_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D".to_owned(), 5.0),
    ];
    assert_eq!(gpu_utilization(&engines), 35.0);
    assert_eq!(gpu_utilization(&[("pid_1_luid_a_phys_0_eng_0_engtype_3D".into(), 80.0), ("pid_2_luid_a_phys_0_eng_0_engtype_3D".into(), 70.0)]), 100.0);
    assert_eq!(gpu_utilization(&[]), 0.0);
}

#[test]
fn top_process_is_normalized_to_the_machine() {
    let procs = vec![("_Total".to_owned(), 800.0), ("Idle".to_owned(), 600.0), ("svchost".to_owned(), 10.0), ("svchost#1".to_owned(), 20.0), ("busy".to_owned(), 200.0)];
    let t = top_process(&procs, 8).unwrap();
    assert_eq!(t.name, "busy");
    assert_eq!(t.cpu_percent, 25.0);
    assert!(top_process(&[("_Total".into(), 1.0)], 4).is_none());
}

/// Real machine: structure only. Every metric declares availability; unavailable metrics are null with a reason.
#[test]
fn real_snapshot_declares_availability() {
    let mut sampler = Sampler::new();
    let s = sampler.sample();
    let names_seen: BTreeSet<&str> = s.metrics.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names_seen.len(), s.metrics.len(), "metric names are unique");
    for m in &s.metrics {
        match m.availability {
            Availability::Available => assert!(m.value.is_some_and(f64::is_finite), "{}", m.name),
            _ => assert!(m.value.is_none() && m.reason.is_some(), "{} must be null with a reason", m.name),
        }
        eprintln!("{:?} {} = {:?} {} {}", m.availability, m.name, m.value, m.unit, m.reason.as_deref().unwrap_or(""));
    }
    for required in [names::CPU_TOTAL, names::MEMORY_USED, "memory.total_bytes", names::SYSTEM_FREE_PERCENT, names::THERMAL_MAX, names::DISK_FAILURE_PREDICTED, "fan.rpm", names::BATTERY_PERCENT, "network.received_bytes_per_sec", "gpu.utilization_percent"] {
        assert!(names_seen.contains(required), "{required} is always declared (available or not)");
    }
    let fan = s.metrics.iter().find(|m| m.name == "fan.rpm").unwrap();
    assert_eq!(fan.availability, Availability::Unavailable);
    let get = |n: &str| s.metrics.iter().find(|m| m.name == n).and_then(|m| m.value);
    let cpu = get(names::CPU_TOTAL).expect("CPU load is available on Windows");
    assert!((0.0..=100.0).contains(&cpu));
    assert!(get(names::MEMORY_USED).is_some_and(|v| (0.0..=100.0).contains(&v)));
    assert!(get(names::SYSTEM_FREE_PERCENT).is_some_and(|v| (0.0..=100.0).contains(&v)));
    assert!(s.health.score.is_some_and(|v| v <= 100));
    let w = s.to_wire();
    assert_eq!(w["health"]["dimensions"].as_array().unwrap().len(), 5);
    assert!(w["diagnosis"]["code"].is_string());
    // A second sample from the same sampler covers the interval since the first (rates stay finite).
    let s2 = sampler.sample();
    assert!(s2.sampled_at >= s.sampled_at);
}

/// STS-07: rates come from counter differences; an interface new since the last reading or whose counters reset has
/// no rate for the interval; classification follows the reported type and hardware flag.
#[test]
fn interface_rates_difference_counters_and_classify() {
    use cm_core::status::network::{InterfaceClass, InterfaceFacts};
    let row = |luid: u64, name: &str, if_type: u32, tunnel_type: u32, hardware: bool, received: u64, sent: u64| IfRow {
        luid,
        name: name.into(),
        facts: InterfaceFacts { if_type, tunnel_type, hardware: Some(hardware) },
        received,
        sent,
    };
    let prev: BTreeMap<u64, (u64, u64)> = [(1, (1_000, 500)), (2, (0, 0)), (3, (10, 10)), (4, (9_999, 9_999))].into_iter().collect();
    let rows = [
        row(1, "Wi-Fi", 71, 0, true, 3_000, 1_500),
        row(2, "vEthernet (WSL)", 6, 0, false, 200, 100),
        row(3, "Teredo", 131, 14, false, 10, 10),
        row(4, "reset", 6, 0, true, 5, 5),
        row(5, "new", 6, 0, true, 50, 50),
    ];
    let r = interface_rates(&prev, &rows, 2.0);
    let got: Vec<(&str, InterfaceClass, f64, f64)> = r.iter().map(|x| (x.name.as_str(), x.class, x.received_bytes_per_sec, x.sent_bytes_per_sec)).collect();
    assert_eq!(got, [("Wi-Fi", InterfaceClass::Physical, 1_000.0, 500.0), ("vEthernet (WSL)", InterfaceClass::Virtual, 100.0, 50.0), ("Teredo", InterfaceClass::Tunnel, 0.0, 0.0)]);
    assert!(interface_rates(&prev, &rows, 0.0).is_empty());
}

/// Real machine: the interface table is readable and the snapshot carries classified network totals.
#[test]
fn real_snapshot_classifies_interfaces() {
    let rows = interface_table().expect("GetIfTable2 is readable");
    for r in &rows {
        eprintln!("if {:?} type={} tunnel={} hardware={:?} -> {:?}", r.name, r.facts.if_type, r.facts.tunnel_type, r.facts.hardware, cm_core::status::network::classify(&r.facts));
    }
    let s = Sampler::new().sample();
    let n = s.network.as_ref().expect("network totals are sampled");
    assert_eq!(n.classification, cm_core::status::network::Classification::InterfaceType);
    let total = s.metrics.iter().find(|m| m.name == "network.received_bytes_per_sec").and_then(|m| m.value).unwrap();
    assert_eq!(total, n.received_bytes_per_sec);
    assert!(n.separate.iter().all(|i| matches!(i.class, cm_core::status::network::InterfaceClass::Tunnel | cm_core::status::network::InterfaceClass::Virtual)));
    eprintln!("{:#}", s.to_wire()["network"]);
}
