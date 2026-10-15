//! Strict answer parsing, sanitizing, grounding against core facts and the core-owned hand-off
//! (AID-04/05/08/09/10). Provider text is untrusted: it is parsed into one fixed shape, command
//! and approval text is removed, every claim is checked against the evidence package, and the
//! recommendation can only become more conservative. Next steps come from [`handoff`], which
//! reads core facts only; model text can label a step as "suggested", never enable one.

use std::collections::BTreeSet;

use super::{
    ANSWER_VERSION, Basis, ClaimCheck, ClaimStatus, Conflict, ConflictCode, CoreFacts, DiagError, DiagnosisRequest, DiagnosisResult, EvidencePackage,
    Freshness, FreshnessState, Handoff, HandoffTarget, Layer, ModelAnswer, NextAction, Recommendation, SanitizeReason, Sanitized, budget,
};

/// Parse provider text into the one accepted shape. A single surrounding ```json fence is
/// tolerated; anything else (prose, several objects, unknown fields, numbers, duplicate keys,
/// another version) is `invalid_structure` and nothing in it is used.
pub fn parse_answer(text: &str) -> Result<ModelAnswer, DiagError> {
    if text.len() > budget::RAW_ANSWER_BYTES {
        return Err(DiagError::InvalidAnswer("answer exceeds the output budget".into()));
    }
    let mut t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        t = rest.strip_suffix("```").ok_or_else(|| DiagError::InvalidAnswer("unterminated code fence".into()))?.trim();
    }
    if !t.starts_with('{') {
        return Err(DiagError::InvalidAnswer("answer is not a single JSON object".into()));
    }
    let value = crate::c14n::strict_parse(t).map_err(|e| DiagError::InvalidAnswer(e.to_string()))?;
    let answer: ModelAnswer = serde_json::from_value(value).map_err(|e| DiagError::InvalidAnswer(e.to_string()))?;
    if answer.answer_version != ANSWER_VERSION {
        return Err(DiagError::InvalidAnswer("unsupported answer_version".into()));
    }
    if answer.summary.trim().is_empty() || answer.purpose.trim().is_empty() {
        return Err(DiagError::InvalidAnswer("summary and purpose are required".into()));
    }
    Ok(answer)
}

/// Bounds of a stored answer (after sanitizing every field fits).
pub fn check_bounds(a: &ModelAnswer) -> Result<(), String> {
    let n = |s: &str| s.chars().count();
    let ok = a.answer_version == ANSWER_VERSION
        && n(&a.summary) <= budget::SUMMARY_CHARS
        && n(&a.purpose) <= budget::PARAGRAPH_CHARS
        && n(&a.deletion_impact) <= budget::PARAGRAPH_CHARS
        && n(&a.rebuild_cost) <= budget::PARAGRAPH_CHARS
        && a.conditions.len() <= budget::LIST_ITEMS
        && a.conditions.iter().all(|c| n(c) <= budget::SHORT_CHARS)
        && a.gaps.len() <= budget::LIST_ITEMS
        && a.gaps.iter().all(|c| n(c) <= budget::SHORT_CHARS)
        && a.claims.len() <= budget::CLAIMS
        && a.claims.iter().all(|c| n(&c.text) <= budget::SHORT_CHARS && c.refs.len() <= budget::REFS_PER_CLAIM)
        && a.alternatives.len() <= budget::ALTERNATIVES
        && a.alternatives.iter().all(|x| n(&x.text) <= budget::SHORT_CHARS)
        && a.questions.len() <= budget::QUESTIONS
        && a.questions.iter().all(|q| n(&q.text) <= budget::SHORT_CHARS && q.choices.len() <= budget::CHOICES && q.choices.iter().all(|c| n(c) <= budget::CHOICE_CHARS));
    if ok { Ok(()) } else { Err("answer field over budget".into()) }
}

