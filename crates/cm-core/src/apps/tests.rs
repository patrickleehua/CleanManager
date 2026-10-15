use super::*;

fn win(p: &str) -> RawPath {
    RawPath::windows(p.encode_utf16().collect()).unwrap()
}

fn ident(file: &str) -> LocationIdentity {
    LocationIdentity { volume_id: "\\\\?\\Volume{0000}\\|00000000000000aa".into(), file_id: format!("fileid128:{file:0>32}") }
}

fn reg(key: &str) -> AppSource {
    AppSource::RegistryUninstall { hive: RegistryHive::Hklm, view: RegistryView::View64, key: key.into() }
}

fn app(name: &str, source: AppSource, loc: Option<&str>, id: Option<&str>) -> AppInstance {
    let mut a = AppInstance::new(name.into(), source, loc.map(win));
    a.location_identity = id.map(ident);
    a
}

fn ts() -> Timestamp {
    Timestamp("2026-10-15T00:00:00Z".into())
}

#[test]
fn instance_id_is_stable_and_depends_on_source_and_location_not_name() {
    let a = app("Tool", reg("Tool"), Some("C:\\A"), None);
    let b = app("Renamed Tool", reg("Tool"), Some("C:\\A"), None);
    assert_eq!(a.instance_id, b.instance_id, "display name is not identity");
    assert_eq!(a.instance_id, instance_id_of(&reg("Tool"), Some(&win("C:\\A"))));
    assert!(a.instance_id.starts_with("app_") && a.instance_id.len() == 36);
    assert_ne!(a.instance_id, app("Tool", reg("Tool"), Some("C:\\B"), None).instance_id);
    let view32 = AppSource::RegistryUninstall { hive: RegistryHive::Hklm, view: RegistryView::View32, key: "Tool".into() };
    assert_ne!(a.instance_id, app("Tool", view32, Some("C:\\A"), None).instance_id);
    let user = AppSource::Msi { hive: RegistryHive::Hkcu, view: RegistryView::Shared, product_code: "{A}".into() };
    let machine = AppSource::Msi { hive: RegistryHive::Hklm, view: RegistryView::View64, product_code: "{A}".into() };
    assert_ne!(instance_id_of(&user, None), instance_id_of(&machine, None), "per-user and per-machine MSI stay separate");
}

#[test]
fn same_display_name_with_different_location_or_source_stays_separate() {
    // APP-01: two installations share a display name.
    let a = app("Editor", reg("Editor"), Some("C:\\Program Files\\Editor"), Some("1"));
    let b = app("Editor", reg("Editor"), Some("D:\\Portable\\Editor"), Some("2"));
    let c = app("Editor", AppSource::Msix { package_family: "Editor_abc".into(), full_name: "Editor_1.0.0.0_x64__abc".into() }, None, None);
    let merged = merge_instances(vec![a.clone(), b.clone(), c.clone()]);
    assert_eq!(merged.len(), 3);
    let ids: std::collections::BTreeSet<_> = merged.iter().map(|m| m.instance_id.clone()).collect();
    assert_eq!(ids.len(), 3);
    assert!(merged.iter().all(|m| m.name == "Editor"));
}

#[test]
fn exact_duplicates_merge_without_losing_evidence_or_protection() {
    let mut a = app("Agent", reg("Agent"), Some("C:\\Agent"), None);
    a.evidence.push(Evidence::new(EvidenceKind::RegistryKey, "k"));
    let mut b = a.clone();
    b.version = Some("2.0".into());
    b.location_identity = Some(ident("9"));
    b.protected_class = Some(ProtectedReason::EndpointSecurityOrManagement);
    b.evidence.push(Evidence::new(EvidenceKind::SystemComponent, "1"));
    let merged = merge_instances(vec![a, b]);
    assert_eq!(merged.len(), 1);
    let m = &merged[0];
    assert_eq!(m.version.as_deref(), Some("2.0"));
    assert_eq!(m.location_identity, Some(ident("9")));
    assert_eq!(m.protected_class, Some(ProtectedReason::EndpointSecurityOrManagement));
    assert_eq!(m.evidence.len(), 2, "evidence is unioned, not duplicated");
}

