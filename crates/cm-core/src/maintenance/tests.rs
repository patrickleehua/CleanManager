use super::*;
use crate::plan::v2::{PlanAction, check_plan_v2, validate};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Scripted(Vec<(&'static str, Probe, Execution)>);

impl TaskBackend for Scripted {
    fn probe(&self, task: &TaskSpec) -> Probe {
        self.0.iter().find(|(id, ..)| *id == task.id).map(|(_, p, _)| p.clone()).unwrap_or(Probe::Unknown { reason: "not scripted".into() })
    }
    fn execute(&self, task: &TaskSpec) -> Execution {
        self.0.iter().find(|(id, ..)| *id == task.id).map(|(_, _, e)| e.clone()).expect("execute only after an applicable probe")
    }
}

fn now() -> i64 {
    1_800_000_000
}

#[test]
fn mnt01_windows_catalog_covers_the_first_batch_without_speed_promises() {
    let ids: Vec<&str> = WINDOWS_TASKS.iter().map(|t| t.id).collect();
    assert_eq!(ids, [ids::SHELL_RESTART, ids::ICON_CACHE, ids::DNS_FLUSH, ids::TRIM, ids::IMAGE_HEALTH]);
    for t in WINDOWS_TASKS {
        assert!(!t.problem.is_empty() && !t.impact.is_empty() && !t.applies_when.is_empty() && t.timeout_seconds >= t.estimated_seconds);
        let text = format!("{} {} {}", t.title, t.problem, t.impact).to_lowercase();
        assert!(!text.contains('%') && !text.contains("faster") && !text.contains("boost"), "{}: no speed promise", t.id);
    }
    assert!(find(ids::TRIM).unwrap().requires_admin && find(ids::IMAGE_HEALTH).unwrap().requires_admin);
    assert!(!find(ids::DNS_FLUSH).unwrap().requires_admin && !find(ids::ICON_CACHE).unwrap().requires_admin);
    assert_eq!(catalog(TaskPlatform::Macos), MACOS_TASKS);
    assert!(WINDOWS_TASKS.iter().all(|t| t.platform == TaskPlatform::Windows && t.id.starts_with("windows.")));
    assert!(MACOS_TASKS.iter().all(|t| t.platform == TaskPlatform::Macos && t.id.starts_with("macos.")));
    let mut all: Vec<&str> = WINDOWS_TASKS.iter().chain(MACOS_TASKS).map(|t| t.id).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), WINDOWS_TASKS.len() + MACOS_TASKS.len(), "task IDs are unique across platforms");
    assert!(MACOS_TASKS.iter().all(|t| find(t.id).is_some_and(|f| f.platform == TaskPlatform::Macos)));
    assert_eq!(catalog_digest(TaskPlatform::Windows), catalog_digest(TaskPlatform::Windows));
}

#[test]
fn mnt05_registry_optimizer_search_finds_no_task_but_explains_why() {
    for q in ["注册表优化", "registry cleaner", "Registry Optimizer", "禁用服务", "debloat", "内存释放", "一键加速", "pagefile", "disable indexing"] {
        let (tasks, prohibited) = search(q, TaskPlatform::Windows);
        assert!(tasks.is_empty(), "{q}: no task may match, got {:?}", tasks.iter().map(|t| t.id).collect::<Vec<_>>());
        assert!(!prohibited.is_empty() && prohibited.iter().all(|p| !p.reason.is_empty()), "{q}: explained");
    }
    // "Search" itself is a task (restart the Search host); turning indexing off is explained, not offered.
    let (tasks, prohibited) = search("关闭搜索索引", TaskPlatform::Windows);
    assert!(tasks.iter().all(|t| t.id == ids::SHELL_RESTART) && prohibited.iter().any(|p| p.id == "search-indexing-disable"));
    let (tasks, _) = search("dns", TaskPlatform::Windows);
    assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [ids::DNS_FLUSH]);
    // Naming a prohibited entry as a task is refused with its reason.
    let r = resolve_tasks(&["registry-optimizer"], TaskPlatform::Windows).unwrap_err();
    assert_eq!(r.code, "policy_denied");
    assert!(r.message.contains("no task"));
    assert_eq!(resolve_tasks(&["windows.nope"], TaskPlatform::Windows).unwrap_err().code, "invalid_argument");
    assert_eq!(resolve_tasks(&[ids::DNS_FLUSH, ids::DNS_FLUSH], TaskPlatform::Windows).unwrap_err().code, "invalid_argument");
    assert_eq!(resolve_tasks(&[ids::DNS_FLUSH], TaskPlatform::Macos).unwrap_err().code, "unsupported");
}