const COMMAND_MARKERS: &[&str] = &[
    "```", "rm -", "rm /", "rmdir", "rd /s", "del /", "del \"", "erase /", "remove-item", "move-item", "format c", "diskpart", "sudo ", "mkfs", "dd if=",
    "shred ", "cipher /w", "reg delete", "vssadmin", "powershell", "pwsh", "cmd /c", "cmd.exe", "bash -c", "| sh", "curl ", "wget ", "chmod ", "chown ",
    "$(", "invoke-expression", "iex ", "start-process", "robocopy", "xcopy", "trash-put", "cleanmanager plan", "cleanmanager clean", "cleanmanager apply",
];
const APPROVAL_MARKERS: &[&str] = &[
    "auto-approve", "auto approve", "automatically approve", "already approved", "is approved", "pre-approved", "authorized to delete", "已批准", "已授权", "自动批准",
    "无需确认", "跳过确认", "不用确认",
];
const SAFE_MARKERS: &[&str] = &[
    "safe to delete", "safely delete", "safe to remove", "no risk", "完全安全", "安全删除", "可以安全", "放心删除", "放心删", "删除没有风险", "无风险", "没有任何风险",
];
const HEDGES: &[&str] = &["未知", "不确定", "无法确认", "无法判断", "未检测", "未核验", "不知道", "是否", "请确认", "需要确认", "需确认", "unknown", "not verified", "unverified", "whether", "if "];
const BACKUP: &[&str] = &["备份", "backed up", "backup"];
const USAGE: &[&str] = &["未被使用", "没有被使用", "不再使用", "没有程序使用", "没有程序在使用", "没有在用", "无人使用", "没人使用", "未在使用", "not in use", "unused", "no longer used"];
const RECLAIM: &[&str] = &["释放", "腾出", "free up", "frees", "reclaim"];

fn has(text: &str, markers: &[&str]) -> bool {
    let lower = text.to_lowercase();
    markers.iter().any(|m| lower.contains(m))
}

fn hedged(text: &str) -> bool {
    has(text, HEDGES)
}

fn bound(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        (s.to_owned(), false)
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        (out, true)
    }
}

const REMOVED_COMMAND: &str = "[已移除：回答包含命令文本；CleanManager 不执行也不复制模型给出的命令]";
const REMOVED_APPROVAL: &str = "[已移除：回答包含批准/授权说法；授权只能由你在计划审阅中完成]";
const REMOVED_SAFE: &str = "[已移除：一句未经核验的安全性断言]";

/// One text field through the sanitizer; returns the cleaned text.
fn clean(field: &str, text: &str, max: usize, out: &mut Vec<Sanitized>) -> String {
    let mut s = text.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect::<String>();
    if has(&s, COMMAND_MARKERS) {
        out.push(Sanitized { field: field.into(), reason: SanitizeReason::CommandText });
        s = REMOVED_COMMAND.into();
    } else if has(&s, APPROVAL_MARKERS) {
        out.push(Sanitized { field: field.into(), reason: SanitizeReason::ApprovalInstruction });
        s = REMOVED_APPROVAL.into();
    } else if has(&s, SAFE_MARKERS) && !hedged(&s) {
        out.push(Sanitized { field: field.into(), reason: SanitizeReason::UnverifiedSafetyClaim });
        s = REMOVED_SAFE.into();
    }
    let (b, cut) = bound(&s, max);
    if cut {
        out.push(Sanitized { field: field.into(), reason: SanitizeReason::Truncated });
    }
    b
}

fn cap<T>(v: &mut Vec<T>, max: usize, field: &str, out: &mut Vec<Sanitized>) {
    if v.len() > max {
        v.truncate(max);
        out.push(Sanitized { field: field.into(), reason: SanitizeReason::DroppedItem });
    }
}

