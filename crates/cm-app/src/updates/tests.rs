//! Injected channels and backends only: no package manager runs, nothing is installed, no network is used.

use std::cell::{Cell, RefCell};

use super::*;
use crate::execute::ApplyContext;
use crate::plans::{V2Backends, apply_plan_v2};
use cm_core::apps::{AppSource, RegistryHive, RegistryView};
use cm_core::authz::{KindReceipt, ReceiptSource};
use cm_core::execute::{ActionOutcome, ActionTarget, FileActions, LiveCheck, Reconciled};
use cm_core::host::HostRef;
use cm_core::updates::ToolOutput;
use cm_store::Stores;

const T0: i64 = 1_800_000_000;
const PRINCIPAL: &str = "user:alice";

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cm-app-updates-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn raw(p: &str) -> RawPath {
    RawPath::windows(p.encode_utf16().collect()).unwrap()
}

fn app(name: &str, version: &str) -> AppInstance {
    let mut a = AppInstance::new(
        name.into(),
        AppSource::RegistryUninstall { hive: RegistryHive::Hkcu, view: RegistryView::Shared, key: name.into() },
        Some(raw(&format!(r"C:\Apps\{name}"))),
    );
    a.version = Some(version.into());
    a
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count())))
}

fn winget_text(rows: &[[&str; 5]]) -> String {
    let line = |c: &[&str; 5]| format!("{}{}{}{}{}", pad(c[0], 20), pad(c[1], 24), pad(c[2], 12), pad(c[3], 12), c[4]);
    let mut out = vec![line(&["Name", "Id", "Version", "Available", "Source"]), "-".repeat(76)];
    out.extend(rows.iter().map(line));
    out.join("\n")
}

/// Counts every channel call; each call stands for a network request by the package manager.
struct Channels {
    calls: Cell<u32>,
    text: String,
}

impl UpdateChannels for Channels {
    fn winget_list(&self) -> Result<String, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.text.clone())
    }
    fn scoop_apps(&self) -> Vec<ScoopApp> {
        self.calls.set(self.calls.get() + 1);
        Vec::new()
    }
    fn self_updater(&self, _: &AppInstance) -> Option<SelfUpdaterProbe> {
        self.calls.set(self.calls.get() + 1);
        None
    }
}

fn three() -> (Vec<AppInstance>, Channels) {
    let apps = vec![app("Alpha", "1.0"), app("Beta", "2.0"), app("Gamma", "3.0"), app("NoChannel", "9.9")];
    let text = winget_text(&[
        ["Alpha", "Vendor.Alpha", "1.0", "1.1", "winget"],
        ["Beta", "Vendor.Beta", "2.0", "2.1", "winget"],
        ["Gamma", "Vendor.Gamma", "3.0", "3.5", "winget"],
    ]);
    (apps, Channels { calls: Cell::new(0), text })
}

/// UPD-04: with checks off, no channel is called at all (zero network requests) and results show their old time.
#[test]
fn checks_off_make_no_requests_and_keep_old_times() {
    let store = dir("off");
    let (apps, ch) = three();
    let r = check(&store, &apps, &ch, "2026-10-15T09:00:00Z").unwrap();
    assert!(r.checked && ch.calls.get() > 0);
    assert_eq!(r.items.iter().find(|c| c.name == "NoChannel").unwrap().state, UpdateState::CannotCheck);
    set_checks(&store, false).unwrap();
    let before = ch.calls.get();
    let off = check(&store, &apps, &ch, "2026-10-16T09:00:00Z").unwrap();
    assert_eq!(ch.calls.get(), before, "checks off: no channel call, so no network request");
    assert!(!off.checked && !off.checks_enabled);
    assert_eq!(off.checked_at.as_deref(), Some("2026-10-15T09:00:00Z"));
    assert!(off.items.iter().all(|c| c.checked_at.as_deref() != Some("2026-10-16T09:00:00Z")));
    assert_eq!(off.items.iter().find(|c| c.name == "Alpha").unwrap().checked_at.as_deref(), Some("2026-10-15T09:00:00Z"));
    // Never checked + off: "not checked", never "up to date".
    let fresh = dir("off-fresh");
    set_checks(&fresh, false).unwrap();
    let ch2 = Channels { calls: Cell::new(0), text: String::new() };
    let r = check(&fresh, &apps, &ch2, "t").unwrap();
    assert_eq!(ch2.calls.get(), 0);
    assert!(r.items.iter().all(|c| c.state == UpdateState::NotChecked && c.checked_at.is_none()));
    let w = r.to_wire();
    assert_eq!(w["counts"]["not_checked"], 4);
    let _ = std::fs::remove_dir_all(&store);
    let _ = std::fs::remove_dir_all(&fresh);
}

