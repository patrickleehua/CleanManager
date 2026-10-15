use std::collections::BTreeSet;

use cm_core::apps::{AppSource, Coverage, associate, exclusive_space, merge_instances};
use cm_core::capability::CapabilityState;
use cm_core::model::Timestamp;

use super::*;

/// The module source, used to prove at test time that it contains no write or execute path.
const MODULE_SOURCE: &str = include_str!("../apps.rs");

#[test]
fn module_has_no_registry_write_or_command_path() {
    // Assembled from parts so this test file does not itself contain the tokens.
    let forbidden = [
        ["RegSet", "Value"].concat(),
        ["RegCreate", "Key"].concat(),
        ["RegDelete", "Key"].concat(),
        ["RegDelete", "Value"].concat(),
        ["KEY_", "WRITE"].concat(),
        ["KEY_ALL", "_ACCESS"].concat(),
        ["KEY_SET", "_VALUE"].concat(),
        ["Uninstall", "String"].concat(),
        ["Modify", "Path"].concat(),
        ["Shell", "Execute"].concat(),
        ["Create", "Process"].concat(),
        ["std::process", "::Command"].concat(),
    ];
    for token in &forbidden {
        assert!(!MODULE_SOURCE.contains(token.as_str()), "apps.rs must not reference {token}");
    }
    assert_eq!(read_access(KEY_WOW64_64KEY), KEY_READ | KEY_WOW64_64KEY);
    assert_eq!(read_access(KEY_WOW64_32KEY), KEY_READ | KEY_WOW64_32KEY);
    assert_eq!(read_access(0), KEY_READ);
}

#[test]
fn location_normalization_and_product_codes() {
    let s = |t: &str, expand: bool| RegString { units: t.encode_utf16().collect(), expand };
    let ok = |t: &str| String::from_utf16(&normalize_location(&s(t, false)).unwrap()).unwrap();
    assert_eq!(ok(" \"C:\\Program Files\\App\\\" "), "C:\\Program Files\\App");
    assert_eq!(ok("C:\\"), "C:\\");
    assert_eq!(ok("\\\\server\\share\\app\\"), "\\\\server\\share\\app");
    assert!(normalize_location(&s("relative\\dir", false)).is_err());
    assert!(normalize_location(&s("%CM_SURELY_UNDEFINED_VAR%\\x", true)).is_err());
    let expanded = String::from_utf16(&normalize_location(&s("%SystemRoot%\\Temp", true)).unwrap()).unwrap();
    assert!(!expanded.contains('%') && expanded.ends_with("\\Temp"));
    assert!(is_product_code("{23170F69-40C1-2702-2408-000001000000}"));
    assert!(!is_product_code("7-Zip"));
    assert_eq!(split_full_name("Microsoft.App_1.2.3.0_x64__8wekyb3d8bbwe"), Some(("Microsoft.App", "1.2.3.0", "8wekyb3d8bbwe")));
    assert_eq!(split_full_name("not-a-package"), None);
}

#[test]
fn real_inventory_is_read_only_well_formed_and_unique() {
    let (apps, issues) = inventory();
    for i in &issues {
        eprintln!("issue: {} {} {}", i.code, i.source, i.message);
    }
    assert!(!apps.is_empty(), "a Windows host always has uninstall entries or packages");
    let ids: BTreeSet<_> = apps.iter().map(|a| a.instance_id.clone()).collect();
    assert_eq!(ids.len(), apps.len(), "instance IDs are unique");
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for a in &apps {
        assert!(!a.name.trim().is_empty());
        assert!(!matches!(a.source, AppSource::Unknown), "every Windows record has a known source");
        assert!(!a.evidence.is_empty());
        assert_eq!(a.uninstall_capability.state, CapabilityState::Unsupported);
        assert_eq!(a.instance_id, cm_core::apps::instance_id_of(&a.source, a.install_location.as_ref()));
        if a.location_identity.is_some() {
            assert!(a.install_location.is_some(), "identity only for a recorded location");
        }
        *counts.entry(a.source.wire_name()).or_default() += 1;
    }
    let with_identity = apps.iter().filter(|a| a.location_identity.is_some()).count();
    let protected = apps.iter().filter(|a| a.protected_class.is_some()).count();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    let dup_names = apps.iter().filter(|a| !names.insert(a.name.as_str())).count();
    eprintln!("apps={} by_source={counts:?} with_identity={with_identity} protected={protected} same_name_extra={dup_names} issues={}", apps.len(), issues.len());

    // Merging again is a no-op: real records never collapse by name.
    assert_eq!(merge_instances(apps.clone()).len(), apps.len());

    // Associations and space attribution work on real data without double counting.
    let report = associate(&apps, &Timestamp("2026-10-15T00:00:00Z".into()));
    assert_eq!(report.associations.len() + report.without_location.len(), apps.len());
    let shared = report.associations.iter().filter(|a| !a.shared_with.is_empty()).count();
    let unresolved = report.associations.iter().filter(|a| a.coverage != Coverage::Known).count();
    eprintln!("associations={} shared={shared} unresolved={unresolved} without_location={}", report.associations.len(), report.without_location.len());
    let space = exclusive_space(&report.associations, |_| None).unwrap();
    assert!(space.exclusive.keys().all(|id| report.associations.iter().any(|a| &a.instance_id == id && a.is_exclusive())));
}

#[test]
fn synthetic_same_name_records_stay_separate() {
    let a = AppInstance::new(
        "Same".into(),
        AppSource::RegistryUninstall { hive: RegistryHive::Hklm, view: RegistryView::View64, key: "Same".into() },
        RawPath::windows("C:\\A".encode_utf16().collect()).ok(),
    );
    let b = AppInstance::new(
        "Same".into(),
        AppSource::RegistryUninstall { hive: RegistryHive::Hklm, view: RegistryView::View32, key: "Same".into() },
        RawPath::windows("C:\\A".encode_utf16().collect()).ok(),
    );
    let c = AppInstance::new(
        "Same".into(),
        AppSource::Msix { package_family: "Same_x".into(), full_name: "Same_1.0.0.0_x64__x".into() },
        None,
    );
    assert_eq!(merge_instances(vec![a, b, c]).len(), 3);
}
