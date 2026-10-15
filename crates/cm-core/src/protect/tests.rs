use super::*;

fn win(s: &str) -> RawPath {
    RawPath::windows(s.encode_utf16().collect()).unwrap()
}

fn unix(s: &str) -> RawPath {
    RawPath::unix(s.as_bytes().to_vec()).unwrap()
}

fn id(s: &str) -> ObjectIdentity {
    ObjectIdentity::new("vol-c", s)
}

fn root(class: HardDenyClass, file: &str) -> HardDenyRoot {
    HardDenyRoot { class, volume_id: "vol-c".into(), file_id: file.into(), display: file.into() }
}

fn hard() -> HardDenySet {
    HardDenySet::new([
        root(HardDenyClass::SystemRoot, "windows"),
        root(HardDenyClass::UserProfileRoot, "alice"),
        root(HardDenyClass::VolumeRoot, "c-root"),
    ])
}

fn chain(object: &str, ancestors: &[&str]) -> IdentityChain {
    IdentityChain { object: Some(id(object)), real_path: None, ancestors: ancestors.iter().map(|a| Some(id(a))).collect(), links: vec![] }
}

const T: &str = "2026-10-15T08:00:00Z";

// ---- textual pre-filter -------------------------------------------------------------------

#[test]
fn protect_prefilter_windows_denies_dangerous_shapes() {
    let cases: &[(&str, TextualDeny)] = &[
        ("C:", TextualDeny::Relative),
        ("C:foo", TextualDeny::Relative),
        ("foo\\bar", TextualDeny::Relative),
        ("\\Windows", TextualDeny::Relative),
        ("C:\\", TextualDeny::VolumeRoot),
        ("C:\\Users\\a\\..\\b", TextualDeny::Traversal),
        ("\\\\.\\PhysicalDrive0", TextualDeny::DevicePath),
        ("\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1", TextualDeny::DevicePath),
        ("\\??\\C:\\x", TextualDeny::DevicePath),
        ("\\\\server\\C$\\x", TextualDeny::AdminShare),
        ("\\\\server\\share", TextualDeny::ShareRoot),
        ("C:\\data\\file.txt:secret", TextualDeny::AlternateDataStream),
        ("C:\\data\\CON", TextualDeny::ReservedName),
        ("C:\\data\\com1.txt", TextualDeny::ReservedName),
        ("C:\\data\\x.", TextualDeny::NonCanonical("trailing dot or space in a component")),
        ("\\\\?\\C:/x", TextualDeny::NonCanonical("forward slash in a verbatim path")),
        ("C:\\data\\a\u{1b}b", TextualDeny::ControlCharacter),
        ("C:\\data\\\u{202e}gpj.exe", TextualDeny::FormatCharacter),
        ("C:\\Users\\alice", TextualDeny::ProtectedLocation("user profile root")),
        ("C:\\Windows\\System32", TextualDeny::ProtectedLocation("Windows system directory")),
        ("C:\\$MFT", TextualDeny::ProtectedLocation("NTFS metadata or system staging area")),
    ];
    for (p, want) in cases {
        assert_eq!(prefilter(&win(p)).as_ref(), Err(want), "{p}");
    }
    let mut lone: Vec<u16> = "C:\\data\\x".encode_utf16().collect();
    lone.push(0xD800);
    assert_eq!(prefilter(&RawPath::Windows(lone)), Err(TextualDeny::EncodingAnomaly));
}

#[test]
fn protect_prefilter_windows_allows_ordinary_paths() {
    for p in [
        "C:\\Users\\alice\\AppData\\Local\\Temp\\build",
        "c:/users/alice/appdata/local/npm-cache",
        "\\\\?\\C:\\Users\\alice\\AppData\\Local\\Temp\\x",
        "\\\\?\\Volume{12345678-aaaa-bbbb-cccc-123456789abc}\\cache\\x",
        "\\\\server\\share\\builds\\old",
        "D:\\cache\\gradle",
        "C:\\$Recycle.Bin\\S-1-5-21-1\\$RABC.txt",
        "C:\\Users\\alice\\AppData\\Local\\Temp\\console.log",
    ] {
        assert_eq!(prefilter(&win(p)), Ok(()), "{p}");
    }
}

