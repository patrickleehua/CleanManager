//! Real-machine acceptance follow-ups on Windows (BUG-WFA-03, WFA-33/34/40/42/43), end to end through the binary on
//! real NTFS with the user-temp rule (review-only, no owner process) and quarantine mode, inside a scratch directory:
//! - an unusable protection list refuses `plan apply` and `plan create` before anything starts, so the same approved
//!   plan applies after the repair (PRT-02);
//! - a second plan whose targets another plan already moved reports them missing, not protected;
//! - an operation interrupted after an action (the executor's fault-injection seam, with a host that has exited) shows
//!   a report marked `reconciliation_required`, and reconciling it keeps its authorization source;
//! - `plan show` names the authorizing policy, and refusals say why each policy does not apply.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

struct Fx {
    base: PathBuf,
}

impl Fx {
    fn new(name: &str) -> Fx {
        let base = std::env::temp_dir().join(format!("cm-cli-outcomes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("local").join("Temp")).unwrap();
        Fx { base }
    }
    fn local(&self) -> PathBuf {
        self.base.join("local")
    }
    fn temp(&self) -> PathBuf {
        self.local().join("Temp")
    }
    fn store(&self) -> PathBuf {
        self.base.join("store")
    }
    /// A file the user-temp rule plans (older than its age threshold).
    fn stale(&self, name: &str, size: usize) -> PathBuf {
        let p = self.temp().join(name);
        std::fs::write(&p, vec![7u8; size]).unwrap();
        let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(month_ago).unwrap();
        p
    }
    fn cli(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager"))
            .args(args)
            .arg("--store")
            .arg(self.store())
            .arg("--json")
            .env("CM_RESOLVE_LOCALAPPDATA", self.local())
            .env("CM_RESOLVE_TEMP", self.temp())
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "exactly one record for {args:?}: {text}\n{}", String::from_utf8_lossy(&out.stderr));
        let v: Value = serde_json::from_str(&text).unwrap();
        schema_fields(&v);
        (out.status.code().unwrap(), v)
    }
    /// scan + analyze of the temp folder: the candidate set and candidate ids by file name.
    fn analyze(&self) -> (String, Vec<(String, Value)>) {
        let (code, scan) = self.cli(&["scan", "--root", self.temp().to_str().unwrap()]);
        assert_eq!(code, 0, "{scan}");
        let (_, analysis) = self.cli(&["analyze", "--scan", scan["scan_id"].as_str().unwrap()]);
        let set = analysis["candidate_set_id"].as_str().unwrap().to_owned();
        let (_, page) = self.cli(&["candidates", "list", "--set", &set]);
        let items = page["items"].as_array().cloned().unwrap_or_default();
        let named = items.into_iter().map(|c| (c["path"]["display"].as_str().unwrap().rsplit('\\').next().unwrap().to_owned(), c)).collect();
        (set, named)
    }
    fn plan(&self, set: &str, mode: &str, candidates: &[&str]) -> Value {
        let (code, plan) = self.cli(&["plan", "create", "--set", set, "--mode", mode, "--candidates", &candidates.join(",")]);
        assert_eq!(code, 0, "{plan}");
        plan
    }
    fn stores(&self) -> cm_store::Stores {
        cm_store::Stores::open(&self.store()).unwrap()
    }
    /// A receipt equivalent to the terminal review's (the review itself needs a TTY), or the one the executor stores
    /// when a policy authorizes the plan.
    fn receipt(&self, plan: &Value, policy: Option<(&str, u32)>) {
        self.stores()
            .audit
            .save_receipt(&cm_core::authz::Receipt {
                receipt_id: format!("r_test_{}", plan["body"]["plan_id"].as_str().unwrap()),
                plan_id: plan["body"]["plan_id"].as_str().unwrap().into(),
                plan_digest: plan["plan_digest"].as_str().unwrap().into(),
                mode: serde_json::from_value(plan["body"]["mode"].clone()).unwrap(),
                principal: cm_core::authz::current_principal(),
                source: if policy.is_some() { cm_core::authz::ReceiptSource::Policy } else { cm_core::authz::ReceiptSource::InteractiveReview },
                policy_id: policy.map(|(id, _)| id.into()),
                policy_version: policy.map(|(_, v)| v),
                created_at: cm_core::time::now_utc(),
                expires_at: plan["body"]["expires_at"].as_str().unwrap().into(),
                revoked_at: None,
            })
            .unwrap();
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn candidate<'a>(named: &'a [(String, Value)], name: &str) -> &'a str {
    named.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} is a candidate: {named:?}")).1["candidate_id"].as_str().unwrap()
}