#[test]
fn uninstall_is_unsupported_with_reason_in_first_release() {
    let a = app("X", reg("X"), None, None);
    assert_eq!(a.uninstall_capability.state, CapabilityState::Unsupported);
    assert!(a.uninstall_capability.reason.as_deref().is_some_and(|r| r.contains("Read-only")));
    let w = a.to_wire();
    for key in ["app_instance_id", "name", "version", "source", "location", "publisher", "identity_evidence", "uninstall_capability", "update_status", "protected_class"] {
        assert!(w.get(key).is_some(), "missing {key}");
    }
    assert_eq!(w["uninstall_capability"]["state"], "unsupported");
    assert_eq!(w["source"], "registry_uninstall");
    assert_eq!(w["version"], serde_json::Value::Null, "unknown stays null");
    assert_eq!(w["update_status"], "not_checked");
    assert_eq!(w.as_object().unwrap().len(), 10, "no fields beyond the schema");
}

#[test]
fn protected_classes_cover_system_runtimes_and_security_agents() {
    assert_eq!(protected_reason("Anything", None, true), Some(ProtectedReason::SystemComponent));
    assert_eq!(protected_reason("Microsoft Visual C++ 2015-2022 Redistributable (x64)", None, false), Some(ProtectedReason::SharedRuntime));
    assert_eq!(protected_reason("Microsoft Edge WebView2 Runtime", None, false), Some(ProtectedReason::SharedRuntime));
    assert_eq!(protected_reason("Microsoft .NET Runtime - 8.0.10 (x64)", None, false), Some(ProtectedReason::SharedRuntime));
    assert_eq!(protected_reason("CrowdStrike Windows Sensor", None, false), Some(ProtectedReason::EndpointSecurityOrManagement));
    assert_eq!(protected_reason("Sensor", Some("SentinelOne, Inc."), false), Some(ProtectedReason::EndpointSecurityOrManagement));
    assert_eq!(protected_reason("Microsoft Intune Management Extension", None, false), Some(ProtectedReason::EndpointSecurityOrManagement));
    assert_eq!(protected_reason("7-Zip 24.08 (x64)", Some("Igor Pavlov"), false), None);
}

#[test]
fn clean_text_escapes_controls_and_treats_blank_as_unknown() {
    assert_eq!(clean_text("  App\u{1b}[2J\0 "), Some("App\\u{1b}[2J".into()));
    assert_eq!(clean_text(" \0 "), None);
}

#[test]
fn shared_location_is_marked_and_never_exclusive() {
    // APP-02: two applications use the same container (same identity, different spelling).
    let a = app("A", reg("A"), Some("C:\\Shared\\Runtime"), Some("5"));
    let b = app("B", reg("B"), Some("C:\\SHARED\\RUNTI~1"), Some("5"));
    let c = app("C", reg("C"), Some("C:\\Apps\\C"), Some("6"));
    let report = associate(&[a.clone(), b.clone(), c.clone()], &ts());
    let by_id = |id: &str| report.associations.iter().find(|x| x.instance_id == id).unwrap();
    assert_eq!(by_id(&a.instance_id).shared_with, vec![b.instance_id.clone()]);
    assert_eq!(by_id(&b.instance_id).shared_with, vec![a.instance_id.clone()]);
    assert!(!by_id(&a.instance_id).is_exclusive() && !by_id(&b.instance_id).is_exclusive());
    assert!(by_id(&c.instance_id).is_exclusive());
    assert_eq!(by_id(&c.instance_id).observed_at, ts());
}

#[test]
fn same_path_without_identity_and_nested_paths_are_shared() {
    let outer = app("Suite", reg("Suite"), Some("C:\\Program Files\\Vendor"), Some("1"));
    let inner = app("Plugin", reg("Plugin"), Some("c:/program files/vendor/plugin/"), Some("2"));
    let sibling = app("Other", reg("Other"), Some("C:\\Program Files\\VendorX"), Some("3"));
    let unresolved_a = app("U1", reg("U1"), Some("D:\\Gone"), None);
    let unresolved_b = app("U2", reg("U2"), Some("d:\\gone\\"), None);
    let r = associate(&[outer.clone(), inner.clone(), sibling.clone(), unresolved_a.clone(), unresolved_b.clone()], &ts());
    let get = |i: &AppInstance| r.associations.iter().find(|x| x.instance_id == i.instance_id).unwrap().clone();
    assert_eq!(get(&outer).shared_with, vec![inner.instance_id.clone()]);
    assert_eq!(get(&inner).shared_with, vec![outer.instance_id.clone()]);
    assert!(get(&sibling).is_exclusive(), "a name prefix is not nesting");
    assert_eq!(get(&unresolved_a).shared_with, vec![unresolved_b.instance_id.clone()]);
    assert_eq!(get(&unresolved_a).coverage, Coverage::Unknown { reason: CoverageGap::LocationUnresolved });
    assert!(!get(&unresolved_a).is_exclusive());
}