/// Remove command/approval/unverified-safety text, enforce budgets and drop references that
/// are not in the package. Never adds anything.
pub fn sanitize(mut a: ModelAnswer, package: &EvidencePackage) -> (ModelAnswer, Vec<Sanitized>) {
    let mut out = Vec::new();
    a.summary = clean("summary", &a.summary, budget::SUMMARY_CHARS, &mut out);
    a.purpose = clean("purpose", &a.purpose, budget::PARAGRAPH_CHARS, &mut out);
    a.deletion_impact = clean("deletion_impact", &a.deletion_impact, budget::PARAGRAPH_CHARS, &mut out);
    a.rebuild_cost = clean("rebuild_cost", &a.rebuild_cost, budget::PARAGRAPH_CHARS, &mut out);
    cap(&mut a.conditions, budget::LIST_ITEMS, "conditions", &mut out);
    for (i, c) in a.conditions.iter_mut().enumerate() {
        *c = clean(&format!("conditions[{i}]"), c, budget::SHORT_CHARS, &mut out);
    }
    cap(&mut a.gaps, budget::LIST_ITEMS, "gaps", &mut out);
    for (i, c) in a.gaps.iter_mut().enumerate() {
        *c = clean(&format!("gaps[{i}]"), c, budget::SHORT_CHARS, &mut out);
    }
    cap(&mut a.claims, budget::CLAIMS, "claims", &mut out);
    for (i, c) in a.claims.iter_mut().enumerate() {
        c.text = clean(&format!("claims[{i}].text"), &c.text, budget::SHORT_CHARS, &mut out);
        let before = c.refs.len();
        let mut seen = BTreeSet::new();
        c.refs.retain(|r| package.has_ref(r) && seen.insert(r.clone()));
        if c.refs.len() != before {
            out.push(Sanitized { field: format!("claims[{i}].refs"), reason: SanitizeReason::UnknownReference });
        }
        cap(&mut c.refs, budget::REFS_PER_CLAIM, &format!("claims[{i}].refs"), &mut out);
    }
    cap(&mut a.alternatives, budget::ALTERNATIVES, "alternatives", &mut out);
    for (i, x) in a.alternatives.iter_mut().enumerate() {
        x.text = clean(&format!("alternatives[{i}].text"), &x.text, budget::SHORT_CHARS, &mut out);
    }
    cap(&mut a.questions, budget::QUESTIONS, "questions", &mut out);
    for (i, q) in a.questions.iter_mut().enumerate() {
        // Question ids are reassigned so a provider cannot reference a previous object's thread.
        q.question_id = format!("q{}", i + 1);
        q.text = clean(&format!("questions[{i}].text"), &q.text, budget::SHORT_CHARS, &mut out);
        cap(&mut q.choices, budget::CHOICES, &format!("questions[{i}].choices"), &mut out);
        for (j, c) in q.choices.iter_mut().enumerate() {
            *c = clean(&format!("questions[{i}].choices[{j}]"), c, budget::CHOICE_CHARS, &mut out);
        }
    }
    a.questions.retain(|q| !q.text.trim().is_empty());
    (a, out)
}

fn layer_of(package: &EvidencePackage, id: &str) -> Option<Layer> {
    package.item(id).map(|i| i.layer)
}

fn conflict(conflicts: &mut Vec<Conflict>, code: ConflictCode, text: &str) {
    if !conflicts.iter().any(|c| c.code == code) {
        conflicts.push(Conflict { code, text: text.into() });
    }
}