#[test]
fn a_run_is_a_maintenance_plan_bound_to_the_catalog() {
    let tasks = resolve_tasks(&[ids::TRIM, ids::DNS_FLUSH], TaskPlatform::Windows).unwrap();
    let plan = build_maintenance_plan(&tasks, TaskPlatform::Windows, "mp_1", now(), DIGEST).unwrap();
    assert_eq!(plan.body.plan_kind, PlanKind::Maintenance);
    assert!(!plan.body.policy_matchable, "never preauthorized");
    assert!(plan.body.requires_admin, "TRIM needs an administrator");
    assert_eq!(plan.body.rule_set_digest, catalog_digest(TaskPlatform::Windows));
    // Catalog order, not argument order.
    let order: Vec<&str> = plan.body.items.iter().map(|a| match a {
        PlanAction::MaintenanceTask(t) => t.task_id.as_str(),
        _ => unreachable!(),
    }).collect();
    assert_eq!(order, [ids::DNS_FLUSH, ids::TRIM]);
    validate(&plan.body).unwrap();
    check_plan_v2(&plan, now() + 1).unwrap();
    let user_only = build_maintenance_plan(&resolve_tasks(&[ids::DNS_FLUSH], TaskPlatform::Windows).unwrap(), TaskPlatform::Windows, "mp_2", now(), DIGEST).unwrap();
    assert!(!user_only.body.requires_admin);
    // A stored action naming another version or admin flag is skipped, never run.
    let PlanAction::MaintenanceTask(a) = &plan.body.items[1] else { unreachable!() };
    assert!(planned_task(a, TaskPlatform::Windows).is_ok());
    let old = MaintenanceTaskAction { task_version: "0.0.1".into(), ..a.clone() };
    assert_eq!(planned_task(&old, TaskPlatform::Windows).unwrap_err().0, "task_version_changed");
    let flipped = MaintenanceTaskAction { requires_admin: false, ..a.clone() };
    assert_eq!(planned_task(&flipped, TaskPlatform::Windows).unwrap_err().0, "task_version_changed");
    let unknown = MaintenanceTaskAction { task_id: "windows.registry-clean".into(), ..a.clone() };
    assert_eq!(planned_task(&unknown, TaskPlatform::Windows).unwrap_err().0, "task_unknown");
}

#[test]
fn mnt02_03_probe_maps_to_the_six_outcomes() {
    let t = find(ids::TRIM).unwrap();
    let cases = [
        (Probe::NotNeeded { code: "not_due".into(), summary: "last optimized 2026-10-01".into() }, TaskOutcome::Unchanged, "not_due"),
        (Probe::NotApplicable { code: "no_ssd".into(), reason: "no SSD".into() }, TaskOutcome::Skipped, "no_ssd"),
        (Probe::Unavailable { code: "api_missing".into(), reason: "no API".into() }, TaskOutcome::Unavailable, "api_missing"),
        (Probe::Unknown { reason: "cannot read".into() }, TaskOutcome::Skipped, "state_unknown"),
    ];
    for (probe, outcome, code) in cases {
        let b = Scripted(vec![(ids::TRIM, probe, Execution::new(TaskOutcome::Failed, "must_not_run", "x"))]);
        let r = run_task(&b, t, true, &now);
        assert_eq!((r.outcome, r.code.as_str()), (outcome, code));
        assert!(!r.reason.is_empty());
    }
    for (outcome, code) in [(TaskOutcome::Applied, "retrimmed"), (TaskOutcome::Attention, "repairable"), (TaskOutcome::Failed, "exit_1")] {
        let b = Scripted(vec![(ids::TRIM, Probe::Applicable { detail: String::new() }, Execution::new(outcome, code, "summary"))]);
        let r = run_task(&b, t, true, &now);
        assert_eq!((r.outcome, r.code.as_str(), r.elevated), (outcome, code, true));
    }
}

