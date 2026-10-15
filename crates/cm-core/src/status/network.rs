//! Network totals without double counting (parity STS-07, N13).
//!
//! The total counts physical interfaces only. Loopback is excluded. Tunnels (VPN, PPP, IPv6 transition) and other
//! virtual adapters (Hyper-V / WSL `vEthernet`, VM host-only networks, TAP/Wintun drivers) carry traffic that the
//! physical interface also carries (encapsulated or switched), so they are listed separately and never added to the
//! total. An interface that cannot be classified is counted and the total says it may double count; so does a total
//! computed without any classification (the sampler could only read unlabelled counters).
//!
//! Interface types are IANA ifType numbers (RFC 1213 / IANAifType-MIB), which Windows (`MIB_IF_ROW2.Type`) and
//! macOS (`if_data.ifi_type`) both report.

use serde::{Deserialize, Serialize};

/// IANA ifType values the classification reads.
pub mod if_type {
    pub const OTHER: u32 = 1;
    pub const PPP: u32 = 23;
    pub const SOFTWARE_LOOPBACK: u32 = 24;
    pub const PROP_VIRTUAL: u32 = 53;
    pub const TUNNEL: u32 = 131;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceClass {
    Physical,
    Tunnel,
    Virtual,
    Loopback,
    Unknown,
}

/// What the platform reports about one interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceFacts {
    /// IANA ifType.
    pub if_type: u32,
    /// Tunnel encapsulation (`TUNNEL_TYPE`); 0 = none.
    pub tunnel_type: u32,
    /// The driver says the interface is backed by hardware (Windows `HardwareInterface`); `None` when not reported.
    pub hardware: Option<bool>,
}