#[test]
fn instances_without_location_are_a_coverage_gap() {
    let a = app("NoLoc", reg("NoLoc"), None, None);
    let r = associate(std::slice::from_ref(&a), &ts());
    assert!(r.associations.is_empty());
    assert_eq!(r.without_location, vec![a.instance_id]);
}

#[test]
fn exclusive_space_never_double_counts_shared_or_nested_locations() {
    let a = app("A", reg("A"), Some("C:\\Shared"), Some("5"));
    let b = app("B", reg("B"), Some("C:\\Shared"), Some("5"));
    let nested = app("N", reg("N"), Some("C:\\Shared\\Sub"), Some("7"));
    let c = app("C", reg("C"), Some("C:\\C"), Some("6"));
    let d = app("D", reg("D"), Some("C:\\D"), Some("8"));
    let gone = app("G", reg("G"), Some("C:\\Gone"), None);
    let r = associate(&[a.clone(), b.clone(), nested, c.clone(), d.clone(), gone], &ts());
    let size = |x: &Association| match x.path.lossy().as_str() {
        "C:\\Shared" => Some(Bytes(1_000)),
        "C:\\Shared\\Sub" => Some(Bytes(400)),
        "C:\\C" => Some(Bytes(50)),
        "C:\\Gone" => Some(Bytes(999_999)),
        _ => None,
    };
    let s = exclusive_space(&r.associations, size).unwrap();
    assert_eq!(s.shared.known, Bytes(1_000), "shared container counted once, nested content inside it");
    assert_eq!(s.shared_locations, 1);
    assert!(!s.exclusive.contains_key(&a.instance_id) && !s.exclusive.contains_key(&b.instance_id));
    assert_eq!(s.exclusive[&c.instance_id].exact(), Some(Bytes(50)));
    assert_eq!(s.exclusive[&d.instance_id].unknown_count, 1, "unknown size is not zero");
    assert_eq!(s.unresolved_locations, 1, "unresolved locations are never attributed");
    let total: u64 = s.exclusive.values().map(|t| t.known.0).sum::<u64>() + s.shared.known.0;
    assert_eq!(total, 1_050);
}

#[test]
fn missing_installation_record_is_never_residue() {
    // APP-03: a data folder has no matching installation record.
    let installed = app("Contoso Editor", reg("Contoso"), Some("C:\\Program Files\\Contoso Editor"), Some("1"));
    let r = associate(std::slice::from_ref(&installed), &ts());
    let cases = [
        (win("C:\\Users\\u\\AppData\\Local\\Fabrikam"), InstallRecordStatus::NotFound, UnknownOwnership::NoInstallationRecord),
        (win("C:\\Users\\u\\AppData\\Local\\Contoso Editor"), InstallRecordStatus::NotFound, UnknownOwnership::NameResemblanceOnly),
        (win("C:\\Users\\u\\AppData\\Local\\Old"), InstallRecordStatus::ConfirmedUninstalled, UnknownOwnership::UninstalledOwnershipUnproven),
        (win("C:\\Users\\u\\AppData\\Local\\Contoso Editor"), InstallRecordStatus::SourceUnsupported, UnknownOwnership::SourceUnsupported),
    ];
    for (path, record, expected) in cases {
        let v = assess_directory(&path, Some(&ident("ff")), record, std::slice::from_ref(&installed), &r.associations);
        match &v.ownership {
            Ownership::Unknown { reason, detail } => {
                assert_eq!(*reason, expected, "{}", path.display());
                assert!(!detail.is_empty());
            }
            other => panic!("expected unknown, got {other:?}"),
        }
        assert!(!v.cleanup_candidate() && !v.auto_select());
    }
}

#[test]
fn associated_directories_are_explained_but_still_not_candidates() {
    let a = app("A", reg("A"), Some("C:\\Shared"), Some("5"));
    let b = app("B", reg("B"), Some("C:\\Shared"), Some("5"));
    let r = associate(&[a.clone(), b.clone()], &ts());
    let inside = assess_directory(&win("C:\\Shared\\cache"), None, InstallRecordStatus::NotFound, &[a.clone(), b.clone()], &r.associations);
    let mut ids = vec![a.instance_id.clone(), b.instance_id.clone()];
    ids.sort();
    assert_eq!(inside.ownership, Ownership::Associated { instance_ids: ids, shared: true });
    assert!(!inside.cleanup_candidate());
    let by_identity = assess_directory(&win("\\\\?\\C:\\elsewhere"), Some(&ident("5")), InstallRecordStatus::NotFound, &[a], &r.associations);
    assert!(matches!(by_identity.ownership, Ownership::Associated { .. }));
    let json = serde_json::to_value(&by_identity).unwrap();
    assert_eq!(json["cleanup_candidate"], false);
}