fn tool() -> UpdateTool {
    UpdateTool { exe: raw(r"C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller\winget.exe"), sha256: "e".repeat(64) }
}

#[test]
fn plans_hold_verifiable_package_updates_and_route_the_rest() {
    let store = dir("plan");
    let (mut apps, ch) = three();
    apps.push(AppInstance::new("Photos".into(), AppSource::Msix { package_family: "P_8w".into(), full_name: "P_1_x64__8w".into() }, None));
    let r = check(&store, &apps, &ch, "t").unwrap();
    let ids: Vec<String> = apps.iter().map(|a| a.instance_id.clone()).collect();
    let signer = |a: &AppInstance| (a.name != "Gamma").then(|| "Vendor Inc.".to_owned());
    let inputs = PlanInputs { results: &r.items, apps: &apps, ids: &ids, winget: Some(tool()), scoop: None, installed_signer: &signer };
    let (plan, routed) = plan_updates(&inputs, "up_1", T0, NO_RULES_DIGEST).unwrap();
    let plan = plan.unwrap();
    assert_eq!(plan.body.plan_kind, PlanKind::Update);
    assert!(!plan.body.policy_matchable);
    assert_eq!(plan.body.items.len(), 2, "Alpha and Beta");
    let names: Vec<(&str, Route)> = routed.iter().map(|r| (r.name.as_str(), r.route)).collect();
    assert!(names.contains(&("Gamma", Route::OpenOfficialPage)), "unverifiable installed signer");
    assert!(names.contains(&("Photos", Route::OpenStore)));
    assert!(names.iter().any(|(n, _)| *n == "NoChannel"));
    let PlanAction::ToolAdapter(t) = &plan.body.items[0] else { panic!() };
    assert_eq!(t.argv, cm_core::updates::argv::winget_upgrade("Vendor.Alpha", "1.1"));
    assert!(describe(t, &plan.meta[0]).contains("1.0 -> 1.1"));
    let unknown = PlanInputs { ids: &["app_nope".to_owned()], ..inputs };
    assert!(matches!(plan_updates(&unknown, "up_2", T0, NO_RULES_DIGEST), Err(UpdatePlanError::UnknownApp(_))));
    let _ = std::fs::remove_dir_all(&store);
}

struct NoFiles;

impl FileActions for NoFiles {
    fn check(&self, _: &ActionTarget<'_>) -> LiveCheck {
        unreachable!("update plans have no file actions")
    }
    fn quarantine(&self, _: &ActionTarget<'_>, _: &RawPath, _: &str) -> ActionOutcome {
        unreachable!()
    }
    fn trash(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unreachable!()
    }
    fn delete(&self, _: &ActionTarget<'_>) -> ActionOutcome {
        unreachable!()
    }
    fn quarantine_dir_for(&self, _: &ActionTarget<'_>, _: &str) -> Option<RawPath> {
        None
    }
    fn reconcile(&self, _: &ActionTarget<'_>, _: cm_core::rules::Mode, _: Option<&RawPath>) -> Reconciled {
        Reconciled::StillUnknown
    }
    fn free_space(&self, _: &RawPath) -> Option<u64> {
        None
    }
}

/// Fake package manager: the upgrade of `failing` fails; every other step succeeds.
struct Backend {
    failing: String,
    upgraded: RefCell<Vec<String>>,
}