#[test]
fn protect_prefilter_unix_denies_and_allows() {
    let deny = [
        "relative", "~/x", "/", "/System", "/system/library", "/usr/bin/env", "/usr/local", "/etc/hosts", "/Users/alice", "/Users/alice/Library",
        "/Users/alice/Library/Caches", "/Users/alice/Library/Keychains/login.keychain-db", "/Users/alice/x/../y", "/Users/alice//x", "/Volumes/Data",
        "/private/var/folders",
    ];
    for p in deny {
        assert!(prefilter(&unix(p)).is_err(), "{p}");
    }
    assert_eq!(prefilter(&RawPath::Unix(b"/Users/a/\xff".to_vec())), Err(TextualDeny::EncodingAnomaly));
    for p in ["/Users/alice/Library/Caches/com.example", "/private/var/folders/ab/cd/T/x", "/usr/local/Cellar/foo/1.0", "/Users/alice/.Trash/old.zip"] {
        assert_eq!(prefilter(&unix(p)), Ok(()), "{p}");
    }
}

// ---- identity checks ----------------------------------------------------------------------

#[test]
fn protect_identity_denies_roots_ancestors_and_failures() {
    let h = hard();
    assert!(check_identity(&chain("windows", &["c-root"]), &h).is_denied(), "root itself");
    assert!(check_identity(&chain("system32", &["windows", "c-root"]), &h).is_denied(), "inside subtree root");
    assert!(check_identity(&chain("alice", &["users", "c-root"]), &h).is_denied(), "exact root itself");
    assert!(check_identity(&chain("cache", &["local", "appdata", "alice", "users", "c-root"]), &h).is_allowed(), "exact roots do not cover descendants");
    assert!(check_identity(&IdentityChain::default(), &h).is_denied(), "unidentified object");
    let mut c = chain("cache", &["c-root"]);
    c.ancestors.push(None);
    assert!(check_identity(&c, &h).is_denied(), "unidentified ancestor");
}

#[test]
fn protect_links_can_only_deny() {
    let h = hard();
    let link_to = |target: Result<ResolvedTarget, String>| IdentityChain {
        object: Some(id("junction")),
        real_path: None,
        ancestors: vec![Some(id("temp")), Some(id("c-root"))],
        links: vec![LinkResolution { link: win("C:\\t\\junction"), on_object: true, target }],
    };
    let target = |obj: &str, anc: &[&str]| ResolvedTarget { path: win("C:\\Real"), object: id(obj), ancestors: anc.iter().map(|a| id(a)).collect() };
    let d = check_identity(&link_to(Ok(target("windows", &["c-root"]))), &h);
    assert!(matches!(&d, ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason } if reason.contains("resolves to")), "{d:?}");
    assert!(check_identity(&link_to(Ok(target("system32", &["windows", "c-root"]))), &h).is_denied());
    assert!(check_identity(&link_to(Err("access denied".into())), &h).is_denied(), "resolution failure denies");
    assert!(check_identity(&link_to(Ok(target("elsewhere", &["c-root"]))), &h).is_allowed(), "a harmless target stays allowed");
    // A link cannot rescue an object that is itself denied.
    let mut denied = link_to(Ok(target("elsewhere", &["c-root"])));
    denied.ancestors.push(Some(id("windows")));
    assert!(check_identity(&denied, &h).is_denied());
}

#[test]
fn protect_alias_cannot_weaken_a_subtree_root() {
    let mut h = HardDenySet::new([root(HardDenyClass::SystemRoot, "x")]);
    h.insert(root(HardDenyClass::UserProfileRoot, "x"));
    assert_eq!(h.len(), 1);
    assert!(h.covering(&id("x")).is_some());
    let mut h = HardDenySet::new([root(HardDenyClass::UserProfileRoot, "y")]);
    h.insert(root(HardDenyClass::SystemRoot, "y"));
    assert!(h.covering(&id("y")).is_some(), "the stronger class replaces the weaker one");
}

