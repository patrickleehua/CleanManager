//! Redacted, bounded evidence for one object (AID-02/03/07). Built only from index metadata,
//! the candidate set, protection layers and the read-only app inventory: no file is opened, no
//! directory is listed beyond what the index already holds, and cloud placeholders are never
//! read. What is collected depends on the object's id and the person's disclosure choices only;
//! names are data and cannot widen collection (a name is redacted, truncated or omitted, never
//! interpreted).

use std::collections::BTreeSet;

use serde_json::json;

use super::{
    ALWAYS_EXCLUDED, ChildSummary, CoreFacts, DIAG_CONTRACT, DiagError, DiagnosisRequest, Disclosure, EVIDENCE_VERSION, EvidenceItem, EvidencePackage,
    Gap, Layer, ObjectRef, Origin, Redaction, RedactionRule, Subject, UserStatement, budget,
};
use crate::analysis::{Candidate, CandidateState, CatalogFacts, IndexView, ProtectionClass, UserList, plannable, protection_by_name};
use crate::bytes::{Bytes, KnownTotal};
use crate::c14n::digest;
use crate::capability::Platform;
use crate::model::{CloudState, IdentityQuality, ObjectKind};
use crate::path::RawPath;
use crate::protect::{HardDenySet, ObjectIdentity, prefilter};
use crate::rules::RebuildClass;

/// Upper bound on index nodes visited to summarize a directory (counts only, never disclosed).
pub const MAX_WALK: usize = 500_000;

/// Scan facts the evidence is bound to.
#[derive(Debug, Clone)]
pub struct ScanFacts<'a> {
    pub scan_id: &'a str,
    pub revision: u32,
    pub partial: bool,
    /// Scan finish time: the observation time of every fact in the package.
    pub observed_at: &'a str,
}

/// The candidate of this exact entry in the scan's latest candidate set, if any.
#[derive(Debug, Clone, Copy)]
pub struct CandidateFacts<'a> {
    pub candidate: &'a Candidate,
    pub catalog: Option<&'a CatalogFacts>,
}

/// Protection layers 1 and 3 as the caller could load them; layer 2 (built-in classes) is
/// evaluated here from the index.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProtectionInput<'a> {
    pub hard_deny: Option<&'a HardDenySet>,
    pub user: UserList<'a>,
}

/// Read-only ownership facts from the application inventory (APP-02/03).
#[derive(Debug, Clone, Default)]
pub struct OwnershipFacts {
    pub checked: bool,
    /// (`app_instance_id`, display name) of instances whose recorded location is or contains the object.
    pub apps: Vec<(String, String)>,
    pub shared: bool,
}

pub struct EvidenceInput<'a> {
    pub view: &'a dyn IndexView,
    pub entry: u64,
    pub origin: Origin,
    pub scan: ScanFacts<'a>,
    /// `None` when the scan has no candidate set yet (reported as a gap).
    pub candidates: Option<&'a [CandidateFacts<'a>]>,
    pub protection: ProtectionInput<'a>,
    pub ownership: OwnershipFacts,
    /// The person's profile directory, replaced by a placeholder in default locations.
    pub home: Option<&'a RawPath>,
    pub disclosure: Disclosure,
    pub question: Option<String>,
    pub statements: Vec<UserStatement>,
    pub parent_diagnosis_id: Option<String>,
}

pub struct Built {
    pub request: DiagnosisRequest,
    pub package: EvidencePackage,
    pub facts: CoreFacts,
}

// ---------------------------------------------------------------------------------------------
// Redaction

/// Phrases that try to instruct a model or widen collection; a name carrying one is omitted.
const INSTRUCTION_LIKE: &[&str] = &[
    "ignore previous", "ignore all", "ignore the above", "disregard", "system prompt", "you are now", "assistant:", "system:", "developer:",
    "approve", "auto-approve", "include the contents", "send the contents", "upload", "rm -rf", "del /", "remove-item", "format c:",
    "忽略", "无视", "指令", "提示词", "批准", "授权", "上传", "发送内容", "全部删除", "自动删除",
];

