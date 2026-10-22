use std::collections::BTreeMap;

use super::*;
use crate::analysis::{EntryFacts, MemView};
use crate::model::IdentityQuality;

/// Minimal PE image: MZ stub, PE header, one section of 0x200 raw bytes at 0x200.
pub(crate) fn pe(section: &[u8; 8], overlay: &[u8], body_marker: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    b[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // one section
    b[0x54..0x56].copy_from_slice(&0u16.to_le_bytes()); // no optional header
    let s = 0x40 + 24;
    b[s..s + 8].copy_from_slice(section);
    b[s + 16..s + 20].copy_from_slice(&0x200u32.to_le_bytes());
    b[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes());
    b[0x210..0x210 + body_marker.len()].copy_from_slice(body_marker);
    b.extend_from_slice(overlay);
    b
}

pub(crate) fn cfb(clsid: [u8; 16]) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[..8].copy_from_slice(&CFB_MAGIC);
    b[0x1e..0x20].copy_from_slice(&9u16.to_le_bytes());
    b[0x30..0x34].copy_from_slice(&0u32.to_le_bytes());
    b[512 + 0x50..512 + 0x60].copy_from_slice(&clsid);
    b
}

/// A ZIP whose central directory lists `entries` (name, general-purpose flags).
pub(crate) fn zip(entries: &[(&str, u16)]) -> Vec<u8> {
    let mut b = b"PK\x03\x04".to_vec();
    b.resize(64, 0);
    let cd_off = b.len() as u32;
    for (name, flags) in entries {
        let mut e = b"PK\x01\x02".to_vec();
        e.resize(46, 0);
        e[8..10].copy_from_slice(&flags.to_le_bytes());
        e[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        e.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&e);
    }
    let cd_size = b.len() as u32 - cd_off;
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.resize(22, 0);
    eocd[10..12].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd[12..16].copy_from_slice(&cd_size.to_le_bytes());
    eocd[16..20].copy_from_slice(&cd_off.to_le_bytes());
    b.extend_from_slice(&eocd);
    b
}

fn kind(bytes: &[u8]) -> Option<InstallerKind> {
    match classify(&bytes.to_vec()) {
        Verdict::Installer { kind, .. } => Some(kind),
        Verdict::NotInstaller { .. } | Verdict::InUse { .. } => None,
    }
}

fn reason(bytes: &[u8]) -> String {
    match classify(&bytes.to_vec()) {
        Verdict::NotInstaller { reason } => reason,
        other => panic!("expected not an installer, got {other:?}"),
    }
}

#[test]
fn executables_need_an_installer_engine_signature() {
    assert_eq!(kind(&pe(b".text\0\0\0", &[], b"")), None, "an ordinary program is not an installer");
    assert_eq!(kind(&pe(b".wixburn", &[], b"")), Some(InstallerKind::Exe));
    let mut nsis = vec![0u8; 4];
    nsis.extend_from_slice(&[0xef, 0xbe, 0xad, 0xde]);
    nsis.extend_from_slice(b"NullsoftInst");
    nsis.resize(64, 0);
    assert_eq!(kind(&pe(b".text\0\0\0", &nsis, b"")), Some(InstallerKind::Exe));
    let inno: Vec<u8> = "This installation was built with Inno Setup.".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(kind(&pe(b".rsrc\0\0\0", &[], &inno)), Some(InstallerKind::Exe));
    assert_eq!(kind(&b"MZ but not really a program, just a renamed text file".repeat(4)), None);
}