#[test]
fn protect_hard_deny_within_finds_contained_roots() {
    use crate::analysis::{EntryFacts, MemView};
    use crate::model::{CloudState, IdentityQuality, ObjectKind};
    let mut v = MemView { platform: Some(Platform::Windows), root: 1, ..Default::default() };
    let facts = |fid: &str| EntryFacts {
        kind: ObjectKind::Directory,
        boundary: false,
        object_id: 0,
        logical: None,
        allocated: None,
        volume_id: "vol-c".into(),
        file_id: fid.into(),
        quality: IdentityQuality::Strong,
        cloud: CloudState::NotCloud,
        modified_unix: None,
        link_count: None,
        skipped_here: 0,
    };
    for (e, parent, name) in [(1u64, None, "C:\\x"), (2, Some(1), "CleanManager"), (3, Some(2), "store"), (4, Some(1), "other")] {
        v.names.insert(e, win(name));
        v.parents.insert(e, parent);
        v.facts.insert(e, facts(name));
        v.identities.insert(("vol-c".into(), format!("f{e}")), e);
    }
    let h = HardDenySet::new([root(HardDenyClass::ToolStore, "f3")]);
    assert!(hard_deny_within(&v, 2, &h).is_some(), "parent of the store");
    assert!(hard_deny_within(&v, 3, &h).is_some(), "the store itself");
    assert!(hard_deny_within(&v, 4, &h).is_none(), "a sibling");
}

// ---- user list ----------------------------------------------------------------------------

fn sample_file() -> String {
    r#"{
  "schema_version": 1,
  "entries": [
    {"id": "keep-npm", "match": "descendants", "path": "C:\\Users\\alice\\AppData\\Local\\npm-cache", "added_at": "2026-10-15T08:00:00Z"},
    {"id": "keep-one", "match": "exact", "path": "D:\\cache\\keep.bin", "note": "manual", "added_at": "2026-10-15T08:00:00.125Z"},
    {"id": "keep-up", "match": "ancestors", "path": "D:\\projects\\a\\b", "added_at": "2026-10-15T08:00:00Z"},
    {"id": "keep-logs", "match": "glob", "glob": "D:\\logs\\**\\*.keep", "added_at": "2026-10-15T08:00:00Z"},
    {"id": "keep-rule", "match": "rule", "rule_id": "win.browser.cache", "added_at": "2026-10-15T08:00:00Z"},
    {"id": "keep-task", "match": "maintenance_task", "task_id": "refresh_icon_cache", "added_at": "2026-10-15T08:00:00Z"}
  ]
}"#
    .to_owned()
}

#[test]
fn protect_user_list_round_trips_and_matches() {
    let list = load_protection_file_for(&sample_file(), Platform::Windows).unwrap();
    assert_eq!(list.entries().len(), 6);
    let again = load_protection_file_for(&list.serialize(), Platform::Windows).unwrap();
    assert_eq!(again, list);

    let hit = |p: &str, rec: bool| list.match_path(&win(p), rec).map(|e| e.id.clone());
    // descendants
    assert_eq!(hit("C:\\Users\\alice\\AppData\\Local\\npm-cache", false).as_deref(), Some("keep-npm"));
    assert_eq!(hit("c:/users/ALICE/appdata/local/NPM-CACHE/_cacache/x", false).as_deref(), Some("keep-npm"));
    assert_eq!(hit("\\\\?\\C:\\Users\\alice\\AppData\\Local\\npm-cache\\x", false).as_deref(), Some("keep-npm"));
    assert_eq!(hit("C:\\Users\\alice\\AppData\\Local", false), None);
    assert_eq!(hit("C:\\Users\\alice\\AppData\\Local", true).as_deref(), Some("keep-npm"), "removing a container removes the protected subtree");
    // exact
    assert_eq!(hit("D:\\cache\\keep.bin", false).as_deref(), Some("keep-one"));
    assert_eq!(hit("D:\\cache\\keep.bin.old", false), None);
    assert_eq!(hit("D:\\cache", false), None);
    assert_eq!(hit("D:\\cache", true).as_deref(), Some("keep-one"));
    // ancestors
    assert_eq!(hit("D:\\projects\\a", false).as_deref(), Some("keep-up"));
    assert_eq!(hit("D:\\projects\\a\\b", false).as_deref(), Some("keep-up"));
    assert_eq!(hit("D:\\projects\\a\\b\\c", false), None);
    assert_eq!(hit("D:\\projects\\z", false), None);
    // glob: matches, their subtrees, and containers for recursive removals
    assert_eq!(hit("D:\\logs\\x.keep", false).as_deref(), Some("keep-logs"));
    assert_eq!(hit("D:\\logs\\a\\b\\X.KEEP", false).as_deref(), Some("keep-logs"));
    assert_eq!(hit("D:\\logs\\a\\b.keep\\inner", false).as_deref(), Some("keep-logs"));
    assert_eq!(hit("D:\\logs\\a\\b.log", false), None);
    assert_eq!(hit("D:\\logs\\a", false), None);
    assert_eq!(hit("D:\\logs\\a", true).as_deref(), Some("keep-logs"));
    assert_eq!(hit("E:\\logs\\x.keep", false), None);
    // rules and tasks
    assert_eq!(list.protects_rule("win.browser.cache").map(|e| e.id.as_str()), Some("keep-rule"));
    assert!(list.protects_rule("win.temp").is_none());
    assert!(evaluate_task("refresh_icon_cache", &list).is_denied());
    assert!(evaluate_task("flush_dns", &list).is_allowed());
}