fn is_bidi_or_control(c: char) -> bool {
    c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

pub fn looks_like_instruction(s: &str) -> bool {
    let lower = s.to_lowercase();
    INSTRUCTION_LIKE.iter().any(|p| lower.contains(p))
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.' | '@' | '=')
}

fn redact_token(t: &str, rules: &mut BTreeSet<RedactionRule>) -> String {
    if let Some((local, domain)) = t.split_once('@')
        && !local.is_empty()
        && domain.contains('.')
    {
        rules.insert(RedactionRule::Email);
        return "[email]".into();
    }
    let letters = t.chars().any(|c| c.is_ascii_alphabetic());
    let digits = t.chars().any(|c| c.is_ascii_digit());
    let lower = t.to_ascii_lowercase();
    let prefixed = ["sk-", "sk_", "ghp_", "gho_", "xoxb-", "xoxp-", "akia", "eyj"].iter().any(|p| lower.starts_with(p)) && t.len() >= 16;
    if prefixed || (t.len() >= 24 && letters && digits) {
        rules.insert(RedactionRule::SecretLike);
        return "[secret]".into();
    }
    // Runs of six or more digits (account, phone or ID numbers) become a marker.
    let mut out = String::new();
    let mut run = String::new();
    for c in t.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            run.push(c);
            continue;
        }
        if run.len() >= 6 {
            rules.insert(RedactionRule::LongNumber);
            out.push_str("[number]");
        } else {
            out.push_str(&run);
        }
        run.clear();
        if c != '\0' {
            out.push(c);
        }
    }
    out
}

fn truncate(s: &str, max: usize, rules: &mut BTreeSet<RedactionRule>) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    rules.insert(RedactionRule::Truncated);
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Remove control and bidi characters (they could rewrite the surrounding text) and collapse
/// whitespace. Used for every name, even under full-path disclosure.
pub fn sanitize(s: &str, rules: &mut BTreeSet<RedactionRule>) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if is_bidi_or_control(c) {
            rules.insert(RedactionRule::ControlCharacters);
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Default redaction of a free text (names, app names): control characters, e-mail
/// addresses, secret-like tokens and long numbers; instruction-like text is omitted whole.
pub fn redact_text(s: &str, max: usize, rules: &mut BTreeSet<RedactionRule>) -> String {
    let clean = sanitize(s, rules);
    if looks_like_instruction(&clean) {
        rules.insert(RedactionRule::InstructionLike);
        return "[已省略：疑似指令文本]".into();
    }
    let mut out = String::new();
    let mut tok = String::new();
    for c in clean.chars().chain(std::iter::once('\u{0}')) {
        if token_char(c) {
            tok.push(c);
            continue;
        }
        if !tok.is_empty() {
            out.push_str(&redact_token(&tok, rules));
            tok.clear();
        }
        if c != '\u{0}' {
            out.push(c);
        }
    }
    truncate(&out, max, rules)
}

fn split_ext(name: &str) -> (&str, Option<String>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && (1..=10).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => {
            (stem, Some(ext.to_ascii_lowercase()))
        }
        _ => (name, None),
    }
}

fn comparable(s: &str, platform: Platform) -> String {
    match platform {
        Platform::Windows => s.replace('/', "\\").to_lowercase(),
        Platform::Macos => s.to_owned(),
    }
}