#[test]
fn mnt03_counts_and_all_clear() {
    let mk = |o| TaskRun::not_run("t", "1", false, o, "c", "r", "2026-10-15T00:00:00Z");
    let runs: Vec<TaskRun> = TaskOutcome::ALL.iter().map(|o| mk(*o)).collect();
    let c = OutcomeCounts::of(&runs);
    assert_eq!((c.applied, c.unchanged, c.skipped, c.unavailable, c.attention, c.failed), (1, 1, 1, 1, 1, 1));
    assert!(!c.all_clear());
    assert!(OutcomeCounts::of(&[mk(TaskOutcome::Applied), mk(TaskOutcome::Skipped), mk(TaskOutcome::Unchanged)]).all_clear());
    assert!(!OutcomeCounts::of(&[mk(TaskOutcome::Applied), mk(TaskOutcome::Attention)]).all_clear());
    let wire = serde_json::to_value(c).unwrap();
    assert_eq!(wire.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>(), TaskOutcome::ALL.iter().map(|o| o.as_str().to_owned()).collect());
}

fn grant() -> WorkerGrant {
    WorkerGrant { plan_digest: DIGEST.into(), tasks: vec![TaskRef { task_id: ids::TRIM.into(), task_version: "1.0.0".into() }] }
}

fn run_line(id: &str) -> String {
    serde_json::to_string(&WorkerRequest::Run { plan_digest: DIGEST.into(), task_id: id.into(), task_version: "1.0.0".into() }).unwrap()
}

fn fake_run(spec: &'static TaskSpec) -> TaskRun {
    TaskRun::not_run(spec.id, spec.version, true, TaskOutcome::Unchanged, "fake", "fake", "2026-10-15T00:00:00Z")
}

#[test]
fn exe08_worker_grant_accepts_only_catalog_admin_tasks() {
    grant().validate(TaskPlatform::Windows).unwrap();
    let args = grant().to_args();
    // `parse` checks against this machine's catalog: the elevated worker exists on Windows only, and macOS has no
    // maintenance catalog in this build, so a grant can never reach a worker there.
    match TaskPlatform::current() {
        TaskPlatform::Windows => assert_eq!(WorkerGrant::parse(&args[1], &args[3]).unwrap(), grant()),
        TaskPlatform::Macos => {
            assert!(catalog(TaskPlatform::Macos).is_empty());
            assert!(WorkerGrant::parse(&args[1], &args[3]).unwrap_err().contains("not in the catalog"));
        }
    }
    let bad = |g: WorkerGrant| g.validate(TaskPlatform::Windows).unwrap_err();
    bad(WorkerGrant { plan_digest: "ABC".into(), ..grant() });
    bad(WorkerGrant { tasks: vec![], ..grant() });
    bad(WorkerGrant { tasks: vec![TaskRef { task_id: ids::DNS_FLUSH.into(), task_version: "1.0.0".into() }], ..grant() });
    bad(WorkerGrant { tasks: vec![TaskRef { task_id: "cmd.exe /c whoami".into(), task_version: "1.0.0".into() }], ..grant() });
    bad(WorkerGrant { tasks: vec![TaskRef { task_id: ids::TRIM.into(), task_version: "9.9.9".into() }], ..grant() });
    bad(WorkerGrant { tasks: [grant().tasks.clone(), grant().tasks].concat(), ..grant() });
    assert!(WorkerGrant::parse(DIGEST, "windows.trim-when-due").is_err());
}