#[test]
fn protect_unix_user_list_matches_case_insensitively() {
    let mut list = UserProtection::empty(Platform::Macos);
    list.add(UserEntry::path("a", MatchKind::Descendants, "/Users/alice/Library/Caches/com.keep", T)).unwrap();
    list.add(UserEntry::glob("b", "/Users/*/Library/Caches/*.precious", T)).unwrap();
    assert!(list.match_path(&unix("/Users/alice/Library/Caches/COM.KEEP/x"), false).is_some());
    assert!(list.match_path(&unix("/Users/bob/Library/Caches/x.precious"), false).is_some());
    assert!(list.match_path(&unix("/Users/bob/Library/Caches/x"), false).is_none());
}

#[test]
fn protect_corrupt_or_incompatible_files_are_unavailable() {
    let bad = [
        ("", "malformed"),
        ("{", "malformed"),
        ("[]", "malformed"),
        (r#"{"entries": []}"#, "version"),
        (r#"{"schema_version": 2, "entries": []}"#, "version"),
        (r#"{"schema_version": "1", "entries": []}"#, "version"),
        (r#"{"schema_version": 1}"#, "malformed"),
        (r#"{"schema_version": 1, "entries": [], "extra": true}"#, "malformed"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "path": "D:\\x", "added_at": "2026-10-15T08:00:00Z", "allow": true}]}"#, "malformed"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "everything", "added_at": "2026-10-15T08:00:00Z"}]}"#, "malformed"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "path": "relative\\x", "added_at": "2026-10-15T08:00:00Z"}]}"#, "entry"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "path": "D:\\x\\..\\..\\Windows", "added_at": "2026-10-15T08:00:00Z"}]}"#, "entry"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "descendants", "path": "C:\\Windows", "added_at": "2026-10-15T08:00:00Z"}]}"#, "entry"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "glob": "D:\\x", "added_at": "2026-10-15T08:00:00Z"}]}"#, "entry"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "path": "D:\\x", "added_at": "yesterday"}]}"#, "entry"),
        (r#"{"schema_version": 1, "entries": [{"id": "a", "match": "exact", "path": "D:\\x", "added_at": "2026-10-15T08:00:00Z"}, {"id": "a", "match": "exact", "path": "D:\\y", "added_at": "2026-10-15T08:00:00Z"}]}"#, "entry"),
    ];
    for (text, kind) in bad {
        let err = load_protection_file_for(text, Platform::Windows).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProtectionUnavailable);
        let ok = match kind {
            "malformed" => matches!(err, ProtectionUnavailable::Malformed(_)),
            "version" => matches!(err, ProtectionUnavailable::UnsupportedVersion { .. }),
            _ => matches!(err, ProtectionUnavailable::InvalidEntry { .. }),
        };
        assert!(ok, "{text} -> {err:?}");
    }
    assert!(load_protection_file_for(r#"{"schema_version": 1, "entries": []}"#, Platform::Windows).unwrap().entries().is_empty());
}

#[test]
fn protect_entry_validation_reports_argument_errors() {
    let mut list = UserProtection::empty(Platform::Windows);
    let reject = |list: &UserProtection, e: UserEntry| list.validate(&e).unwrap_err();
    assert!(matches!(reject(&list, UserEntry::path("a", MatchKind::Exact, "x\\y", T)), EntryError::PathRejected(TextualDeny::Relative)));
    assert!(matches!(reject(&list, UserEntry::path("a", MatchKind::Exact, "C:\\Users\\alice\\..\\bob", T)), EntryError::PathRejected(TextualDeny::Traversal)));
    assert!(matches!(reject(&list, UserEntry::path("a", MatchKind::Exact, "C:\\", T)), EntryError::PathRejected(TextualDeny::VolumeRoot)));
    assert!(matches!(reject(&list, UserEntry::path("a", MatchKind::Exact, "C:\\Program Files\\App", T)), EntryError::PathRejected(TextualDeny::ProtectedLocation(_))));
    assert!(matches!(reject(&list, UserEntry::path("bad id!", MatchKind::Exact, "D:\\x", T)), EntryError::InvalidId));
    assert!(matches!(reject(&list, UserEntry::glob("a", "logs\\*.txt", T)), EntryError::InvalidGlob(_)));
    assert!(matches!(reject(&list, UserEntry::glob("a", "D:\\a\\..\\*", T)), EntryError::InvalidGlob(_)));
    assert!(matches!(reject(&list, UserEntry::glob("a", "D:\\a**\\x", T)), EntryError::InvalidGlob(_)));
    assert!(matches!(reject(&list, UserEntry::rule("a", "has space", T)), EntryError::InvalidIdentifier("rule_id")));
    assert!(matches!(reject(&list, UserEntry::task("a", "", T)), EntryError::InvalidIdentifier("task_id")));
    assert!(matches!(reject(&list, UserEntry::path("a", MatchKind::Exact, "D:\\x", T).with_note("a\nb")), EntryError::InvalidNote));
    let mut missing = UserEntry::rule("a", "r", T);
    missing.rule_id = None;
    assert!(matches!(reject(&list, missing), EntryError::MissingField("rule_id")));
    list.add(UserEntry::path("a", MatchKind::Exact, "D:\\x", T)).unwrap();
    assert!(matches!(list.add(UserEntry::path("a", MatchKind::Exact, "D:\\y", T)), Err(EntryError::DuplicateId(_))));
    assert_eq!(EntryError::InvalidId.code(), ErrorCode::InvalidArgument);
    assert!(list.remove("a").is_some());
    assert!(list.entries().is_empty());
}

#[test]
fn protect_entry_identity_into_hard_deny_is_rejected() {
    let h = hard();
    assert!(matches!(check_entry_identity(&chain("system32", &["windows"]), &h), Err(EntryError::TargetsHardDeny(_))));
    assert!(check_entry_identity(&chain("cache", &["c-root"]), &h).is_ok());
    assert!(check_entry_identity(&IdentityChain::default(), &h).is_ok(), "not-yet-existing paths may be protected");
}

// ---- layer precedence ---------------------------------------------------------------------

#[test]
fn protect_layers_apply_in_order_and_user_cannot_lift() {
    let h = hard();
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::path("cache", MatchKind::Descendants, "D:\\cache", T)).unwrap();
    user.add(UserEntry::rule("r", "rule.protected", T)).unwrap();
    let subject = |path: &'static str, ch: &'static IdentityChain, class, rule| Subject { path: Box::leak(Box::new(win(path))), chain: ch, default_class: class, rule_id: rule, recursive: false };
    let sys: &'static IdentityChain = Box::leak(Box::new(chain("system32", &["windows"])));
    let plain: &'static IdentityChain = Box::leak(Box::new(chain("obj", &["c-root"])));

    // Hard deny wins even when the user list and default class also match.
    let d = evaluate(&subject("D:\\cache\\x", sys, Some(ProtectionClass::Credentials), Some("rule.protected")), &h, &user);
    assert_eq!(d.layer(), Some(DenyLayer::HardDeny));
    // Default class beats the user list.
    let d = evaluate(&subject("D:\\cache\\x", plain, Some(ProtectionClass::Database), None), &h, &user);
    assert_eq!(d.layer(), Some(DenyLayer::DefaultClass));
    assert!(matches!(&d, ProtectionDecision::Denied { reason, .. } if reason.contains("database")));
    // User path and user rule.
    assert_eq!(evaluate(&subject("D:\\cache\\x", plain, None, None), &h, &user).layer(), Some(DenyLayer::User));
    assert_eq!(evaluate(&subject("D:\\other\\x", plain, None, Some("rule.protected")), &h, &user).layer(), Some(DenyLayer::User));
    assert!(evaluate(&subject("D:\\other\\x", plain, None, Some("rule.free")), &h, &user).is_allowed());
    // An empty user list removes nothing from the first two layers.
    let empty = UserProtection::empty(Platform::Windows);
    assert_eq!(evaluate(&subject("D:\\cache\\x", sys, None, None), &h, &empty).layer(), Some(DenyLayer::HardDeny));
    assert_eq!(evaluate(&subject("D:\\cache\\x", plain, Some(ProtectionClass::Credentials), None), &h, &empty).layer(), Some(DenyLayer::DefaultClass));
    // A link whose real target the user protected is denied by the user layer.
    let linked: &'static IdentityChain = Box::leak(Box::new(IdentityChain {
        object: Some(id("j")),
        real_path: None,
        ancestors: vec![Some(id("c-root"))],
        links: vec![LinkResolution {
            link: win("D:\\tmp\\j"),
            on_object: true,
            target: Ok(ResolvedTarget { path: win("D:\\cache\\real"), object: id("real"), ancestors: vec![id("c-root")] }),
        }],
    }));
    assert_eq!(evaluate(&subject("D:\\tmp\\j", linked, None, None), &h, &user).layer(), Some(DenyLayer::User));
}

#[test]
fn protect_explain_reports_every_layer_strongest_first() {
    let h = hard();
    let mut user = UserProtection::empty(Platform::Windows);
    user.add(UserEntry::path("keep", MatchKind::Descendants, "D:\\cache", T)).unwrap();
    let sys = chain("system32", &["windows"]);
    let path = win("D:\\cache\\x");
    let s = Subject { path: &path, chain: &sys, default_class: Some(ProtectionClass::AiSessionsConfig), rule_id: None, recursive: false };
    let v = explain(&s, &h, &user);
    assert_eq!(v.iter().map(|l| l.layer).collect::<Vec<_>>(), [DenyLayer::HardDeny, DenyLayer::DefaultClass, DenyLayer::User]);
    assert!(v.iter().all(|l| l.decision.is_denied()), "every applicable reason is shown, not only the first");
    assert_eq!(v[2].entry_id.as_deref(), Some("keep"));
    assert_eq!(evaluate(&s, &h, &user), v[0].decision);
    let plain = chain("obj", &["c-root"]);
    let other = win("D:\\other\\x");
    let s = Subject { path: &other, chain: &plain, default_class: None, rule_id: None, recursive: false };
    assert!(explain(&s, &h, &user).iter().all(|l| l.decision.is_allowed() && l.entry_id.is_none()));
}

#[test]
fn protect_textual_checks_apply_to_real_locations() {
    let h = hard();
    let target = ResolvedTarget { path: unix("/private/var"), object: id("private-var"), ancestors: vec![id("c-root")] };
    let mut ch = IdentityChain {
        object: Some(id("tmp1")),
        real_path: Some(unix("/private/var/folders/ab/cd/T/tmp1")),
        ancestors: vec![Some(id("c-root"))],
        links: vec![LinkResolution { link: unix("/var"), on_object: false, target: Ok(target) }],
    };
    assert!(check_identity(&ch, &h).is_allowed(), "an ancestor link into an exact-protected directory does not deny its descendants");
    ch.links[0].on_object = true;
    assert!(check_identity(&ch, &h).is_denied(), "a link that is itself the object and points at /private/var is denied");
    ch.links[0].on_object = false;
    ch.real_path = Some(unix("/System/Library/x"));
    assert!(check_identity(&ch, &h).is_denied(), "the real location goes through the textual filter");
}