/// Redacted parent location: profile prefix replaced, at most three trailing components, each
/// component redacted like a name.
fn default_location(parent: &str, platform: Platform, home: Option<&RawPath>, rules: &mut BTreeSet<RedactionRule>) -> String {
    let sep = if platform == Platform::Windows { '\\' } else { '/' };
    let (prefix, rest) = match home.map(|h| h.lossy()) {
        Some(h) if !h.is_empty() => {
            let (hp, pp) = (comparable(h.trim_end_matches(['\\', '/']), platform), comparable(parent, platform));
            if pp == hp || pp.starts_with(&format!("{hp}{sep}")) {
                rules.insert(RedactionRule::UserProfile);
                let marker = if platform == Platform::Windows { "%USERPROFILE%" } else { "~" };
                (marker.to_owned(), parent.chars().skip(hp.chars().count()).collect::<String>())
            } else {
                (String::new(), parent.to_owned())
            }
        }
        _ => (String::new(), parent.to_owned()),
    };
    let comps: Vec<&str> = rest.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    let (lead, keep) = if prefix.is_empty() && !comps.is_empty() {
        // Keep a drive or volume root (`C:`), which identifies no person.
        let first = comps[0];
        let root = platform == Platform::Windows && first.len() == 2 && first.ends_with(':');
        if root { (first.to_owned(), &comps[1..]) } else { (String::new(), &comps[..]) }
    } else {
        (prefix, &comps[..])
    };
    let skipped = keep.len().saturating_sub(3);
    if skipped > 0 {
        rules.insert(RedactionRule::ParentComponents);
    }
    let mut parts: Vec<String> = Vec::new();
    if !lead.is_empty() {
        parts.push(lead);
    }
    if skipped > 0 {
        parts.push("…".into());
    }
    // The account name can also appear outside a matched profile prefix (8.3 short names such as
    // `ALICE~1`, other drives): any component naming it is replaced.
    let user = home.map(|h| h.lossy()).and_then(|h| h.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().map(str::to_lowercase)).filter(|u| u.len() >= 2);
    for c in &keep[skipped..] {
        let lower = c.to_lowercase();
        let short = lower.split_once('~').is_some_and(|(stem, _)| stem.len() >= 2 && user.as_deref().is_some_and(|u| u.starts_with(stem)));
        if user.as_deref() == Some(lower.as_str()) || short {
            rules.insert(RedactionRule::UserProfile);
            parts.push("[user]".into());
        } else {
            parts.push(redact_text(c, 60, rules));
        }
    }
    let joined = parts.join(&sep.to_string());
    truncate(&joined, budget::LOCATION_CHARS, rules)
}

// ---------------------------------------------------------------------------------------------
// Facts

fn kind_str(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::File => "file",
        ObjectKind::Directory => "directory",
        ObjectKind::Symlink => "symlink",
        ObjectKind::Reparse => "reparse",
        ObjectKind::Other => "other",
    }
}

fn cloud_str(c: CloudState) -> &'static str {
    match c {
        CloudState::NotCloud => "not_cloud",
        CloudState::Hydrated => "hydrated",
        CloudState::Placeholder => "placeholder",
        CloudState::Unknown => "unknown",
    }
}

/// `1.5 GiB（1610612736 字节）`.
pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{b} 字节") } else { format!("{v:.1} {}（{b} 字节）", UNITS[u]) }
}

#[derive(Default)]
struct Walk {
    files: u64,
    dirs: u64,
    unread: u64,
    boundaries: u64,
    placeholders: u64,
    inner: BTreeSet<ProtectionClass>,
    truncated: bool,
    visited: usize,
}

/// Count a subtree through the index (no file-system access). `seen` makes hard links count once.
fn walk(view: &dyn IndexView, start: u64, include_self: bool, seen: &mut BTreeSet<u64>, w: &mut Walk) -> KnownTotal {
    let mut total = KnownTotal::default();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if w.visited >= MAX_WALK {
            w.truncated = true;
            break;
        }
        w.visited += 1;
        let f = view.info(id);
        if id != start || include_self {
            if let Some(c) = protection_by_name(view, id) {
                w.inner.insert(c);
            }
        }
        w.unread += f.skipped_here;
        for _ in 0..f.skipped_here {
            let _ = total.add(None);
        }
        if f.boundary {
            w.boundaries += 1;
            continue;
        }
        if f.cloud == CloudState::Placeholder {
            w.placeholders += 1;
        }
        if f.kind == ObjectKind::Directory {
            if id != start || include_self {
                w.dirs += 1;
            }
            stack.extend_from_slice(view.children(id));
        } else {
            w.files += 1;
            if seen.insert(f.object_id) {
                let _ = total.add(f.logical.map(Bytes));
            }
        }
    }
    total
}