#[test]
fn msi_is_recognized_by_class_id_not_by_compound_format() {
    assert_eq!(kind(&cfb(CLSID_MSI)), Some(InstallerKind::Msi));
    assert_eq!(kind(&cfb(CLSID_MSP)), Some(InstallerKind::Msp));
    let word = [0x06, 0x09, 0x02, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
    assert!(reason(&cfb(word)).contains("not a Windows Installer"), "a legacy .doc is not an installer");
}

#[test]
fn archives_count_only_with_installer_payload() {
    assert_eq!(kind(&zip(&[("AppxManifest.xml", 0), ("App.exe", 0)])), Some(InstallerKind::Msix));
    assert_eq!(kind(&zip(&[("AppxMetadata/AppxBundleManifest.xml", 0), ("x.msix", 0)])), Some(InstallerKind::MsixBundle));
    assert_eq!(kind(&zip(&[("readme.txt", 0), ("tool/setup.msi", 0)])), Some(InstallerKind::Archive));
    assert!(reason(&zip(&[("report.pdf", 0), ("notes.docx", 0), ("setup.exe", 0)])).contains("without installer payload"), "documents (and bare exe names) only");
    assert!(reason(&zip(&[("setup.msi", 1)])).contains("encrypted"));
    assert!(reason(&b"Rar!\x1a\x07\x01\0 rest of a rar archive with an msi inside".repeat(2)).contains("not listed"));
}

#[test]
fn images_are_recognized_by_their_signatures() {
    let mut iso = vec![0u8; 0x9100];
    iso[0x8001..0x8006].copy_from_slice(b"CD001");
    assert_eq!(kind(&iso), Some(InstallerKind::Iso));
    let mut vhdx = b"vhdxfile".to_vec();
    vhdx.resize(4096, 0);
    assert_eq!(kind(&vhdx), Some(InstallerKind::Vhdx));
    let mut vhd = vec![0u8; 4096];
    vhd[4096 - 512..4096 - 504].copy_from_slice(b"conectix");
    assert_eq!(kind(&vhd), Some(InstallerKind::Vhd));
    let renamed_iso = vec![b'x'; 0x9100];
    assert_eq!(kind(&renamed_iso), None, "a renamed ordinary file is not an image");
}

#[test]
fn installed_hints_need_publisher_and_a_distinctive_name() {
    let apps = vec![("Google Chrome".to_string(), Some("Google LLC".to_string())), ("Visual Studio Code".to_string(), Some("Microsoft Corporation".to_string()))];
    assert_eq!(installed_hint("ChromeSetup.exe", Some("Google LLC"), &apps).as_deref(), Some("Google Chrome is installed (publisher Google LLC)"));
    assert!(installed_hint("ChromeSetup.exe", None, &apps).is_none(), "no signer, no hint");
    assert!(installed_hint("ChromeSetup.exe", Some("Evil Corp"), &apps).is_none());
    assert!(installed_hint("setup-x64.exe", Some("Microsoft Corporation"), &apps).is_none(), "generic words never match");
    assert!(installed_hint("VSCodeUserSetup-x64-1.95.exe", Some("Microsoft Corporation"), &apps).is_some());
    assert!(installed_hint("PowerToysSetup-0.85.exe", Some("Microsoft Corporation"), &apps).is_none(), "same publisher, different product");
}

#[test]
fn browser_download_dirs_are_read_from_preferences() {
    assert_eq!(chromium_download_dir(r#"{"download":{"default_directory":"D:\\Dl"}}"#).as_deref(), Some("D:\\Dl"));
    assert_eq!(chromium_download_dir(r#"{"download":{}}"#), None);
    let ff = "user_pref(\"browser.download.dir\", \"E:\\\\Firefox\");\nuser_pref(\"browser.download.folderList\", 2);\n";
    assert_eq!(firefox_download_dir(ff).as_deref(), Some("E:\\Firefox"));
    assert_eq!(firefox_download_dir("user_pref(\"browser.download.dir\", \"E:\\\\F\");"), None, "folderList 2 means the custom folder is used");
}

// ---------------------------------------------------------------------------------------------

fn wp(s: &str) -> RawPath {
    RawPath::windows(s.encode_utf16().collect()).unwrap()
}

fn view(files: &[&str]) -> MemView {
    let mut v = MemView { platform: Some(crate::capability::Platform::Windows), ..Default::default() };
    let mut ids: BTreeMap<String, u64> = BTreeMap::new();
    let add = |v: &mut MemView, id: u64, parent: Option<u64>, name: &str, kind: ObjectKind| {
        v.names.insert(id, wp(name));
        v.parents.insert(id, parent);
        if let Some(p) = parent {
            v.kids.entry(p).or_default().push(id);
        }
        v.facts.insert(id, EntryFacts {
            kind, boundary: false, object_id: id, logical: (kind == ObjectKind::File).then_some(4096), allocated: None, volume_id: "v".into(),
            file_id: format!("fileid128:{id:032x}"), quality: IdentityQuality::Strong, cloud: CloudState::NotCloud, modified_unix: Some(0), link_count: Some(1),
            skipped_here: 0,
        });
    };
    add(&mut v, 0, None, "C:\\Users\\me\\Downloads", ObjectKind::Directory);
    for f in files {
        let parts: Vec<&str> = f.split('/').collect();
        let mut parent = 0;
        let mut key = String::new();
        for (i, part) in parts.iter().enumerate() {
            key = format!("{key}/{part}");
            parent = match ids.get(&key) {
                Some(id) => *id,
                None => {
                    let id = ids.len() as u64 + 1;
                    add(&mut v, id, Some(parent), part, if i + 1 == parts.len() { ObjectKind::File } else { ObjectKind::Directory });
                    ids.insert(key.clone(), id);
                    id
                }
            };
        }
    }
    v
}

struct Fake;

impl InstallerProbe for Fake {
    fn classify(&self, path: &RawPath) -> Verdict {
        let p = path.lossy().to_lowercase();
        if p.contains("held") {
            Verdict::InUse { reason: "fake: held open without read sharing".into() }
        } else if p.ends_with(".exe") || p.ends_with(".iso") || p.ends_with(".msi") {
            let kind = if p.ends_with(".iso") { InstallerKind::Iso } else if p.ends_with(".msi") { InstallerKind::Msi } else { InstallerKind::Exe };
            yes(kind, "fake")
        } else {
            not("fake: not an installer")
        }
    }
    fn signer(&self, _: &RawPath) -> Option<String> {
        Some("Google LLC".into())
    }
    fn mounted(&self, path: &RawPath) -> Option<bool> {
        Some(path.lossy().contains("mounted"))
    }
    fn in_use(&self, path: &RawPath) -> Option<bool> {
        Some(path.lossy().contains("busy"))
    }
    fn holders(&self, path: &RawPath) -> Option<Vec<String>> {
        path.lossy().contains("held.zip").then(|| vec!["powershell.exe (pid 4242)".to_string()])
    }
}

/// INC-04 (WFA-58): a file held open without read sharing cannot be recognized, yet it is listed as in use (with its
/// holder when known) and never becomes plannable; a held file whose name suggests no installer stays out.
#[test]
fn a_held_file_is_listed_in_use_with_its_holder_and_is_never_plannable() {
    let v = view(&["held.zip", "held-unknown-holder.msi", "held-notes.pdf", "ChromeSetup.exe"]);
    let input = InstallerInput { scan_id: "s", revision: 1, source_label: "downloads", max_depth: 2, apps: &[] };
    let a = analyze_installers(&v, &input, &AnalysisProtection::default(), &Fake);
    let names: Vec<String> = a.set.candidates.iter().map(|c| c.path.lossy().rsplit('\\').next().unwrap().to_owned()).collect();
    assert_eq!(names, ["held.zip", "held-unknown-holder.msi", "ChromeSetup.exe"], "the held pdf is not an installer by any measure");
    let (zip, zf) = (&a.set.candidates[0], &a.facts[0]);
    assert_eq!((zip.state, zip.tier, zip.allowed_modes.is_empty()), (CandidateState::PendingEvidence, Tier::ReviewOnly, true));
    assert!(zip.reasons.iter().any(|r| r.starts_with("in_use:")) && zip.reasons.iter().any(|r| r.starts_with("content_unverified")), "{:?}", zip.reasons);
    assert_eq!((zf.kind, zf.content_verified, zf.in_use, zf.signer.as_deref()), (InstallerKind::Archive, false, Some(true), None));
    assert_eq!(zf.holders.as_deref(), Some(&["powershell.exe (pid 4242)".to_string()][..]));
    assert_eq!(zf.in_use_note().as_deref(), Some("in use by powershell.exe (pid 4242): close it there, then scan again"));
    assert!(zf.recognition_note().starts_with("not recognized by content: fake: held open"), "{}", zf.recognition_note());
    assert!(zip.impact.starts_with("unverified archive installer"), "{}", zip.impact);
    let unknown = &a.facts[1];
    assert_eq!((unknown.in_use, unknown.holders.clone()), (Some(true), None));
    assert!(unknown.in_use_note().unwrap().contains("could not be determined"));
    // Plans never take an unverified row, even when named explicitly.
    assert!(crate::plan::build_plan(&v, &a.set, std::slice::from_ref(&zip.candidate_id), Mode::Trash, &plan_input()).is_err());
    let ok = &a.facts[2];
    assert!(ok.content_verified && ok.in_use_note().is_none() && ok.recognition_note().starts_with("recognized: "));
}

fn plan_input() -> crate::plan::PlanInput<'static> {
    crate::plan::PlanInput { plan_id: "p", scope_id: "scope", created_unix: 0, observed_at: "2026-10-22T00:00:00Z", policy_digest: "d" }
}

#[test]
fn names_suggest_a_kind_only_for_installer_extensions() {
    assert_eq!(InstallerKind::suggested_by_name("Setup.EXE"), Some(InstallerKind::Exe));
    assert_eq!(InstallerKind::suggested_by_name("pkg.appxbundle"), Some(InstallerKind::MsixBundle));
    assert_eq!(InstallerKind::suggested_by_name("disk.vhdx"), Some(InstallerKind::Vhdx));
    assert_eq!(InstallerKind::suggested_by_name("report.pdf"), None);
    assert_eq!(InstallerKind::suggested_by_name("noextension"), None);
}

#[test]
fn analysis_is_review_only_bounded_and_skips_live_conflicts() {
    let v = view(&["ChromeSetup.exe", "notes.pdf", "mounted.iso", "busy.msi", "sub/inner.exe", "sub/deeper/too-deep.exe"]);
    let apps = vec![("Google Chrome".to_string(), Some("Google LLC".to_string()))];
    let input = InstallerInput { scan_id: "s", revision: 1, source_label: "downloads", max_depth: 2, apps: &apps };
    let a = analyze_installers(&v, &input, &AnalysisProtection::default(), &Fake);
    let names: Vec<String> = a.set.candidates.iter().map(|c| c.path.lossy().rsplit('\\').next().unwrap().to_owned()).collect();
    assert_eq!(names, ["ChromeSetup.exe", "mounted.iso", "busy.msi", "inner.exe"], "pdf not an installer; depth 3 not searched");
    assert!(a.set.candidates.iter().all(|c| c.tier == Tier::ReviewOnly), "INC-03: never preselected");
    let iso = &a.set.candidates[1];
    assert_eq!(iso.state, CandidateState::PendingEvidence);
    assert!(iso.reasons.iter().any(|r| r.starts_with("mounted_image")));
    assert!(a.set.candidates[2].reasons.iter().any(|r| r.starts_with("in_use")));
    assert_eq!(a.facts[0].installed_hint.as_deref(), Some("Google Chrome is installed (publisher Google LLC)"));
    assert_eq!(a.facts[1].signer, None, "images carry no Authenticode signer");
    assert!(a.set.candidates[0].rules[0].rule_id == "installer.exe" && is_current_digest(&a.set.rule_set_digest));
    assert!(crate::plan::is_builtin_catalog_digest(&a.set.rule_set_digest));
}