/// Loopback by type; tunnel by type (tunnel, PPP) or encapsulation; virtual when the driver says it is not hardware
/// or the type is proprietary-virtual; physical only for a hardware interface of a concrete type; anything else
/// is unknown.
pub fn classify(f: &InterfaceFacts) -> InterfaceClass {
    match f.if_type {
        if_type::SOFTWARE_LOOPBACK => return InterfaceClass::Loopback,
        if_type::TUNNEL | if_type::PPP => return InterfaceClass::Tunnel,
        _ if f.tunnel_type != 0 => return InterfaceClass::Tunnel,
        if_type::PROP_VIRTUAL => return InterfaceClass::Virtual,
        _ => {}
    }
    match f.hardware {
        Some(false) => InterfaceClass::Virtual,
        Some(true) if f.if_type != if_type::OTHER && f.if_type != 0 => InterfaceClass::Physical,
        _ => InterfaceClass::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceRate {
    pub name: String,
    pub class: InterfaceClass,
    pub received_bytes_per_sec: f64,
    pub sent_bytes_per_sec: f64,
}

/// How the interfaces were told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Each interface was classified from its reported type and flags.
    InterfaceType,
    /// The platform gave only unlabelled per-interface counters.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkTotals {
    pub classification: Classification,
    /// Total over `counted`.
    pub received_bytes_per_sec: f64,
    pub sent_bytes_per_sec: f64,
    /// The total may include the same traffic twice (some counted interface is not known to be physical).
    pub may_double_count: bool,
    /// Interfaces in the total: physical, plus unknown ones (which set `may_double_count`).
    pub counted: Vec<InterfaceRate>,
    /// Tunnel and virtual adapters: shown, never added to the total.
    pub separate: Vec<InterfaceRate>,
    /// Loopback interfaces left out entirely.
    pub loopback_excluded: u32,
    pub note: String,
}

impl NetworkTotals {
    /// Totals from per-interface rates. With `Classification::Unavailable` every interface is treated as unknown.
    pub fn from_rates(classification: Classification, rates: Vec<InterfaceRate>) -> NetworkTotals {
        let (mut counted, mut separate, mut loopback_excluded) = (Vec::new(), Vec::new(), 0u32);
        for mut r in rates {
            if classification == Classification::Unavailable {
                r.class = InterfaceClass::Unknown;
            }
            match r.class {
                InterfaceClass::Loopback => loopback_excluded += 1,
                InterfaceClass::Tunnel | InterfaceClass::Virtual => separate.push(r),
                InterfaceClass::Physical | InterfaceClass::Unknown => counted.push(r),
            }
        }
        let by_traffic = |a: &InterfaceRate, b: &InterfaceRate| {
            (b.received_bytes_per_sec + b.sent_bytes_per_sec).total_cmp(&(a.received_bytes_per_sec + a.sent_bytes_per_sec)).then_with(|| a.name.cmp(&b.name))
        };
        counted.sort_by(by_traffic);
        separate.sort_by(by_traffic);
        let may_double_count = classification == Classification::Unavailable || counted.iter().any(|r| r.class == InterfaceClass::Unknown);
        let note = match (classification, may_double_count) {
            (Classification::Unavailable, _) => "Interfaces could not be classified: the total is the sum over all adapters and may count tunnel or virtual-switch traffic twice",
            (_, true) => "Physical interfaces plus interfaces of unknown kind; the total may count tunnel or virtual-switch traffic twice",
            (_, false) => "Physical interfaces only; loopback is excluded and tunnel or virtual adapters are listed separately, not added",
        };
        NetworkTotals {
            classification,
            received_bytes_per_sec: counted.iter().map(|r| r.received_bytes_per_sec).sum(),
            sent_bytes_per_sec: counted.iter().map(|r| r.sent_bytes_per_sec).sum(),
            may_double_count,
            counted,
            separate,
            loopback_excluded,
            note: note.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(if_type: u32, tunnel_type: u32, hardware: Option<bool>) -> InterfaceFacts {
        InterfaceFacts { if_type, tunnel_type, hardware }
    }

    fn rate(name: &str, class: InterfaceClass, rx: f64, tx: f64) -> InterfaceRate {
        InterfaceRate { name: name.into(), class, received_bytes_per_sec: rx, sent_bytes_per_sec: tx }
    }

    #[test]
    fn classification_rules() {
        use InterfaceClass::*;
        // Ethernet (6) and Wi-Fi (71) backed by hardware.
        assert_eq!(classify(&facts(6, 0, Some(true))), Physical);
        assert_eq!(classify(&facts(71, 0, Some(true))), Physical);
        // Loopback wins over any flag.
        assert_eq!(classify(&facts(if_type::SOFTWARE_LOOPBACK, 0, Some(false))), Loopback);
        // Tunnels: by type (Teredo/IP-HTTPS are 131), PPP (RAS VPN), or encapsulation on another type.
        assert_eq!(classify(&facts(if_type::TUNNEL, 14, Some(false))), Tunnel);
        assert_eq!(classify(&facts(if_type::PPP, 0, Some(false))), Tunnel);
        assert_eq!(classify(&facts(6, 3, Some(true))), Tunnel);
        // Hyper-V/WSL vEthernet and TAP drivers report Ethernet without hardware; Wintun reports proprietary-virtual.
        assert_eq!(classify(&facts(6, 0, Some(false))), Virtual);
        assert_eq!(classify(&facts(if_type::PROP_VIRTUAL, 0, Some(true))), Virtual);
        // Not enough to tell.
        assert_eq!(classify(&facts(6, 0, None)), Unknown);
        assert_eq!(classify(&facts(if_type::OTHER, 0, Some(true))), Unknown);
    }

    /// STS-07 "A VPN tunnel is active": the total counts only the physical interface; the tunnel is listed apart.
    #[test]
    fn tunnel_is_separate_and_loopback_excluded() {
        let t = NetworkTotals::from_rates(
            Classification::InterfaceType,
            vec![
                rate("Wi-Fi", InterfaceClass::Physical, 1000.0, 200.0),
                rate("Corp VPN", InterfaceClass::Tunnel, 900.0, 180.0),
                rate("vEthernet (WSL)", InterfaceClass::Virtual, 50.0, 10.0),
                rate("Loopback Pseudo-Interface 1", InterfaceClass::Loopback, 7.0, 7.0),
            ],
        );
        assert_eq!((t.received_bytes_per_sec, t.sent_bytes_per_sec), (1000.0, 200.0));
        assert!(!t.may_double_count);
        assert_eq!(t.counted.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Wi-Fi"]);
        assert_eq!(t.separate.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Corp VPN", "vEthernet (WSL)"]);
        assert_eq!(t.loopback_excluded, 1);
        let w = serde_json::to_value(&t).unwrap();
        assert_eq!(w["classification"], "interface_type");
        assert_eq!(w["separate"][0]["class"], "tunnel");
    }

    #[test]
    fn unknown_interfaces_are_counted_and_flagged() {
        let t = NetworkTotals::from_rates(Classification::InterfaceType, vec![rate("Ethernet", InterfaceClass::Physical, 10.0, 1.0), rate("odd0", InterfaceClass::Unknown, 5.0, 2.0)]);
        assert_eq!((t.received_bytes_per_sec, t.sent_bytes_per_sec), (15.0, 3.0));
        assert!(t.may_double_count);
        assert!(t.note.contains("may count"));
    }

    #[test]
    fn unclassified_sum_says_it_may_double_count() {
        let t = NetworkTotals::from_rates(Classification::Unavailable, vec![rate("Intel(R) Wi-Fi", InterfaceClass::Physical, 10.0, 1.0), rate("Hyper-V Virtual Ethernet Adapter", InterfaceClass::Virtual, 10.0, 1.0)]);
        assert_eq!(t.received_bytes_per_sec, 20.0);
        assert!(t.may_double_count);
        assert!(t.separate.is_empty());
        assert!(t.counted.iter().all(|r| r.class == InterfaceClass::Unknown));
        let empty = NetworkTotals::from_rates(Classification::InterfaceType, Vec::new());
        assert_eq!((empty.received_bytes_per_sec, empty.may_double_count), (0.0, false));
    }
}