/// SHA-256 over the object's index facts (identity, sizes, time, cloud, links), bound to the
/// scan revision. Compared with a later observation to mark a diagnosis stale (AID-10).
pub fn fingerprint(view: &dyn IndexView, scan_id: &str, revision: u32, entry: u64) -> String {
    let f = view.info(entry);
    let v = json!({
        "scan_id": scan_id, "revision": revision, "entry_id": entry.to_string(), "volume_id": f.volume_id, "file_id": f.file_id,
        "kind": kind_str(f.kind), "logical": f.logical.map(|b| b.to_string()), "allocated": f.allocated.map(|b| b.to_string()),
        "modified_unix": f.modified_unix.map(|t| t.to_string()), "cloud": cloud_str(f.cloud), "link_count": f.link_count, "skipped_here": f.skipped_here,
        "quality": if f.quality == IdentityQuality::Strong { "strong" } else { "weak" },
    });
    digest(&v).expect("plain JSON")
}

/// Object-only fingerprint (no scan binding): equal across scans when nothing about the object changed.
pub fn object_fingerprint(view: &dyn IndexView, entry: u64) -> String {
    let f = view.info(entry);
    let v = json!({
        "volume_id": f.volume_id, "file_id": f.file_id, "kind": kind_str(f.kind), "logical": f.logical.map(|b| b.to_string()),
        "modified_unix": f.modified_unix.map(|t| t.to_string()), "cloud": cloud_str(f.cloud), "link_count": f.link_count,
    });
    digest(&v).expect("plain JSON")
}

fn hard_deny_reason(view: &dyn IndexView, entry: u64, path: &RawPath, hard: Option<&HardDenySet>) -> Option<String> {
    if let Err(r) = prefilter(path) {
        return Some(r.to_string());
    }
    let hard = hard?;
    let id = |e: u64| {
        let f = view.info(e);
        ObjectIdentity::new(f.volume_id.clone(), f.file_id.clone())
    };
    if let Some(r) = hard.root_at(&id(entry)) {
        return Some(r.class.as_str().to_owned());
    }
    if let Some(r) = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| hard.covering(&id(a))) {
        return Some(format!("inside:{}", r.class.as_str()));
    }
    if view.info(entry).kind == ObjectKind::Directory
        && let Some(r) = crate::protect::hard_deny_within(view, entry, hard)
    {
        return Some(format!("contains:{}", r.class.as_str()));
    }
    None
}

fn rebuild_text(r: RebuildClass) -> &'static str {
    match r {
        RebuildClass::LocalRebuild => "可由所属程序在本机重建",
        RebuildClass::NetworkRedownload => "需要重新下载才能恢复",
        RebuildClass::NotRebuildable => "删除后无法重建",
    }
}

fn state_text(s: CandidateState) -> &'static str {
    match s {
        CandidateState::Allowed => "allowed",
        CandidateState::ProtectedReviewable => "protected_reviewable",
        CandidateState::PendingEvidence => "pending_evidence",
        CandidateState::Blocked => "blocked",
    }
}

struct Collector {
    items: Vec<EvidenceItem>,
    gaps: Vec<Gap>,
    rules: BTreeSet<RedactionRule>,
}

impl Collector {
    fn item(&mut self, layer: Layer, code: &str, text: String) {
        if self.items.len() >= budget::EVIDENCE_ITEMS {
            return;
        }
        let mut r = BTreeSet::new();
        let text = truncate(&sanitize(&text, &mut r), budget::EVIDENCE_TEXT_CHARS, &mut r);
        self.rules.extend(r);
        let evidence_id = format!("ev_{}", self.items.len() + 1);
        self.items.push(EvidenceItem { evidence_id, layer, code: code.into(), text });
    }

    fn gap(&mut self, code: &str, text: &str) {
        if self.gaps.len() >= budget::GAPS || self.gaps.iter().any(|g| g.code == code) {
            return;
        }
        let gap_id = format!("gap_{}", self.gaps.len() + 1);
        self.gaps.push(Gap { gap_id, code: code.into(), text: text.into() });
    }
}