#[test]
fn exe08_mnt04_worker_refuses_commands_foreign_tasks_and_other_plans() {
    let refused = |line: &str| {
        let mut s = WorkerSession::new(grant(), TaskPlatform::Windows).unwrap();
        let mut ran = false;
        let (reply, go_on) = s.handle(line, &mut |spec| {
            ran = true;
            fake_run(spec)
        });
        assert!(!ran, "{line}: nothing may run");
        assert!(!go_on, "{line}: a refusal ends the session");
        match reply {
            WorkerReply::Refused { code, .. } => code,
            other => panic!("{line}: {other:?}"),
        }
    };
    // Arbitrary commands and scripts: not a defined request.
    assert_eq!(refused(r#"{"op":"exec","command":"cmd.exe /c del C:\\"}"#), "malformed_request");
    assert_eq!(refused(r#"{"op":"run","plan_digest":"x","task_id":"windows.trim-when-due","task_version":"1.0.0","argv":["/c"]}"#), "malformed_request");
    assert_eq!(refused("powershell -enc AAAA"), "malformed_request");
    assert_eq!(refused(&"x".repeat(WORKER_MAX_LINE + 1)), "line_too_long");
    // A catalog task that is not an administrator task of this plan, and a task that is not in the catalog.
    assert_eq!(refused(&run_line(ids::IMAGE_HEALTH)), "task_not_granted");
    assert_eq!(refused(&run_line(ids::DNS_FLUSH)), "task_not_granted");
    assert_eq!(refused(&run_line("windows.registry-clean")), "task_not_granted");
    // Another plan's digest.
    let other = serde_json::to_string(&WorkerRequest::Run { plan_digest: "f".repeat(64), task_id: ids::TRIM.into(), task_version: "1.0.0".into() }).unwrap();
    assert_eq!(refused(&other), "wrong_plan");
    // A granted task runs once; a repeat is refused.
    let mut s = WorkerSession::new(grant(), TaskPlatform::Windows).unwrap();
    let (reply, go_on) = s.handle(&run_line(ids::TRIM), &mut fake_run);
    assert!(matches!(reply, WorkerReply::Result { ref run } if run.task_id == ids::TRIM) && go_on);
    let (reply, go_on) = s.handle(&run_line(ids::TRIM), &mut fake_run);
    assert!(matches!(reply, WorkerReply::Refused { ref code, .. } if code == "already_run") && !go_on);
    let mut s = WorkerSession::new(grant(), TaskPlatform::Windows).unwrap();
    assert_eq!(s.handle(r#"{"op":"done"}"#, &mut fake_run), (WorkerReply::Bye, false));
}

#[test]
fn mnt06_diagnosis_lists_findings_and_never_acts() {
    let p = |pid, name: &str, window, lifetime, age| ProcessSample {
        pid,
        name: name.into(),
        image: Some(format!("C:\\Apps\\{name}")),
        window_core_percent: window,
        lifetime_core_percent: lifetime,
        age_seconds: age,
        working_set_bytes: 2 << 30,
    };
    let input = DiagnosisInput {
        window_seconds: 2.0,
        processes: vec![p(0, "Idle", 700.0, 700.0, 99_999), p(10, "spin.exe", 99.0, 97.0, 3_600), p(11, "build.exe", 95.0, 10.0, 30), p(12, "quiet.exe", 1.0, 1.0, 9_999), p(13, "vmmem", 0.5, 3.0, 7_200)],
        memory_load_percent: Some(91.0),
        commit_used_percent: Some(95.0),
        volumes: vec![VolumeSpace { display: "C:".into(), free_bytes: 5 << 30, total_bytes: 500 << 30 }, VolumeSpace { display: "D:".into(), free_bytes: 400 << 30, total_bytes: 500 << 30 }],
        startup_items: Some((17, 30)),
        attached_images: Some(vec!["D:\\iso\\setup.iso".into()]),
    };
    let f = diagnose(&input);
    let codes: Vec<&str> = f.iter().map(|x| x.code).collect();
    assert_eq!(codes, ["runaway_process", "busy_process", "memory_pressure", "commit_pressure", "low_disk_space", "idle_virtual_machine", "attached_disk_image", "many_startup_items"]);
    let runaway = &f[0];
    assert_eq!((runaway.process_id, runaway.duration_seconds, runaway.subject.as_str()), (Some(10), Some(3_600), "spin.exe"));
    assert!(runaway.detail.contains("C:\\Apps\\spin.exe") && runaway.detail.contains("not ended"));
    assert_eq!(runaway.next_step, "status.processes");
    assert_eq!(f[4].subject, "C:");
    assert!(diagnose(&DiagnosisInput::default()).is_empty());
}