/// Check every claim against the package and the core facts, then derive the effective
/// recommendation (AID-03/04/05). Facts always win; a claim never unlocks anything.
pub fn ground(answer: ModelAnswer, package: &EvidencePackage, facts: &CoreFacts) -> DiagnosisResult {
    let model_recommendation = answer.recommendation;
    let (answer, sanitized) = sanitize(answer, package);
    let mut conflicts = Vec::new();
    let mut grounding = Vec::new();
    let no_rule = package.has_code("rule.none");
    for (i, c) in answer.claims.iter().enumerate() {
        let layers: Vec<Layer> = c.refs.iter().filter_map(|r| layer_of(package, r)).collect();
        let (mut status, mut note) = match c.basis {
            Basis::Observation if layers.iter().any(|l| matches!(l, Layer::Observation | Layer::Protection | Layer::Ownership | Layer::Coverage)) => (ClaimStatus::Supported, None),
            Basis::Rule if layers.contains(&Layer::Rule) => (ClaimStatus::Supported, None),
            Basis::UserStatement if layers.contains(&Layer::UserStatement) => (ClaimStatus::UserStatement, Some("你的陈述，CleanManager 未核验".to_owned())),
            Basis::Inference => (ClaimStatus::Inference, Some("模型推测".to_owned())),
            _ => (ClaimStatus::Unsupported, Some("没有引用到支持它的本机证据".to_owned())),
        };
        let user = c.basis == Basis::UserStatement;
        let text = &c.text;
        if has(text, BACKUP) && !hedged(text) && !user {
            status = ClaimStatus::Unsupported;
            note = Some("CleanManager 没有检测备份".into());
            conflict(&mut conflicts, ConflictCode::UnverifiedBackupClaim, "回答声称已有备份，但本机证据中没有经过核验的备份。");
        }
        if has(text, USAGE) && !hedged(text) && !user {
            status = ClaimStatus::Unsupported;
            note = Some("CleanManager 没有检测使用状态".into());
            conflict(&mut conflicts, ConflictCode::UnverifiedUsageClaim, "回答声称对象未被使用，但本机没有这项检测。");
        }
        if has(text, RECLAIM) && text.chars().any(|ch| ch.is_ascii_digit()) && !hedged(text) {
            status = ClaimStatus::Unsupported;
            note = Some("可释放空间未知".into());
            conflict(&mut conflicts, ConflictCode::UnverifiedReclaimClaim, "回答给出了可释放空间，但独占回收量未知。");
        }
        if c.basis == Basis::Rule && no_rule {
            status = ClaimStatus::ConflictsWithFacts;
            note = Some("本机没有规则匹配这个对象".into());
            conflict(&mut conflicts, ConflictCode::RuleClaimWithoutRule, "回答引用了清理规则，但没有已启用规则匹配该对象。");
        }
        if facts.protection.is_some() && has(text, &["可以删除", "可删除", "可以清理", "can be deleted", "can delete", "disposable", "可丢弃"]) && !hedged(text) {
            status = ClaimStatus::ConflictsWithFacts;
            note = Some("该对象受保护".into());
        }
        grounding.push(ClaimCheck { index: i as u32, status, note });
    }
    if sanitized.iter().any(|s| s.reason == SanitizeReason::UnverifiedSafetyClaim) {
        conflict(&mut conflicts, ConflictCode::SafeToDeleteClaim, "回答给出了未经核验的安全性断言；CleanManager 不给出安全许可，相关句子已移除。");
    }

    let mut eff = model_recommendation;
    if let Some(p) = &facts.protection {
        if eff == Recommendation::Reviewable {
            conflict(&mut conflicts, ConflictCode::ProtectedObject, &format!("对象受保护（{p}）；保护优先于任何建议。"));
        }
        eff = eff.max(Recommendation::Keep);
    }
    if eff == Recommendation::Reviewable {
        if facts.candidate_id.is_none() {
            conflict(&mut conflicts, ConflictCode::NotACandidate, "没有已启用规则把它列为清理候选，核心无法为它生成计划。");
            eff = Recommendation::NeedsContext;
        } else if !facts.plannable {
            conflict(&mut conflicts, ConflictCode::CandidateNotPlannable, "核心当前不允许把该候选放入计划（层级、探测或证据不足）。");
            eff = Recommendation::NeedsContext;
        }
        if facts.partial_evidence || facts.cloud_placeholder || !facts.protection_checked {
            conflict(&mut conflicts, ConflictCode::PartialEvidence, "证据不完整（未读子范围、云端占位、弱身份或保护层未核对）。");
            eff = Recommendation::NeedsContext;
        }
        if grounding.iter().any(|g| matches!(g.status, ClaimStatus::ConflictsWithFacts)) || !conflicts.is_empty() {
            eff = eff.max(Recommendation::NeedsContext);
        }
    }
    DiagnosisResult { answer, model_recommendation, effective_recommendation: eff, grounding, conflicts, sanitized }
}

/// What the latest index says about the bound object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentObservation {
    Same,
    Changed,
    Missing,
    NotChecked,
}

/// Freshness of a diagnosis (AID-10): the validity window and the latest observation.
pub fn freshness(expires_at: &str, now_unix: i64, current: CurrentObservation, now: &str) -> Freshness {
    let mut reasons = Vec::new();
    if crate::plan::parse_utc(expires_at).is_none_or(|t| now_unix >= t) {
        reasons.push("result_window_elapsed".to_owned());
    }
    match current {
        CurrentObservation::Changed => reasons.push("observation_changed".into()),
        CurrentObservation::Missing => reasons.push("object_not_in_latest_index".into()),
        CurrentObservation::Same | CurrentObservation::NotChecked => {}
    }
    let state = if !reasons.is_empty() {
        FreshnessState::Stale
    } else if current == CurrentObservation::NotChecked {
        FreshnessState::Unknown
    } else {
        FreshnessState::Current
    };
    Freshness { state, checked_at: now.into(), reasons }
}