fn plan_id(plan: &Value) -> &str {
    plan["body"]["plan_id"].as_str().unwrap()
}

/// Every top-level field (and every `details` field of an error) is declared by schemas/v1 (`policy_list` is not part
/// of schemas/v1).
fn schema_fields(v: &Value) {
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
    if v["kind"] == "policy_list" {
        return;
    }
    let def = &schema["$defs"][v["kind"].as_str().unwrap()];
    let props = def["properties"].as_object().unwrap_or_else(|| panic!("kind {} is in schemas/v1", v["kind"]));
    for k in v.as_object().unwrap().keys() {
        assert!(props.contains_key(k), "{}: field {k} is not in the schema", v["kind"]);
    }
    if v["kind"] == "error" {
        let details = props["details"]["properties"].as_object().unwrap();
        for k in v["details"].as_object().unwrap().keys() {
            assert!(details.contains_key(k), "error.details.{k} is not in the schema");
        }
    }
}

fn item<'a>(report: &'a Value, action_id: &str) -> &'a Value {
    report["items"].as_array().unwrap().iter().find(|i| i["action_id"] == action_id).unwrap_or_else(|| panic!("{action_id} in {report}"))
}

/// BUG-WFA-03 / PRT-02: while `protection.json` is unreadable, `plan apply` of an approved plan and `plan create` are
/// refused with `protection_unavailable` before anything starts. Nothing is recorded, so after the repair the same
/// approved plan runs (it is not used up by a report of skipped items).
#[test]
fn an_unusable_protection_list_stops_apply_and_plan_create_without_using_up_the_plan() {
    let fx = Fx::new("protection");
    let a = fx.stale("a.tmp", 4096);
    let b = fx.stale("b.tmp", 4096);
    let (set, named) = fx.analyze();
    let plan = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp"), candidate(&named, "b.tmp")]);
    fx.receipt(&plan, None);
    std::fs::create_dir_all(fx.store()).unwrap();
    std::fs::write(fx.store().join("protection.json"), b"{not json").unwrap();

    let (code, err) = fx.cli(&["plan", "apply", plan_id(&plan)]);
    assert_eq!((code, err["kind"].as_str(), err["code"].as_str()), (8, Some("error"), Some("protection_unavailable")), "{err}");
    assert_eq!(err["details"]["plan_id"], plan["body"]["plan_id"]);
    assert!(err["required_action"].as_str().unwrap().contains("protection.json"), "{err}");
    assert!(a.exists() && b.exists(), "nothing was touched");
    let (_, ops) = fx.cli(&["operations", "list"]);
    assert_eq!(ops["items"].as_array().unwrap().len(), 0, "no operation was recorded: {ops}");
    // Another plan cannot be created from the set analyzed before the outage either.
    let (code, err) = fx.cli(&["plan", "create", "--set", &set, "--mode", "quarantine", "--candidates", candidate(&named, "a.tmp")]);
    assert_eq!((code, err["code"].as_str()), (8, Some("protection_unavailable")), "{err}");
    // Reading the plan keeps working; its approval is untouched.
    let (code, shown) = fx.cli(&["plan", "show", plan_id(&plan)]);
    assert_eq!((code, shown["authorization_status"].as_str()), (0, Some("authorized")), "{shown}");
    assert_eq!(std::fs::read(fx.store().join("protection.json")).unwrap(), b"{not json", "left for repair");

    // Repaired: the same approved plan applies now.
    std::fs::remove_file(fx.store().join("protection.json")).unwrap();
    let (code, report) = fx.cli(&["plan", "apply", plan_id(&plan)]);
    assert_eq!((code, report["status"].as_str(), report["succeeded"].as_u64()), (0, Some("completed"), Some(2)), "{report}");
    assert!(!a.exists() && !b.exists());
}

