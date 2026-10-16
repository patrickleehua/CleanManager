//! Pure decisions of the uninstall flow (UNI-01..10); the platform and end-to-end parts are tested in cm-platform/cm-cli.

use super::*;

fn rec(name: &str, source: RecordSource) -> UninstallRecord {
    UninstallRecord {
        instance_id: "app_test".into(), name: name.into(), version: Some("1.0".into()), publisher: Some("Contoso Ltd.".into()), source, key: None,
        key_name: Some(name.into()), product_code: None, package_full_name: None, package_family: None, uninstall_string: None, quiet_uninstall_string: None,
        install_location: Some(r"C:\Program Files\Contoso\Foo".into()), system_component: false, winget: None, package_name: None, ai_tool: None, protected_class: None,
    }
}

fn env() -> AdapterEnv {
    AdapterEnv { system_dir: r"C:\Windows\System32".into(), ..Default::default() }
}

fn exists(_: &str) -> bool {
    false
}

fn identity(name: &str) -> AppIdentity {
    AppIdentity::of(&rec(name, RecordSource::RegistryUninstall))
}

#[test]
fn registered_command_is_taken_from_the_record_verbatim_with_publisher_binding() {
    let mut r = rec("Foo", RecordSource::RegistryUninstall);
    r.uninstall_string = Some(r#""C:\Program Files\Contoso\Foo\uninst.exe" /uninstall /log="x y.txt""#.into());
    r.quiet_uninstall_string = Some(r#""C:\Program Files\Contoso\Foo\uninst.exe" /S"#.into());
    let c = select_adapter(&r, &env(), &exists).unwrap();
    assert_eq!(c.kind, AdapterKind::RegisteredCommand);
    assert_eq!(c.program, r"C:\Program Files\Contoso\Foo\uninst.exe");
    assert_eq!(c.argv, vec![r#"/uninstall /log="x y.txt""#.to_owned()], "registered tail, verbatim; the quiet command is never used");
    assert!(c.raw_args && c.keeps_own_ui);
    assert_eq!(c.publisher_required.as_deref(), Some("Contoso Ltd."));
    // Unquoted program with spaces: the existing prefix wins, as CreateProcess would.
    r.uninstall_string = Some(r"C:\Program Files\Contoso\Foo\uninst.exe /x".into());
    let c = select_adapter(&r, &env(), &|p: &str| p.ends_with("uninst.exe")).unwrap();
    assert_eq!((c.program.as_str(), c.argv.as_slice()), (r"C:\Program Files\Contoso\Foo\uninst.exe", &["/x".to_owned()][..]));
}

#[test]
fn msi_products_use_system_msiexec_with_x_never_the_registered_modify_command() {
    let mut r = rec("Foo", RecordSource::Msi);
    r.product_code = Some("{11111111-2222-3333-4444-555555555555}".into());
    r.uninstall_string = Some(r"MsiExec.exe /I{11111111-2222-3333-4444-555555555555}".into());
    let c = select_adapter(&r, &env(), &exists).unwrap();
    assert_eq!((c.kind, c.program.as_str()), (AdapterKind::Msi, r"C:\Windows\System32\msiexec.exe"));
    assert_eq!(c.argv, vec!["/x".to_owned(), "{11111111-2222-3333-4444-555555555555}".into()]);
    assert!(c.microsoft_signed && !c.raw_args);
    // A non-MSI registration whose command is msiexec still maps to /x, never /i.
    let mut r = rec("Bar", RecordSource::RegistryUninstall);
    r.uninstall_string = Some(r"MsiExec.exe /I{AAAAAAAA-2222-3333-4444-555555555555}".into());
    assert_eq!(select_adapter(&r, &env(), &exists).unwrap().argv[0], "/x");
}

#[test]
fn package_managers_uninstall_their_own_packages() {
    // UNI-01: winget evidence on the registration → winget, not the vendor command.
    let mut r = rec("Foo", RecordSource::RegistryUninstall);
    r.uninstall_string = Some(r#""C:\x\uninst.exe""#.into());
    r.winget = Some(WingetEvidence { package_id: "Contoso.Foo".into(), source: Some("winget".into()) });
    let mut e = env();
    assert_eq!(select_adapter(&r, &e, &exists).unwrap_err().code, "unsupported", "winget missing: no fallback to deleting files");
    e.winget_exe = Some(r"C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller_x\winget.exe".into());
    let c = select_adapter(&r, &e, &exists).unwrap();
    assert!(c.kind.package_manager());
    assert_eq!(c.argv, ["uninstall", "--id", "Contoso.Foo", "--exact", "--source", "winget"]);
    // Scoop: its own script through Windows PowerShell, fixed argv.
    let mut s = rec("ripgrep", RecordSource::Scoop);
    s.package_name = Some("ripgrep".into());
    e.scoop_root = Some(r"C:\Users\u\scoop".into());
    let c = select_adapter(&s, &e, &exists).unwrap();
    assert_eq!(c.kind, AdapterKind::Scoop);
    assert_eq!(&c.argv[5..], [r"C:\Users\u\scoop\apps\scoop\current\bin\scoop.ps1", "uninstall", "ripgrep"]);
    s.package_name = Some("-rf".into());
    assert!(select_adapter(&s, &e, &exists).is_err(), "never an option-looking name");
    // npm-global AI CLI.
    let mut n = rec("Claude Code", RecordSource::NpmGlobal);
    n.package_name = Some("@anthropic-ai/claude-code".into());
    e.node = Some((r"C:\Program Files\nodejs\node.exe".into(), r"C:\Program Files\nodejs\node_modules\npm\bin\npm-cli.js".into()));
    let c = select_adapter(&n, &e, &exists).unwrap();
    assert_eq!(&c.argv[1..], ["uninstall", "--global", "@anthropic-ai/claude-code"]);
    // MSIX through the package manager API, bound to the manifest.
    let mut m = rec("Foo", RecordSource::Msix);
    m.package_full_name = Some("Contoso.Foo_1.0.0.0_x64__abc".into());
    assert_eq!(select_adapter(&m, &e, &exists).unwrap_err().settings_uri, Some(SETTINGS_URI));
    e.msix_manifest = Some(r"C:\Program Files\WindowsApps\Contoso.Foo\AppxManifest.xml".into());
    assert_eq!(select_adapter(&m, &e, &exists).unwrap().argv, ["remove", "Contoso.Foo_1.0.0.0_x64__abc"]);
}

#[test]
fn unknown_or_unverifiable_sources_explain_and_point_to_settings() {
    let r = rec("Foo", RecordSource::RegistryUninstall);
    let e = select_adapter(&r, &env(), &exists).unwrap_err();
    assert_eq!((e.code, e.settings_uri), ("unsupported", Some(SETTINGS_URI)));
    assert!(e.message.contains("never deletes the install folder"));
    for cmd in [r"uninst.exe /S", r#"cmd.exe /c "C:\x\remove.bat""#, r#""C:\x\remove.bat""#, r#"rundll32.exe C:\x\y.dll,Uninstall"#] {
        let mut r = rec("Foo", RecordSource::RegistryUninstall);
        r.uninstall_string = Some(cmd.into());
        assert_eq!(select_adapter(&r, &env(), &exists).unwrap_err().code, "unsupported", "{cmd}");
    }
    let mut r = rec("Foo", RecordSource::RegistryUninstall);
    r.uninstall_string = Some(r#""C:\x\uninst.exe""#.into());
    r.publisher = None;
    assert_eq!(select_adapter(&r, &env(), &exists).unwrap_err().code, "policy_denied", "no publisher, no signature match");
}

#[test]
fn protected_classes_have_no_uninstall_entry() {
    // UNI-10: security agents point to the official route, not to Settings.
    let mut r = rec("CrowdStrike Falcon Sensor", RecordSource::RegistryUninstall);
    r.uninstall_string = Some(r#""C:\x\uninst.exe""#.into());
    r.protected_class = Some(ProtectedReason::EndpointSecurityOrManagement);
    let e = select_adapter(&r, &env(), &exists).unwrap_err();
    assert_eq!((e.code, e.settings_uri), ("policy_denied", None));
    assert!(e.message.contains("administrator") && e.message.contains("vendor"));
    r.protected_class = Some(ProtectedReason::SystemComponent);
    assert_eq!(select_adapter(&r, &env(), &exists).unwrap_err().code, "policy_denied");
    let mut me = rec("CleanManager", RecordSource::RegistryUninstall);
    me.uninstall_string = Some(r#""C:\x\uninst.exe""#.into());
    assert_eq!(select_adapter(&me, &env(), &exists).unwrap_err().code, "policy_denied", "never itself");
}

#[test]
fn publisher_match_ignores_legal_suffixes_but_not_other_companies() {
    assert!(publisher_matches("Mozilla Corporation", "Mozilla"));
    assert!(publisher_matches("Google LLC", "Google Inc."));
    assert!(publisher_matches("Contoso Ltd", "Contoso Ltd."));
    assert!(!publisher_matches("Evil Corp", "Contoso Ltd."));
    assert!(!publisher_matches("Contosoft", "Contoso"));
    assert!(!publisher_matches("Inc.", "Ltd."), "suffixes alone never match");
}

#[test]
fn exit_codes_are_recorded_and_the_rescan_decides_completeness() {
    // UNI-03: success but the folder is still there → partial, leftovers go to the second plan.
    let gone = Rescan { install_dir_present: Some(false), registration_present: Some(false) };
    let dir = Rescan { install_dir_present: Some(true), registration_present: Some(false) };
    let (s, code, msg) = uninstaller_outcome(AdapterKind::RegisteredCommand, Some(0), 1234, dir);
    assert_eq!((s, code), (StepStatus::Succeeded, "leftovers_remain"));
    assert!(msg.contains("code 0") && msg.contains("1234 ms") && msg.contains("install folder"), "{msg}");
    assert_eq!(overall_status("completed", &["done", code]), "partial");
    assert_eq!(uninstaller_outcome(AdapterKind::RegisteredCommand, Some(0), 1, gone).1, "done");
    assert_eq!(overall_status("completed", &["done", "done"]), "completed");
    assert_eq!(uninstaller_outcome(AdapterKind::RegisteredCommand, Some(0), 1, Rescan::default()).1, "leftovers_remain", "unverifiable is not gone");
    assert_eq!(uninstaller_outcome(AdapterKind::Msi, Some(1602), 1, dir).0, StepStatus::Skipped);
    assert_eq!(uninstaller_outcome(AdapterKind::Msi, Some(3010), 1, gone).1, "restart_required");
    assert_eq!(uninstaller_outcome(AdapterKind::RegisteredCommand, Some(2), 1, gone).0, StepStatus::Failed);
    assert_eq!(uninstaller_outcome(AdapterKind::RegisteredCommand, None, 1, gone).0, StepStatus::Failed);
}

#[test]
fn critical_processes_and_this_tool_are_never_ended() {
    let p = |pid, name: &str, path: Option<&str>| RunningProcess { pid, name: name.into(), path: path.map(str::to_owned) };
    let root = r"C:\Windows";
    assert!(end_refusal(&p(10, "foo.exe", Some(r"C:\Program Files\Foo\foo.exe")), 99, root).is_none());
    assert!(end_refusal(&p(99, "foo.exe", Some(r"C:\Program Files\Foo\foo.exe")), 99, root).is_some(), "own pid");
    assert!(end_refusal(&p(10, "CleanManager.exe", Some(r"C:\x\CleanManager.exe")), 99, root).is_some());
    assert!(end_refusal(&p(10, "explorer.exe", Some(r"C:\Windows\explorer.exe")), 99, root).is_some());
    assert!(end_refusal(&p(10, "helper.exe", Some(r"C:\Windows\System32\helper.exe")), 99, root).is_some());
    assert!(end_refusal(&p(10, "foo.exe", None), 99, root).is_some(), "unknown image");
    assert!(end_refusal(&p(4, "foo.exe", Some(r"C:\x\foo.exe")), 99, root).is_some());
}

#[test]
fn generic_words_never_match_and_variants_are_exact() {
    let id = identity("Foo Bar 2.1 (x64)");
    assert_eq!(name_variants(&id.name), ["foo bar", "foo-bar", "foo_bar", "foobar"]);
    assert!(match_name(&id, "FooBar", false).is_some());
    assert!(match_name(&id, "Foo", false).is_none(), "a word of the name is not the name");
    assert!(match_name(&id, "FooBarBaz", false).is_none(), "no substring matches");
    for generic in ["Update", "Common Files", "Microsoft", "Cache", "Logs", "Packages", "ab"] {
        assert!(match_name(&identity(generic), generic, false).is_none(), "{generic}");
        assert!(name_variants(generic).is_empty(), "{generic}");
    }
    assert_eq!(publisher_variants("Contoso Ltd."), ["contoso", "contoso ltd"]);
    assert!(publisher_variants("Microsoft Corporation").is_empty(), "a generic publisher is never a folder match");
}

#[test]
fn a_beta_sharing_the_data_folder_keeps_it() {
    // UNI-04 scenario: uninstalling "Foo" while "Foo Beta" is installed: its folder is shared_owner and kept.
    let id = identity("Foo");
    let beta = InstalledApp { instance_id: "app_beta".into(), name: "Foo Beta".into(), publisher: None, install_location: Some(r"C:\B".into()), key_name: None };
    let m = match_name(&id, "Foo", false).unwrap();
    let (marker, why) = ownership_marker(&id, r"C:\Users\u\AppData\Roaming\Foo", &m, std::slice::from_ref(&beta)).unwrap();
    assert_eq!(marker, Marker::SharedOwner, "{why}");
    // Uninstalling the beta alone: the plain "Foo" folder matches only by family → owner unknown, kept.
    let id_beta = identity("Foo Beta");
    let m = match_name(&id_beta, "Foo", false).unwrap();
    assert!(m.family_only);
    assert_eq!(ownership_marker(&id_beta, r"C:\R\Foo", &m, &[]).unwrap().0, Marker::OwnerUnknown);
    // Reinstalled: everything is kept.
    let again = InstalledApp { instance_id: id.instance_id.clone(), name: "Foo".into(), publisher: None, install_location: None, key_name: None };
    let m = match_name(&id, "Foo", false).unwrap();
    assert_eq!(ownership_marker(&id, r"C:\R\Foo", &m, &[again]).unwrap().0, Marker::AppReappeared);
    assert_eq!(ownership_marker(&id, r"C:\R\Foo", &m, &[]).unwrap().0, Marker::OwnerUnknown, "a three-letter top-level name alone is weak");
    let m = match_name(&id, "Foo", true).unwrap();
    assert!(ownership_marker(&id, r"C:\R\Contoso\Foo", &m, &[]).is_none(), "publisher\\product is identity evidence");
}

fn leftover(display: &str, group: Group) -> Leftover {
    finish_leftover(Leftover {
        object_id: object_id(LeftoverKind::Directory, display), kind: LeftoverKind::Directory, group, display: display.into(), evidence: vec![], marker: None,
        tier: String::new(), selected: false, warnings: vec![], logical_bytes: Some(10), note: None, requires_admin: false, binding: Binding::Path { path: display.into() },
    })
}

#[test]
fn only_caches_and_logs_are_preselected_and_models_are_not() {
    // UNI-05 scenario: models under the app's data folder are personal data, unselected, with a re-download note.
    assert_eq!(classify_child("models", true), Group::PersonalData);
    let m = leftover(r"C:\R\Foo\models", classify_child("models", true));
    assert!(!m.selected && m.tier == "review_only" && m.note.as_deref().unwrap().contains("re-download"));
    assert!(leftover(r"C:\R\Foo\Cache", classify_child("Cache", true)).selected);
    assert!(leftover(r"C:\R\Foo\logs", classify_child("logs", true)).selected);
    assert!(!leftover(r"C:\R\Foo\settings.json", classify_child("settings.json", false)).selected);
    assert!(!leftover(r"C:\R\Foo\unknown-thing", classify_child("unknown-thing", true)).selected);
    // Sensitive names warn and are never preselected, even in a cache-looking place.
    let s = leftover(r"C:\R\Foo\Cookies", Group::CachesLogs);
    assert!(!s.selected && !s.warnings.is_empty());
    let mut kept = leftover(r"C:\R\Foo\Cache", Group::CachesLogs);
    kept.marker = Some(Marker::SharedOwner);
    let kept = finish_leftover(kept);
    assert_eq!((kept.tier.as_str(), kept.selected), ("kept", false));
}

#[test]
fn registry_leftovers_are_limited_to_the_apps_own_keys() {
    assert!(!registry_denied(r"Software\Contoso\Foo"));
    assert!(!registry_denied(r"SOFTWARE\WOW6432Node\Foo"));
    assert!(!registry_denied(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Foo"));
    assert!(!registry_denied(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\foo.exe"));
    for denied in [r"Software\Microsoft\Foo", r"Software\Classes\Foo", r"Software\Policies\Foo", r"Software", r"SYSTEM\CurrentControlSet\Services\Foo", r"Software\A\B\C",
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run"] {
        assert!(registry_denied(denied), "{denied}");
    }
}

#[test]
fn ai_cli_sessions_and_settings_are_separate_and_unselected() {
    // UNI-07: caches selected; sessions and settings listed apart and need explicit selection.
    let claude = ai_cli("claude-code").unwrap();
    assert_eq!(ai_child_group(claude, "projects", true), Group::PersonalData);
    assert_eq!(ai_child_group(claude, "settings.json", false), Group::Settings);
    assert_eq!(ai_child_group(claude, "statsig", true), Group::CachesLogs);
    assert!(leftover(r"C:\U\.claude\statsig", ai_child_group(claude, "statsig", true)).selected);
    assert!(!leftover(r"C:\U\.claude\projects", ai_child_group(claude, "projects", true)).selected);
    let creds = leftover(r"C:\U\.codex\auth.json", ai_child_group(ai_cli("codex").unwrap(), "auth.json", false));
    assert!(!creds.selected && !creds.warnings.is_empty());
    assert!(AI_CLIS.iter().all(|t| safe_package_name(t.npm_package)));
}

#[test]
fn orphans_are_review_only_and_explain_the_missing_target() {
    // UNI-08 scenario: a startup entry pointing at a deleted program.
    let r = BrokenReference { startup_item_id: "st_x".into(), name: "Foo Tray".into(), missing_target: r"C:\Users\u\AppData\Local\Programs\FooApp\foo.exe".into() };
    let old = DirEntryInfo { path: r"C:\R\FooApp".into(), name: "FooApp".into(), is_dir: true, logical_bytes: Some(5), idle_seconds: Some(40 * 86_400) };
    let fresh = DirEntryInfo { path: r"C:\L\FooApp".into(), name: "FooApp".into(), is_dir: true, logical_bytes: Some(5), idle_seconds: Some(60) };
    let out = orphan_leftovers(std::slice::from_ref(&r), &[old.clone(), fresh], &[]);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|l| !l.selected), "never preselected");
    assert!(out[0].display.contains(&r.missing_target) && out[0].tier == "review_only");
    assert_eq!(out[1].tier, "review_only");
    assert_eq!((out[2].tier.as_str(), out[2].marker), ("kept", Some(Marker::OwnerUnknown)), "recently used: not an orphan");
    // Still claimed by an installed app: no data folders.
    let installed = InstalledApp { instance_id: "a".into(), name: "FooApp".into(), publisher: None, install_location: None, key_name: None };
    assert_eq!(orphan_leftovers(&[r], &[old], &[installed]).len(), 1);
    assert_eq!(app_folder_of(r"C:\Program Files\Common Files\x.exe"), None, "generic folder");
}

#[test]
fn record_digest_changes_with_the_command() {
    let mut a = rec("Foo", RecordSource::RegistryUninstall);
    a.uninstall_string = Some(r#""C:\x\u.exe""#.into());
    let mut b = a.clone();
    b.uninstall_string = Some(r#""C:\x\evil.exe""#.into());
    assert_ne!(a.digest(), b.digest());
    let c = select_adapter(&a, &env(), &exists);
    assert!(c.is_ok() || c.is_err());
    assert!(tool_instance_id(RecordSource::Scoop, "x").starts_with("app_"));
    assert_ne!(tool_instance_id(RecordSource::Scoop, "x"), tool_instance_id(RecordSource::NpmGlobal, "x"));
}

#[test]
fn macos_adapter_ids_round_trip_and_macos_records_never_reach_windows_adapters() {
    for (k, id) in [
        (AdapterKind::MacTrashAppBundle, "macos.trash-app-bundle"),
        (AdapterKind::HomebrewCask, "homebrew.uninstall-cask"),
        (AdapterKind::HomebrewFormula, "homebrew.uninstall-formula"),
    ] {
        assert_eq!((k.adapter_id(), AdapterKind::from_adapter_id(id)), (id, Some(k)));
    }
    assert!(AdapterKind::HomebrewCask.package_manager() && !AdapterKind::MacTrashAppBundle.package_manager());
    for source in [RecordSource::AppBundle, RecordSource::Homebrew, RecordSource::AppStore] {
        let rec: UninstallRecord = serde_json::from_value(serde_json::json!({
            "instance_id": "app_x", "name": "Editor", "source": source, "system_component": false,
            "uninstall_string": r"C:\Tools\unins000.exe"})).expect("record shape");
        let err = select_adapter(&rec, &AdapterEnv::default(), &|_| true).unwrap_err();
        assert_eq!(err.code, "unsupported");
        assert!(["app_bundle", "homebrew", "app_store"].contains(&rec.wire_source()));
    }
}