/// Build the request, the redacted package and the local core facts for one entry. The entry
/// must exist in `view` (callers resolve it through the index first).
pub fn build(input: &EvidenceInput<'_>) -> Result<Built, DiagError> {
    let view = input.view;
    let entry = input.entry;
    let f = view.info(entry).clone();
    let platform = view.platform();
    let path = view.path(entry);
    let raw_name = view.name(entry).lossy();
    let mut name_rules = BTreeSet::new();
    let mut loc_rules = BTreeSet::new();

    // Subject name and location: redacted by default, full only by explicit disclosure.
    let (stem, ext) = split_ext(&raw_name);
    let ext = if f.kind == ObjectKind::Directory { None } else { ext };
    let parent_display = match view.parent(entry) {
        Some(p) => view.path(p).lossy(),
        None => String::new(),
    };
    let (name, location) = if input.disclosure.full_path {
        let name = truncate(&sanitize(&raw_name, &mut name_rules), budget::NAME_CHARS, &mut name_rules);
        let loc = sanitize(&parent_display, &mut loc_rules);
        // Keep the tail of a long path (closest to the object).
        let loc = if loc.chars().count() > budget::LOCATION_CHARS {
            loc_rules.insert(RedactionRule::Truncated);
            let tail: String = loc.chars().rev().take(budget::LOCATION_CHARS - 1).collect::<Vec<_>>().into_iter().rev().collect();
            format!("…{tail}")
        } else {
            loc
        };
        (name, loc)
    } else {
        let base = if ext.is_some() { stem } else { raw_name.as_str() };
        let mut n = redact_text(base, budget::NAME_CHARS.saturating_sub(12), &mut name_rules);
        if let Some(e) = &ext {
            n = format!("{n}.{e}");
        }
        (n, default_location(&parent_display, platform, input.home, &mut loc_rules))
    };

    let mut c = Collector { items: Vec::new(), gaps: Vec::new(), rules: BTreeSet::new() };
    let mut seen = BTreeSet::new();
    let mut w = Walk::default();
    let mut child_totals: Vec<(u64, KnownTotal)> = Vec::new();
    let is_dir = f.kind == ObjectKind::Directory;
    let (size_basis, subtree_total) = if is_dir {
        let mut total = KnownTotal::default();
        w.unread += f.skipped_here;
        for _ in 0..f.skipped_here {
            let _ = total.add(None);
        }
        for &child in view.children(entry) {
            let t = walk(view, child, true, &mut seen, &mut w);
            child_totals.push((child, t));
            if let Ok(k) = total.known.checked_add(t.known) {
                total.known = k;
            }
            total.unknown_count += t.unknown_count;
        }
        (if total.unknown_count == 0 { "subtree_known" } else { "unknown" }, Some(total))
    } else {
        (if f.logical.is_some() { "entity" } else { "unknown" }, None)
    };
    let logical = match subtree_total {
        Some(t) => t.exact().map(|b| b.to_string()),
        None => f.logical.map(|b| b.to_string()),
    };
    let modified_at = f.modified_unix.map(crate::time::utc_from_unix);
    let subject = Subject {
        kind: kind_str(f.kind).into(),
        name: name.clone(),
        extension: ext.clone(),
        location,
        logical_bytes: logical.clone(),
        allocated_bytes: if is_dir { None } else { f.allocated.map(|b| b.to_string()) },
        size_basis: size_basis.into(),
        modified_at: modified_at.clone(),
        link_count: f.link_count,
        cloud: cloud_str(f.cloud).into(),
        identity_quality: if f.quality == IdentityQuality::Strong { "strong" } else { "weak" }.into(),
    };

    // Observation layer.
    let scope = if input.scan.partial { "部分范围未读取" } else { "范围已完整读取" };
    c.item(Layer::Observation, "observation.scan", format!("索引观察时间 {}；这次扫描{scope}。", input.scan.observed_at));
    let size_text = match (&subtree_total, f.logical) {
        (Some(t), _) if t.unknown_count == 0 => format!("目录内已知逻辑大小合计 {}。", human_bytes(t.known.0)),
        (Some(t), _) => format!("目录内已知逻辑大小至少 {}，另有 {} 处大小未知。", human_bytes(t.known.0), t.unknown_count),
        (None, Some(b)) => match f.allocated {
            Some(a) => format!("逻辑大小 {}，分配大小 {}。", human_bytes(b), human_bytes(a)),
            None => format!("逻辑大小 {}，分配大小未知。", human_bytes(b)),
        },
        (None, None) => "大小未知（未能读取）。".into(),
    };
    c.item(Layer::Observation, "observation.size", size_text);
    match &modified_at {
        Some(t) => c.item(Layer::Observation, "observation.modified", format!("最后修改时间 {t}（文件系统记录，不代表最后使用时间）。")),
        None => c.gap("modified_unknown", "修改时间未知。"),
    }
    if f.link_count.is_some_and(|n| n > 1) {
        c.item(Layer::Observation, "observation.hard_links", format!("该文件有 {} 个名称（硬链接）；只删除其中一个名称不会释放空间。", f.link_count.unwrap_or(0)));
    }
    if f.cloud == CloudState::Placeholder {
        c.item(Layer::Observation, "observation.cloud_placeholder", "这是云端占位文件：内容不在本机，诊断不会下载它。".into());
    }
    if f.quality == IdentityQuality::Weak {
        c.item(Layer::Observation, "observation.weak_identity", "对象身份只能弱识别，不能作为执行依据。".into());
    }
    if f.boundary {
        c.item(Layer::Observation, "observation.boundary", "这是链接或挂载边界，扫描未进入其中。".into());
    }
    if is_dir {
        let mut t = format!("目录下索引到 {} 个文件、{} 个子目录。", w.files, w.dirs);
        if w.unread > 0 {
            t.push_str(&format!("{} 处子范围无法读取。", w.unread));
        }
        if w.boundaries > 0 {
            t.push_str(&format!("{} 个链接/挂载边界未进入。", w.boundaries));
        }
        if w.placeholders > 0 {
            t.push_str(&format!("{} 个云端占位文件未下载。", w.placeholders));
        }
        c.item(Layer::Observation, "observation.subtree", t);
    }

    // Rule layer: only the exact entry's candidate can be handed off; a covering parent is noted.
    let mut facts = CoreFacts {
        entry_id: entry.to_string(),
        kind: kind_str(f.kind).into(),
        volume_id: f.volume_id.clone(),
        file_id: f.file_id.clone(),
        object_fingerprint: object_fingerprint(view, entry),
        ..CoreFacts::default()
    };
    match input.candidates {
        None => c.gap("rule_analysis_missing", "这次扫描还没有清理分析结果，规则匹配情况未知。"),
        Some(cands) => {
            let own = cands.iter().find(|x| x.candidate.entry_id == entry);
            let covering = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| cands.iter().find(|x| x.candidate.entry_id == a));
            match own {
                Some(cf) => {
                    let cand = cf.candidate;
                    for r in cand.rules.iter().take(3) {
                        let rebuild = cf.catalog.and_then(|k| k.rebuild_class).map(rebuild_text).unwrap_or("重建方式未记录");
                        c.item(Layer::Rule, "rule.matched", format!("匹配规则 {}@{}（类别 {}）：{}；{}。", r.rule_id, r.version, cand.category, cand.impact, rebuild));
                    }
                    let reasons: Vec<&str> = cand.reasons.iter().map(String::as_str).take(6).collect();
                    let tier = serde_json::to_value(cand.tier).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                    c.item(Layer::Rule, "rule.candidate_state", format!("核心判定：状态 {}，层级 {}；原因 {}。", state_text(cand.state), tier, if reasons.is_empty() { "无".into() } else { reasons.join(", ") }));
                    facts.candidate_id = Some(cand.candidate_id.clone());
                    facts.candidate_state = Some(state_text(cand.state).into());
                    facts.plannable = plannable(cand, cf.catalog);
                    facts.rule_refs = cand.rules.iter().map(|r| format!("{}@{}", r.rule_id, r.version)).collect();
                    if cf.catalog.is_none() {
                        c.gap("catalog_not_fresh", "候选来自已保存的分析，没有本次的实时探测；交接计划前需要重新分析。");
                    }
                    if let Some(skip) = cf.catalog.and_then(|k| k.skip) {
                        c.item(Layer::Rule, "rule.probe_skip", format!("现场探测：{}，当前不能选择。", skip.as_str()));
                    }
                }
                None => match covering {
                    Some(parent) => c.item(
                        Layer::Rule,
                        "rule.covered_by_parent",
                        format!("上级目录是规则 {} 的候选；该候选会包含更多对象，不能作为此对象的单独处理入口。", parent.candidate.rules.first().map(|r| r.rule_id.as_str()).unwrap_or("?")),
                    ),
                    None => c.item(Layer::Rule, "rule.none", "没有已启用的清理规则匹配这个对象；这不说明它是垃圾，也不说明它重要。".into()),
                },
            }
        }
    }

    // Protection layers (protection always wins over any model text).
    let mut protection: Option<String> = None;
    if let Some(why) = hard_deny_reason(view, entry, &path, input.protection.hard_deny) {
        c.item(Layer::Protection, "protection.hard_deny", format!("系统硬性保护：{why}；任何方式都不会处理它。"));
        protection = Some(format!("hard_deny:{why}"));
    }
    match input.protection.user {
        UserList::Loaded(u) => {
            if let Some(e) = u.match_path(&path, is_dir) {
                c.item(Layer::Protection, "protection.user", format!("在你的保护名单中（{}）。", e.id));
                protection.get_or_insert_with(|| format!("user_protected:{}", e.id));
            }
        }
        UserList::Unavailable(_) => c.gap("user_protection_unavailable", "保护名单当前无法读取，处理入口保持关闭。"),
        UserList::NotSupplied => c.gap("user_protection_not_checked", "未核对用户保护名单。"),
    }
    let self_class = protection_by_name(view, entry);
    let ancestor_class = std::iter::successors(view.parent(entry), |p| view.parent(*p)).find_map(|a| protection_by_name(view, a));
    if let Some(cl) = self_class {
        c.item(Layer::Protection, "protection.builtin_self", format!("属于默认保护类别 {}：{}。", cl.as_str(), cl.describe()));
        protection.get_or_insert_with(|| format!("protected:{}", cl.as_str()));
    } else if let Some(cl) = ancestor_class {
        c.item(Layer::Protection, "protection.builtin_ancestor", format!("位于默认保护类别 {} 的目录内。", cl.as_str()));
        protection.get_or_insert_with(|| format!("protected:{}", cl.as_str()));
    }
    if !w.inner.is_empty() {
        let list: Vec<&str> = w.inner.iter().map(|x| x.as_str()).collect();
        c.item(Layer::Protection, "protection.builtin_inner", format!("目录内包含默认保护类别的内容：{}。", list.join(", ")));
        protection.get_or_insert_with(|| format!("contains:{}", list[0]));
    }
    let checked = input.protection.hard_deny.is_some() && matches!(input.protection.user, UserList::Loaded(_));
    if protection.is_none() && checked {
        c.item(Layer::Protection, "protection.none", "硬性保护、默认保护类别和你的保护名单都没有命中。".into());
    }
    if input.protection.hard_deny.is_none() {
        c.gap("hard_deny_not_checked", "未加载系统硬性保护根，处理入口保持关闭。");
    }
    facts.protection = protection;
    facts.protection_checked = checked;

    // Ownership layer (read-only inventory).
    if input.ownership.checked {
        if input.ownership.apps.is_empty() {
            c.item(Layer::Ownership, "ownership.none", "没有安装记录指向这个位置；这不证明它是残留文件。".into());
        }
        for (id, app) in input.ownership.apps.iter().take(3) {
            let mut r = BTreeSet::new();
            let n = redact_text(app, 60, &mut r);
            let shared = if input.ownership.shared { "，并与其他应用共享该位置" } else { "" };
            c.item(Layer::Ownership, "ownership.app", format!("位于已安装应用「{n}」记录的安装位置{shared}。"));
            facts.app_instance_ids.push(id.clone());
        }
    } else {
        c.gap("ownership_not_checked", "未查询应用安装记录。");
    }

    // The person's statements are evidence of what they said, not of the file (AID-05).
    for s in &input.statements {
        let mut r = BTreeSet::new();
        let t = truncate(&sanitize(&s.text, &mut r), 200, &mut r);
        c.item(Layer::UserStatement, "user.statement", format!("用户陈述（未经核验）：{t}"));
    }

    // Gaps the core can never close by itself.
    if input.scan.partial {
        c.gap("partial_scan", "这次扫描有未读取的范围。");
    }
    if w.unread > 0 {
        c.gap("unread_descendants", "目录内有无法读取的子范围，其中内容未知，不能视为可清理。");
    }
    if f.cloud == CloudState::Placeholder || w.placeholders > 0 {
        c.gap("cloud_not_downloaded", "云端占位内容未下载，诊断不读取云端内容。");
    }
    if w.truncated {
        c.gap("subtree_summary_truncated", "目录过大，摘要只统计了一部分。");
    }
    c.gap("backup_unknown", "CleanManager 不检测备份；是否已有可用备份未知。");
    c.gap("usage_unknown", "未检测是否有程序或工程正在使用它。");
    c.gap("reclaim_unknown", "删除后实际能释放多少空间未知（共享块、硬链接、回收站等）。");
    if is_dir && !input.disclosure.child_summary {
        c.gap("children_not_disclosed", "未发送子项摘要。");
    }

    // Optional child summary: at most five largest direct children, names redacted.
    let children = if is_dir && input.disclosure.child_summary {
        child_totals.sort_by(|a, b| b.1.known.cmp(&a.1.known).then(a.0.cmp(&b.0)));
        let mut r = BTreeSet::new();
        let out: Vec<ChildSummary> = child_totals
            .iter()
            .take(budget::CHILDREN)
            .map(|(id, t)| ChildSummary {
                name: redact_text(&view.name(*id).lossy(), 80, &mut r),
                kind: kind_str(view.info(*id).kind).into(),
                logical_bytes: t.exact().map(|b| b.to_string()),
            })
            .collect();
        c.rules.extend(r);
        Some(out)
    } else {
        None
    };
    let children_omitted = if children.is_some() { child_totals.len().saturating_sub(budget::CHILDREN) as u64 } else { 0 };

    let mut redactions: Vec<Redaction> = Vec::new();
    redactions.extend(name_rules.iter().map(|r| Redaction { field: "subject.name".into(), rule: *r }));
    redactions.extend(loc_rules.iter().map(|r| Redaction { field: "subject.location".into(), rule: *r }));
    redactions.extend(c.rules.iter().map(|r| Redaction { field: "items".into(), rule: *r }));
    if name_rules.contains(&RedactionRule::InstructionLike) {
        c.gap("name_instruction_like", "名称含疑似指令文本，已省略；名称不会改变采集范围。");
    } else if !name_rules.is_empty() && !input.disclosure.full_path {
        c.gap("name_redacted", "名称中的敏感片段已脱敏。");
    }
    facts.partial_evidence = input.scan.partial || w.unread > 0 || w.truncated || f.quality == IdentityQuality::Weak || f.boundary;
    facts.cloud_placeholder = f.cloud == CloudState::Placeholder || w.placeholders > 0;
    facts.hard_links = f.link_count.is_some_and(|n| n > 1);

    let package = EvidencePackage {
        version: EVIDENCE_VERSION.into(),
        subject,
        children,
        children_omitted,
        items: c.items,
        gaps: c.gaps,
        redactions,
        excluded: ALWAYS_EXCLUDED.to_vec(),
    };
    package.validate()?;
    let request = DiagnosisRequest {
        contract: DIAG_CONTRACT.into(),
        object: ObjectRef {
            scan_id: input.scan.scan_id.into(),
            revision: input.scan.revision,
            entry_id: entry.to_string(),
            origin: input.origin,
            candidate_id: facts.candidate_id.clone(),
            app_instance_id: facts.app_instance_ids.first().cloned(),
        },
        observed_at: input.scan.observed_at.into(),
        observation_fingerprint: fingerprint(view, input.scan.scan_id, input.scan.revision, entry),
        disclosure: input.disclosure,
        question: input.question.clone(),
        statements: input.statements.clone(),
        parent_diagnosis_id: input.parent_diagnosis_id.clone(),
        prompt_template: super::prompt::TEMPLATE_VERSION.into(),
    };
    request.validate()?;
    Ok(Built { request, package, facts })
}