impl UpdateBackend for Backend {
    fn run(&self, _exe: &str, argv: &[String], _t: u32) -> Result<ToolOutput, String> {
        if argv[0] == "upgrade" {
            let id = argv[2].clone();
            if id == self.failing {
                return Ok(ToolOutput { exit_code: Some(1603), output: "Installer failed with exit code: 1603".into(), timed_out: false });
            }
            self.upgraded.borrow_mut().push(id);
        }
        Ok(ToolOutput { exit_code: Some(0), output: String::new(), timed_out: false })
    }
    fn file_sha256(&self, _: &str) -> Option<String> {
        Some("e".repeat(64))
    }
    fn signer(&self, _: &str) -> Option<String> {
        Some("Vendor, Inc.".into())
    }
    fn downloaded(&self, t: &UpdateTarget, dir: &str) -> Vec<String> {
        vec![format!(r"{dir}\{}.exe", t.package_id)]
    }
    fn scratch_dir(&self, id: &str) -> Result<String, String> {
        Ok(format!(r"C:\tmp\{id}"))
    }
    fn running(&self, _: &UpdateTarget) -> Option<bool> {
        Some(false)
    }
    fn installed_version(&self, t: &UpdateTarget) -> Option<String> {
        Some(if self.upgraded.borrow().contains(&t.package_id) { t.to_version.clone() } else { t.from_version.clone().unwrap_or_default() })
    }
}

/// UPD-03: in a batch of three, one failed upgrade does not stop the other two; the report has one failure with a summary.
#[test]
fn batch_reports_each_item_and_one_failure_does_not_stop_the_rest() {
    let store = dir("batch");
    let stores = Stores::open(&store).unwrap();
    let (apps, ch) = three();
    let r = check(&store, &apps, &ch, "t").unwrap();
    let ids: Vec<String> = apps[..3].iter().map(|a| a.instance_id.clone()).collect();
    let signer = |_: &AppInstance| Some("Vendor Inc.".to_owned());
    let (plan, _) = plan_updates(&PlanInputs { results: &r.items, apps: &apps, ids: &ids, winget: Some(tool()), scoop: None, installed_signer: &signer }, "up_batch", T0, NO_RULES_DIGEST).unwrap();
    let plan = plan.unwrap();
    assert_eq!(plan.body.items.len(), 3);
    stores.audit.save_plan_v2(&plan).unwrap();
    let backend = Backend { failing: "Vendor.Beta".into(), upgraded: RefCell::new(vec![]) };
    let b = V2Backends { files: &NoFiles, startup: None, holders: &|_| None, protect_item: &|_| Ok(()), processes: None, updates: Some(&backend), uninstall: None };
    let h = HostRef { kind: "cli".into(), pid: 1, started_at: "2026-10-15T00:00:00Z".into() };
    let ctx = ApplyContext { principal: PRINCIPAL, host: &h, current_rule_set_digest: "", now: &|| T0 + 10, cancel: &|| false, fault: &|_, _| false, protect: &|_| Ok(()) };
    assert!(matches!(apply_plan_v2(&stores, &b, "up_batch", &ctx), Err(crate::execute::ApplyError::NeedsApproval(_))), "only a review authorizes updates");
    stores
        .audit
        .save_kind_receipt(&KindReceipt {
            receipt_id: "r1".into(),
            plan_id: "up_batch".into(),
            plan_digest: plan.digest.clone(),
            plan_kind: PlanKind::Update,
            principal: PRINCIPAL.into(),
            source: ReceiptSource::InteractiveReview,
            created_at: plan.body.created_at.clone(),
            expires_at: plan.body.expires_at.clone(),
            revoked_at: None,
        })
        .unwrap();
    let report = apply_plan_v2(&stores, &b, "up_batch", &ctx).unwrap();
    let statuses: Vec<ActionStatus> = report.items.iter().map(|i| i.status).collect();
    assert_eq!(statuses, [ActionStatus::Succeeded, ActionStatus::Failed, ActionStatus::Succeeded]);
    assert_eq!((report.succeeded, report.failed), (2, 1));
    assert_eq!(report.status, "partial");
    assert!(report.items[1].message.contains("1603"), "{}", report.items[1].message);
    assert_eq!(*backend.upgraded.borrow(), ["Vendor.Alpha", "Vendor.Gamma"]);
    drop(stores);
    let _ = std::fs::remove_dir_all(&store);
}