/// Next steps, decided by core facts only (AID-09). `completed_fresh`: a completed diagnosis
/// whose freshness is current. A plan hand-off names exactly one existing candidate.
pub fn handoff(request: &DiagnosisRequest, facts: &CoreFacts, answer: Option<&ModelAnswer>, completed_fresh: bool) -> Vec<Handoff> {
    let suggested: BTreeSet<NextAction> = answer.map(|a| a.alternatives.iter().map(|x| x.action).collect()).unwrap_or_default();
    let target = |candidate: bool, app: bool| HandoffTarget {
        scan_id: request.object.scan_id.clone(),
        entry_id: facts.entry_id.clone(),
        candidate_id: if candidate { facts.candidate_id.clone() } else { None },
        app_instance_id: if app { facts.app_instance_ids.first().cloned() } else { None },
    };
    let hard = facts.protection.as_deref().is_some_and(|p| p.starts_with("hard_deny"));
    let user_protected = facts.protection.as_deref().is_some_and(|p| p.starts_with("user_protected"));
    let (plan_ok, plan_reason) = if let Some(p) = &facts.protection {
        (false, format!("受保护（{p}），核心不会为它生成计划；可以保留或定位。"))
    } else if facts.candidate_id.is_none() {
        (false, "没有已启用规则的候选；本扩展不开放任意路径删除。".to_owned())
    } else if !facts.plannable {
        (false, "核心当前不允许把该候选放入计划。".to_owned())
    } else if !facts.protection_checked {
        (false, "保护层未全部核对，处理入口保持关闭。".to_owned())
    } else if !completed_fresh {
        (false, "诊断未完成或已过期；请先重新核验。".to_owned())
    } else {
        (true, "生成只含这一个候选的新计划；授权与现场核验在计划页独立进行。".to_owned())
    };
    let mut out = vec![
        Handoff { action: NextAction::ReviewPlan, available: plan_ok, reason: plan_reason, suggested_by_model: false, target: plan_ok.then(|| target(true, false)) },
        Handoff { action: NextAction::Keep, available: true, reason: "不做任何操作，文件保持原状。".into(), suggested_by_model: false, target: None },
        Handoff {
            action: NextAction::Protect,
            available: !hard && !user_protected,
            reason: if hard {
                "已受系统硬性保护。".into()
            } else if user_protected {
                "已在你的保护名单中。".into()
            } else {
                "加入你的保护名单；以后的规则与策略都不会选中它。".into()
            },
            suggested_by_model: false,
            target: Some(target(false, false)),
        },
        Handoff { action: NextAction::Locate, available: true, reason: "在磁盘视图中定位（只读）。".into(), suggested_by_model: false, target: Some(target(false, false)) },
        Handoff { action: NextAction::Verify, available: true, reason: "按核实清单确认备份、引用与来源。".into(), suggested_by_model: false, target: None },
        Handoff {
            action: NextAction::AppManagement,
            available: !facts.app_instance_ids.is_empty(),
            reason: if facts.app_instance_ids.is_empty() { "没有安装记录关联到这个位置。".into() } else { "在应用页查看关联应用（只读清单）。".into() },
            suggested_by_model: false,
            target: (!facts.app_instance_ids.is_empty()).then(|| target(false, true)),
        },
        Handoff { action: NextAction::Archive, available: false, reason: "CleanManager 不移动或归档文件；请使用你自己的备份工具。".into(), suggested_by_model: false, target: None },
    ];
    for h in &mut out {
        h.suggested_by_model = suggested.contains(&h.action);
    }
    out
}

/// Minimal verification checklist from the package gaps (shown with the `verify` step).
pub fn checklist(package: &EvidencePackage) -> Vec<String> {
    package.gaps.iter().map(|g| g.text.clone()).collect()
}