/// WFA-33: plan B planned the same file as plan A; after A moved it, B reports it missing (`precondition_missing`),
/// not protected or hard-denied.
#[test]
fn a_second_plan_whose_target_is_already_gone_reports_it_missing() {
    let fx = Fx::new("overlap");
    fx.stale("a.tmp", 4096);
    let (set, named) = fx.analyze();
    let first = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp")]);
    let second = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp")]);
    fx.receipt(&first, None);
    fx.receipt(&second, None);
    let (code, report) = fx.cli(&["plan", "apply", plan_id(&first)]);
    assert_eq!((code, report["status"].as_str()), (0, Some("completed")), "{report}");
    let (code, report) = fx.cli(&["plan", "apply", plan_id(&second)]);
    assert_eq!((code, report["status"].as_str(), report["skipped"].as_u64()), (5, Some("partial"), Some(1)), "{report}");
    let only = &report["items"][0];
    assert_eq!((only["status"].as_str(), only["code"].as_str()), (Some("skipped"), Some("precondition_missing")), "{report}");
    assert!(!only["message"].as_str().unwrap().contains("HardDeny"), "{report}");
}

/// WFA-34 / WFA-40: the executor's fault-injection seam stops an operation right after its second action (as a killed
/// process would), on behalf of a host that has exited. `operations show` returns a report marked
/// `reconciliation_required` (not an internal error); reconciling resolves the unconfirmed action from the file system
/// without acting again and keeps the operation's authorization source, recording the reconciliation apart.
#[test]
fn an_interrupted_operation_shows_a_report_to_reconcile_and_reconciling_keeps_its_source() {
    let fx = Fx::new("interrupted");
    for name in ["a.tmp", "b.tmp", "c.tmp"] {
        fx.stale(name, 2048);
    }
    let (set, named) = fx.analyze();
    let plan = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp"), candidate(&named, "b.tmp"), candidate(&named, "c.tmp")]);
    fx.receipt(&plan, None);
    {
        let stores = fx.stores();
        // Same process id, another start time: the host is gone as far as liveness can tell.
        let host = cm_core::host::HostRef { kind: "cli".into(), pid: std::process::id(), started_at: "2000-01-01T00:00:00Z".into() };
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let ctx = cm_app::execute::ApplyContext {
            principal: &cm_core::authz::current_principal(),
            host: &host,
            current_rule_set_digest: plan["body"]["rule_set_digest"].as_str().unwrap(),
            now: &now,
            cancel: &|| false,
            fault: &|p, id| p == cm_app::execute::FaultPoint::AfterAction && id == "a0000001",
            protect: &|_| Ok(()),
        };
        let actions = cm_platform::windows::actions::WindowsActions::with_store(fx.store());
        let err = cm_app::execute::apply_plan(&stores, &actions, plan_id(&plan), &ctx).unwrap_err();
        assert!(matches!(err, cm_app::execute::ApplyError::Crashed(_)), "{err:?}");
    }
    let op = format!("op_{}", &plan["plan_digest"].as_str().unwrap()[..16]);

    let (code, shown) = fx.cli(&["operations", "show", &op]);
    assert_eq!((code, shown["kind"].as_str(), shown["status"].as_str()), (8, Some("operation_report"), Some("reconciliation_required")), "{shown}");
    assert_eq!((item(&shown, "a0000000")["status"].as_str(), item(&shown, "a0000001")["code"].as_str()), (Some("succeeded"), Some("outcome_unconfirmed")), "{shown}");
    assert!(item(&shown, "a0000001")["message"].as_str().unwrap().contains("operations reconcile"), "{shown}");
    assert_eq!(item(&shown, "a0000002")["status"], "not_started", "{shown}");
    assert_eq!((shown["succeeded"].as_u64(), shown["unknown"].as_u64(), shown["not_started"].as_u64()), (Some(1), Some(1), Some(1)), "{shown}");
    assert!(shown.get("reconciled_at").is_none(), "{shown}");

    let (code, rec) = fx.cli(&["operations", "reconcile", &op]);
    assert_eq!((code, rec["status"].as_str()), (5, Some("partial")), "{rec}");
    assert_eq!(item(&rec, "a0000001")["code"], "reconciled_performed", "{rec}");
    assert!(fx.temp().join("c.tmp").exists(), "reconciliation never acts");
    let at = rec["reconciled_at"].as_str().expect("the reconciliation is recorded").to_owned();
    let (_, ops) = fx.cli(&["operations", "list"]);
    let row = ops["items"].as_array().unwrap().iter().find(|o| o["operation_id"] == op.as_str()).unwrap().clone();
    assert_eq!(row["authorization_source"], "interactive_review", "the original approval is kept: {row}");
    assert_eq!(row["reconciled_at"].as_str(), Some(at.as_str()), "{row}");
    // Nothing is left to resolve: reconciling again and showing it change nothing.
    let (_, again) = fx.cli(&["operations", "reconcile", &op]);
    assert_eq!(again["reconciled_at"].as_str(), Some(at.as_str()), "{again}");
    let (code, shown) = fx.cli(&["operations", "show", &op]);
    assert_eq!((code, shown["reconciled_at"].as_str()), (5, Some(at.as_str())), "{shown}");
}

/// WFA-42/43: refusals and `plan show` say which policy applies and why the others do not; once a policy authorized
/// the plan, `plan show` names it with its version.
#[test]
fn plan_show_and_refusals_explain_policies() {
    let fx = Fx::new("policies");
    fx.stale("a.tmp", 4096);
    let (set, named) = fx.analyze();
    let c = &named.iter().find(|(n, _)| n == "a.tmp").unwrap().1;
    let rule = format!("{}@{}", c["rule_id"].as_str().unwrap(), c["rule_version"].as_str().unwrap());
    let plan = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp")]);
    assert_eq!(plan["authorized_by"], Value::Null, "{plan}");
    assert_eq!(plan["policy_matches"], serde_json::json!([]), "no policy exists yet: {plan}");
    let (code, err) = fx.cli(&["plan", "apply", plan_id(&plan)]);
    assert_eq!((code, err["code"].as_str()), (3, Some("needs_approval")), "{err}");
    assert!(err["message"].as_str().unwrap().contains("no policy is set up"), "{err}");

    let scope = plan["body"]["scope_id"].as_str().unwrap();
    let (code, _) = fx.cli(&["policy", "create", "--scope", scope, "--rules", &rule, "--mode", "trash"]);
    assert_eq!(code, 0);
    let (code, err) = fx.cli(&["plan", "apply", plan_id(&plan)]);
    assert_eq!((code, err["details"]["policy_matches"][0]["reason"].as_str()), (3, Some("not_enabled")), "a paused policy says so: {err}");
    assert!(err["message"].as_str().unwrap().contains("paused"), "{err}");
    // Enabled (as `policy enable` does in a terminal): now the mode is what keeps it from applying.
    {
        let stores = fx.stores();
        let mut p = stores.audit.policies().unwrap().remove(0);
        p.state = cm_core::authz::PolicyState::Enabled;
        p.version += 1;
        stores.audit.save_policy(&p, &cm_core::time::now_utc()).unwrap();
    }
    let (code, err) = fx.cli(&["plan", "apply", plan_id(&plan)]);
    let m = &err["details"]["policy_matches"][0];
    assert_eq!((code, m["reason"].as_str(), m["covers"].as_bool(), m["policy_version"].as_str()), (3, Some("mode_differs"), Some(false), Some("2")), "{err}");
    assert!(m["message"].as_str().unwrap().contains("quarantine") && m["message"].as_str().unwrap().contains("trash"), "both modes are named: {err}");
    assert!(err["message"].as_str().unwrap().contains(m["policy_id"].as_str().unwrap()), "{err}");
    // `plan show` lists the same evaluation while a policy could still matter.
    let (_, shown) = fx.cli(&["plan", "show", plan_id(&plan)]);
    assert_eq!(shown["policy_matches"][0]["reason"], "mode_differs", "{shown}");

    // A plan a policy authorized (the receipt the executor stores): `plan show` names the policy and its version.
    let used = fx.plan(&set, "quarantine", &[candidate(&named, "a.tmp")]);
    let policy_id = m["policy_id"].as_str().unwrap();
    fx.receipt(&used, Some((policy_id, 2)));
    let (_, shown) = fx.cli(&["plan", "show", plan_id(&used)]);
    assert_eq!(shown["authorization_status"], "authorized", "{shown}");
    let by = &shown["authorized_by"];
    assert_eq!((by["source"].as_str(), by["policy_id"].as_str(), by["policy_version"].as_str()), (Some("policy"), Some(policy_id), Some("2")), "{shown}");
    assert!(shown.get("policy_matches").is_none(), "an authorized plan needs no policy evaluation: {shown}");
}
