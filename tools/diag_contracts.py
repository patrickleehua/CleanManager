"""Contract `diag/1` for the optional AI cleanup diagnosis (AID-01..13).

Kept apart from schemas/v1 on purpose: the first-version protocol is not widened by an extension
whose release is negotiated separately. `generate()` writes the schema and synthetic examples;
`validate()` checks them (schema + semantics) and returns the negative cases it rejected. The Rust
types in crates/cm-core/src/diag.rs parse the same examples (crate tests), so both sides agree.
Examples are synthetic: no real scan, provider answer or credential is recorded here.
"""
from __future__ import annotations
import copy
import hashlib
import json
from pathlib import Path

SCHEMA_PATH = "schemas/diag/v1/diagnosis.schema.json"
EXAMPLES_DIR = "examples/diagnosis"

STR = {"type": "string", "minLength": 1}
TIME = {"type": "string", "format": "date-time", "pattern": "Z$"}
HASH = {"type": "string", "pattern": "^[a-f0-9]{64}$"}
UINT = {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"}
COUNT = {"type": "integer", "minimum": 0, "maximum": 9007199254740991}
DIAG_ID = {"type": "string", "pattern": "^dg_[0-9a-f]{20}$"}
PREVIEW_ID = {"type": "string", "pattern": "^pv_[0-9a-f]{20}$"}
BUDGET = {"name": 120, "location": 240, "evidence_text": 240, "items": 24, "gaps": 12, "children": 5, "user_text": 280,
          "statements": 8, "payload": 48 * 1024, "summary": 280, "paragraph": 600, "short": 200, "claims": 10, "refs": 6,
          "list": 6, "alternatives": 5, "questions": 3, "choices": 4, "choice": 60, "message": 280, "page": 100}
PREVIEW_TTL, RESULT_TTL, SESSION_TTL = 600, 86400, 86400


def obj(props, required=None):
    return {"type": "object", "properties": props, "required": list(props) if required is None else required, "additionalProperties": False}


def ref(name):
    return {"$ref": "#/$defs/" + name}


def nullable(v):
    return {"anyOf": [v, {"type": "null"}]}


def arr(v, maximum=None, minimum=0):
    out = {"type": "array", "items": v, "minItems": minimum}
    if maximum is not None:
        out["maxItems"] = maximum
    return out


def text(n):
    return {"type": "string", "maxLength": n}


def enum(*v):
    return {"enum": list(v)}


def canonical(v):
    return json.dumps(v, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")


def schema():
    d = {}
    d["object_ref"] = obj({"scan_id": STR, "revision": COUNT, "entry_id": UINT, "origin": enum("cleanup", "disk", "apps"),
                           "candidate_id": nullable(STR), "app_instance_id": nullable(STR)})
    d["disclosure"] = obj({"full_path": {"type": "boolean"}, "child_summary": {"type": "boolean"}})
    d["user_statement"] = obj({"statement_id": STR, "question_id": nullable(STR), "text": {**text(BUDGET["user_text"]), "minLength": 1}, "at": TIME})
    d["request"] = obj({"contract": {"const": "diag/1"}, "object": ref("object_ref"), "observed_at": TIME, "observation_fingerprint": HASH,
                        "disclosure": ref("disclosure"), "question": nullable({**text(BUDGET["user_text"]), "minLength": 1}),
                        "statements": arr(ref("user_statement"), BUDGET["statements"]), "parent_diagnosis_id": nullable(DIAG_ID),
                        "prompt_template": {"const": "diag-prompt/1"}})
    layer = enum("observation", "rule", "protection", "ownership", "coverage", "user_statement")
    d["evidence_item"] = obj({"evidence_id": {"type": "string", "pattern": "^ev_[1-9][0-9]*$"}, "layer": layer, "code": STR,
                              "text": text(BUDGET["evidence_text"])})
    d["gap"] = obj({"gap_id": {"type": "string", "pattern": "^gap_[1-9][0-9]*$"}, "code": STR, "text": text(BUDGET["evidence_text"])})
    d["redaction"] = obj({"field": STR, "rule": enum("user_profile", "email", "long_number", "secret_like", "control_characters",
                                                     "instruction_like", "truncated", "parent_components")})
    kind = enum("file", "directory", "symlink", "reparse", "other")
    d["subject"] = obj({"kind": kind, "name": text(BUDGET["name"]), "extension": nullable({"type": "string", "pattern": "^[a-z0-9]{1,10}$"}),
                        "location": text(BUDGET["location"]), "logical_bytes": nullable(UINT), "allocated_bytes": nullable(UINT),
                        "size_basis": enum("entity", "subtree_known", "unknown"), "modified_at": nullable(TIME),
                        "link_count": nullable(COUNT), "cloud": enum("not_cloud", "hydrated", "placeholder", "unknown"),
                        "identity_quality": enum("strong", "weak")})
    d["child_summary"] = obj({"name": text(80), "kind": kind, "logical_bytes": nullable(UINT)})
    excluded = enum("file_contents", "directory_tree", "credentials", "sessions", "cloud_hydration")
    d["evidence"] = obj({"version": {"const": "diag-evidence/1"}, "subject": ref("subject"),
                         "children": nullable(arr(ref("child_summary"), BUDGET["children"])), "children_omitted": COUNT,
                         "items": arr(ref("evidence_item"), BUDGET["items"]), "gaps": arr(ref("gap"), BUDGET["gaps"]),
                         "redactions": arr(ref("redaction"), 64),
                         "excluded": {**arr(excluded, 5, 5), "uniqueItems": True}})
    rec = enum("reviewable", "needs_context", "keep", "unsupported")
    action = enum("review_plan", "keep", "protect", "locate", "verify", "app_management", "archive")
    d["answer"] = obj({"answer_version": {"const": "diag-answer/1"}, "summary": {**text(BUDGET["summary"]), "minLength": 1},
                       "purpose": {**text(BUDGET["paragraph"]), "minLength": 1}, "recommendation": rec,
                       "conditions": arr(text(BUDGET["short"]), BUDGET["list"]), "deletion_impact": text(BUDGET["paragraph"]),
                       "rebuild_cost": text(BUDGET["paragraph"]),
                       "claims": arr(obj({"text": text(BUDGET["short"]), "basis": enum("observation", "rule", "user_statement", "inference"),
                                          "refs": {**arr({"type": "string", "pattern": "^(ev|gap)_[1-9][0-9]*$"}, BUDGET["refs"]), "uniqueItems": True}}), BUDGET["claims"]),
                       "gaps": arr(text(BUDGET["short"]), BUDGET["list"]),
                       "alternatives": arr(obj({"action": action, "text": text(BUDGET["short"])}), BUDGET["alternatives"]),
                       "questions": arr(obj({"question_id": {"type": "string", "pattern": "^q[1-3]$"}, "text": text(BUDGET["short"]),
                                             "choices": arr(text(BUDGET["choice"]), BUDGET["choices"])}), BUDGET["questions"])})
    d["result"] = obj({"answer": ref("answer"), "model_recommendation": rec, "effective_recommendation": rec,
                       "grounding": arr(obj({"index": COUNT, "status": enum("supported", "user_statement", "inference", "unsupported", "conflicts_with_facts"),
                                             "note": nullable(text(BUDGET["short"]))}), BUDGET["claims"]),
                       "conflicts": arr(obj({"code": enum("protected_object", "not_a_candidate", "candidate_not_plannable", "partial_evidence",
                                                          "unverified_backup_claim", "unverified_usage_claim", "unverified_reclaim_claim",
                                                          "safe_to_delete_claim", "rule_claim_without_rule"), "text": text(BUDGET["short"])}), 16),
                       "sanitized": arr(obj({"field": STR, "reason": enum("command_text", "approval_instruction", "unknown_reference",
                                                                          "unverified_safety_claim", "truncated", "dropped_item")}), 64)})
    d["facts"] = obj({"entry_id": UINT, "kind": kind, "volume_id": STR, "file_id": STR, "object_fingerprint": HASH, "protection": nullable(STR), "protection_checked": {"type": "boolean"},
                      "candidate_id": nullable(STR), "candidate_state": nullable(enum("allowed", "protected_reviewable", "pending_evidence", "blocked")),
                      "plannable": {"type": "boolean"}, "partial_evidence": {"type": "boolean"}, "cloud_placeholder": {"type": "boolean"},
                      "hard_links": {"type": "boolean"}, "app_instance_ids": arr(STR, 16), "rule_refs": arr(STR, 16)})
    provider_kind = enum("anthropic", "openai_compatible", "claude_code_cli", "codex_cli")
    d["destination"] = obj({"provider_id": STR, "label": text(80), "kind": provider_kind, "mode": enum("api", "cli"),
                            "endpoint": nullable(text(300)), "executable": nullable(text(400)), "model": {**text(120), "minLength": 1},
                            "template": {"const": "diag-prompt/1"}})
    d["payload"] = obj({"transport": enum("https_post", "http_post_loopback", "process_stdin"), "target": STR,
                        "headers": arr({"type": "string", "maxLength": 300, "not": {"pattern": "(?i)(sk-ant-|bearer [a-z0-9_-]{16,})"}}, 8),
                        "argv": arr(text(4096), 40), "body": text(BUDGET["payload"])})
    d["preview"] = obj({"preview_id": PREVIEW_ID, "state": {"const": "preparing"}, "request": ref("request"), "evidence": ref("evidence"),
                        "destination": ref("destination"), "payload": ref("payload"), "payload_digest": HASH,
                        "payload_bytes": {"type": "integer", "minimum": 1, "maximum": BUDGET["payload"]},
                        "contains_user_additions": {"type": "boolean"}, "created_at": TIME, "expires_at": TIME})
    target = obj({"scan_id": STR, "entry_id": UINT, "candidate_id": nullable(STR), "app_instance_id": nullable(STR)})
    d["handoff"] = obj({"action": action, "available": {"type": "boolean"}, "reason": text(BUDGET["short"]),
                        "suggested_by_model": {"type": "boolean"}, "target": nullable(target)})
    state = enum("running", "completed", "cancelled", "failed", "expired")
    failure = obj({"code": enum("not_configured", "credential_missing", "credential_store_unavailable", "auth_failed", "model_unavailable",
                                "rate_limited", "provider_error", "refused", "timeout", "network", "output_too_large", "invalid_structure",
                                "unsupported_mode", "process_failed"), "message": text(BUDGET["message"]), "retryable": {"type": "boolean"}})
    d["diagnosis"] = obj({"diagnosis_id": DIAG_ID, "state": state, "request": ref("request"), "evidence": ref("evidence"), "facts": ref("facts"),
                          "destination": ref("destination"), "payload_digest": HASH, "created_at": TIME, "updated_at": TIME,
                          "finished_at": nullable(TIME), "expires_at": TIME, "retention": enum("session", "saved"), "retain_until": nullable(TIME),
                          "result": nullable(ref("result")), "failure": nullable(failure),
                          "freshness": obj({"state": enum("current", "stale", "unknown"), "checked_at": TIME, "reasons": arr(STR, 8)}),
                          "handoff": arr(ref("handoff"), 7)})
    # Lifecycle shape: a result exactly for completed/expired, a failure exactly for failed.
    d["diagnosis"]["allOf"] = [
        {"if": {"properties": {"state": enum("completed", "expired")}}, "then": {"properties": {"result": ref("result"), "failure": {"type": "null"}}}},
        {"if": {"properties": {"state": {"const": "failed"}}}, "then": {"properties": {"result": {"type": "null"}, "failure": failure}}},
        {"if": {"properties": {"state": enum("running", "cancelled")}}, "then": {"properties": {"result": {"type": "null"}, "failure": {"type": "null"}}}},
        {"if": {"properties": {"state": {"const": "running"}}}, "then": {"properties": {"finished_at": {"type": "null"}}},
         "else": {"properties": {"finished_at": TIME}}},
        {"if": {"properties": {"retention": {"const": "session"}}}, "then": {"properties": {"retain_until": TIME}},
         "else": {"properties": {"retain_until": {"type": "null"}}}},
    ]
    d["summary"] = obj({"diagnosis_id": DIAG_ID, "state": state, "object": ref("object_ref"), "subject_name": text(BUDGET["name"]),
                        "provider_id": STR, "model": STR, "effective_recommendation": nullable(rec), "created_at": TIME,
                        "retention": enum("session", "saved"), "parent_diagnosis_id": nullable(DIAG_ID)})
    d["provider_status"] = obj({"provider_id": STR, "label": text(80), "kind": provider_kind, "mode": enum("api", "cli"),
                                "endpoint": nullable(text(300)), "executable": nullable(text(400)), "model": text(120),
                                "credential": enum("present", "absent", "not_required", "unavailable"),
                                "support": obj({"state": enum("supported", "unsupported", "unverified"), "reason": nullable(text(BUDGET["message"]))}),
                                "updated_at": TIME})

    def record(kind, props):
        return obj({"contract": {"const": "diag/1"}, "kind": {"const": kind}, "request_id": STR, **props})
    d["diagnosis_preview"] = record("diagnosis_preview", {"preview": ref("preview")})
    d["diagnosis_record"] = record("diagnosis_record", {"diagnosis": ref("diagnosis")})
    d["diagnosis_list"] = record("diagnosis_list", {"items": arr(ref("summary"), BUDGET["page"]), "next_cursor": nullable(STR)})
    d["diagnosis_config"] = record("diagnosis_config", {"configured": {"type": "boolean"}, "credential_store": STR,
                                                        "providers": arr(ref("provider_status"), 16), "default_provider_id": nullable(STR)})
    kinds = ["diagnosis_preview", "diagnosis_record", "diagnosis_list", "diagnosis_config"]
    return {"$schema": "https://json-schema.org/draft/2020-12/schema", "$id": "urn:cleanmanager:diagnosis:1",
            "title": "CleanManager optional AI diagnosis record (diag/1)",
            "description": "Optional extension, separate from the v1 protocol. Errors use the v1 error record. Examples are synthetic.",
            "oneOf": [ref(k) for k in kinds], "$defs": d}


# ---------------------------------------------------------------------------------------------
# Synthetic examples

T0, T1, T2 = "2026-10-14T12:00:00Z", "2026-10-15T09:30:00Z", "2026-10-15T09:30:12Z"
EXCLUDED = ["file_contents", "directory_tree", "credentials", "sessions", "cloud_hydration"]
GAPS_COMMON = [("backup_unknown", "CleanManager 不检测备份；是否已有可用备份未知。"), ("usage_unknown", "未检测是否有程序或工程正在使用它。"),
               ("reclaim_unknown", "删除后实际能释放多少空间未知（共享块、硬链接、回收站等）。")]
ANTHROPIC = {"provider_id": "anthropic-default", "label": "Anthropic API", "kind": "anthropic", "mode": "api",
             "endpoint": "https://api.anthropic.com/v1/messages", "executable": None, "model": "claude-opus-5-5", "template": "diag-prompt/1"}
LOCAL = {"provider_id": "local-llm", "label": "本机模型", "kind": "openai_compatible", "mode": "api",
         "endpoint": "http://127.0.0.1:11434/v1/chat/completions", "executable": None, "model": "example-local-model", "template": "diag-prompt/1"}


def fp(label):
    return hashlib.sha256(label.encode("utf-8")).hexdigest()


def items(*rows):
    return [{"evidence_id": f"ev_{i + 1}", "layer": layer, "code": code, "text": t} for i, (layer, code, t) in enumerate(rows)]


def gaps(*rows):
    rows = list(rows) + GAPS_COMMON
    return [{"gap_id": f"gap_{i + 1}", "code": c, "text": t} for i, (c, t) in enumerate(rows)]


def subject(kind, name, ext, location, logical, basis="entity", cloud="not_cloud", links=1):
    return {"kind": kind, "name": name, "extension": ext, "location": location, "logical_bytes": logical,
            "allocated_bytes": logical if kind == "file" else None, "size_basis": basis, "modified_at": "2026-05-28T20:26:40Z",
            "link_count": links if kind == "file" else None, "cloud": cloud, "identity_quality": "strong"}


def evidence(subj, item_rows, gap_rows, redactions=(), children=None):
    return {"version": "diag-evidence/1", "subject": subj, "children": children, "children_omitted": 0, "items": items(*item_rows),
            "gaps": gaps(*gap_rows), "redactions": [{"field": f, "rule": r} for f, r in redactions], "excluded": EXCLUDED}


def request(entry, origin="disk", candidate=None, app=None, statements=(), question=None, parent=None, full_path=False):
    return {"contract": "diag/1", "object": {"scan_id": "s_demo", "revision": 1, "entry_id": entry, "origin": origin,
                                              "candidate_id": candidate, "app_instance_id": app},
            "observed_at": T0, "observation_fingerprint": fp("obs-" + entry), "disclosure": {"full_path": full_path, "child_summary": False},
            "question": question, "statements": list(statements), "parent_diagnosis_id": parent, "prompt_template": "diag-prompt/1"}


def facts(entry, kind="file", protection=None, candidate=None, plannable=False, apps=(), rules=(), partial=False, links=False):
    return {"entry_id": entry, "kind": kind, "volume_id": "synthetic-volume", "file_id": "ntfs64:" + fp("file-" + entry)[:16],
            "object_fingerprint": fp("object-" + entry), "protection": protection, "protection_checked": True, "candidate_id": candidate,
            "candidate_state": "allowed" if candidate else None, "plannable": plannable, "partial_evidence": partial, "cloud_placeholder": False,
            "hard_links": links, "app_instance_ids": list(apps), "rule_refs": list(rules)}


def handoffs(entry, candidate=None, plan_ok=False, plan_reason="没有已启用规则的候选；本扩展不开放任意路径删除。", protect=True, app=None, suggested=()):
    tgt = {"scan_id": "s_demo", "entry_id": entry, "candidate_id": None, "app_instance_id": None}
    rows = [
        ("review_plan", plan_ok, plan_reason, {**tgt, "candidate_id": candidate} if plan_ok else None),
        ("keep", True, "不做任何操作，文件保持原状。", None),
        ("protect", protect, "加入你的保护名单；以后的规则与策略都不会选中它。" if protect else "已在你的保护名单中。", tgt),
        ("locate", True, "在磁盘视图中定位（只读）。", tgt),
        ("verify", True, "按核实清单确认备份、引用与来源。", None),
        ("app_management", app is not None, "在应用页查看关联应用（只读清单）。" if app else "没有安装记录关联到这个位置。",
         {**tgt, "app_instance_id": app} if app else None),
        ("archive", False, "CleanManager 不移动或归档文件；请使用你自己的备份工具。", None),
    ]
    return [{"action": a, "available": ok, "reason": r, "suggested_by_model": a in suggested, "target": t} for a, ok, r, t in rows]


def answer(rec, summary, purpose, impact, rebuild, claims, alternatives, questions=(), conditions=(), gap_texts=()):
    return {"answer_version": "diag-answer/1", "summary": summary, "purpose": purpose, "recommendation": rec, "conditions": list(conditions),
            "deletion_impact": impact, "rebuild_cost": rebuild, "claims": claims, "gaps": list(gap_texts), "alternatives": alternatives,
            "questions": list(questions)}


def diagnosis(did, state, req, ev, fct, dest, result=None, failure=None, handoff=(), freshness="current", reasons=(), retention="session"):
    finished = None if state == "running" else T2
    return {"diagnosis_id": did, "state": state, "request": req, "evidence": ev, "facts": fct, "destination": dest,
            "payload_digest": fp("payload-" + did), "created_at": T1, "updated_at": T2, "finished_at": finished, "expires_at": "2026-10-16T09:30:12Z",
            "retention": retention, "retain_until": "2026-10-16T09:30:00Z" if retention == "session" else None, "result": result, "failure": failure,
            "freshness": {"state": freshness, "checked_at": T2, "reasons": list(reasons)}, "handoff": list(handoff)}


def rec_env(kind, **fields):
    return {"contract": "diag/1", "kind": kind, "request_id": "req_synthetic", **fields}


def examples():
    out = {}
    # Personal video, default disclosure: redacted name/location, user statement, API destination.
    video_req = request("42", statements=[{"statement_id": "u1", "question_id": "q1", "text": "已经备份到移动硬盘了", "at": T1}],
                        question="这个视频还能删吗？")
    video_ev = evidence(subject("file", "[email] 素材 A-cam.mov", "mov", "%USERPROFILE%\\Videos\\Wedding 2025", "42000000000"),
                        [("observation", "observation.scan", "索引观察时间 2026-10-14T12:00:00Z；这次扫描范围已完整读取。"),
                         ("observation", "observation.size", "逻辑大小 39.1 GiB（42000000000 字节），分配大小 39.1 GiB（42000000000 字节）。"),
                         ("rule", "rule.none", "没有已启用的清理规则匹配这个对象；这不说明它是垃圾，也不说明它重要。"),
                         ("protection", "protection.none", "硬性保护、默认保护类别和你的保护名单都没有命中。"),
                         ("ownership", "ownership.none", "没有安装记录指向这个位置；这不证明它是残留文件。"),
                         ("user_statement", "user.statement", "用户陈述（未经核验）：已经备份到移动硬盘了")],
                        [("name_redacted", "名称中的敏感片段已脱敏。")],
                        [("subject.name", "email"), ("subject.location", "user_profile")])
    user_msg = json.dumps({"answer_format": "diag-answer/1", "evidence": video_ev, "language": "zh-CN", "question": video_req["question"],
                              "task": "explain_one_object", "template": "diag-prompt/1",
                              "user_statements": [{"question_id": "q1", "statement_id": "u1", "text": "已经备份到移动硬盘了"}]},
                             ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    body = json.dumps({"max_tokens": 16000, "messages": [{"content": user_msg, "role": "user"}], "model": "claude-opus-5-5",
                          "output_config": {"effort": "medium"}, "system": "<diag-prompt/1 system text>"},
                         ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    payload = {"transport": "https_post", "target": ANTHROPIC["endpoint"],
               "headers": ["content-type: application/json", "anthropic-version: 2023-06-01",
                           "x-api-key: <stored in the system credential store as CleanManager:diag:anthropic-default>"],
               "argv": [], "body": body}
    digest = hashlib.sha256(canonical({"destination": ANTHROPIC, "payload": payload})).hexdigest()
    out["preview-video"] = rec_env("diagnosis_preview", preview={
        "preview_id": "pv_" + "1" * 20, "state": "preparing", "request": video_req, "evidence": video_ev, "destination": ANTHROPIC,
        "payload": payload, "payload_digest": digest, "payload_bytes": len(body.encode("utf-8")), "contains_user_additions": True,
        "created_at": T1, "expires_at": "2026-10-15T09:40:00Z"})
    video_answer = answer("needs_context", "这是个人拍摄素材；在确认备份可读且没有剪辑工程引用前，建议保留。",
                          "看起来是一段相机原始素材，常被剪辑工程引用。", "删除后无法从本机恢复；若工程仍引用它，工程会缺素材。",
                          "无法重建，只能从备份恢复。",
                          [{"text": "用户说已经备份到移动硬盘", "basis": "user_statement", "refs": ["ev_6"]},
                           {"text": "没有清理规则覆盖它", "basis": "rule", "refs": ["ev_3"]}],
                          [{"action": "verify", "text": "打开备份确认文件可读"}, {"action": "keep", "text": "确认前保留"}],
                          [{"question_id": "q1", "text": "剪辑工程是否仍引用这段素材？", "choices": ["仍在使用", "已完成导出", "不确定"]}],
                          ["备份可读", "工程不再引用"])
    video_result = {"answer": video_answer, "model_recommendation": "needs_context", "effective_recommendation": "needs_context",
                    "grounding": [{"index": 0, "status": "user_statement", "note": "你的陈述，CleanManager 未核验"},
                                  {"index": 1, "status": "conflicts_with_facts", "note": "本机没有规则匹配这个对象"}],
                    "conflicts": [{"code": "rule_claim_without_rule", "text": "回答引用了清理规则，但没有已启用规则匹配该对象。"}], "sanitized": []}
    out["record-video-needs-context"] = rec_env("diagnosis_record", diagnosis=diagnosis(
        "dg_" + "a" * 20, "completed", video_req, video_ev, facts("42"), ANTHROPIC, result=video_result,
        handoff=handoffs("42", suggested=("verify", "keep"))))
    # Browser code cache with an enabled rule: one candidate, plan hand-off available.
    cache_req = request("17", origin="cleanup", candidate="c_" + "0" * 23 + "1")
    cache_ev = evidence(subject("directory", "Code Cache", None, "%USERPROFILE%\\…\\Local\\ExampleBrowser", "314572800", basis="subtree_known"),
                        [("observation", "observation.scan", "索引观察时间 2026-10-14T12:00:00Z；这次扫描范围已完整读取。"),
                         ("observation", "observation.size", "目录内已知逻辑大小合计 300.0 MiB（314572800 字节）。"),
                         ("rule", "rule.matched", "匹配规则 example.browser.code-cache@0.1.0（类别 app_caches）：下次启动时重新生成；可由所属程序在本机重建。"),
                         ("rule", "rule.candidate_state", "核心判定：状态 allowed，层级 default_selected；原因 无。"),
                         ("protection", "protection.none", "硬性保护、默认保护类别和你的保护名单都没有命中。")],
                        [], [("subject.location", "user_profile"), ("subject.location", "parent_components")])
    cache_answer = answer("reviewable", "这是浏览器的脚本编译缓存；在浏览器关闭时可以进入计划审阅。", "加速网页脚本启动的缓存。",
                          "下次打开网页时首次加载稍慢。", "浏览器会在本机自动重建。",
                          [{"text": "规则说明该缓存可在本机重建", "basis": "rule", "refs": ["ev_3"]}],
                          [{"action": "review_plan", "text": "在计划页审阅这一项"}])
    out["record-cache-reviewable"] = rec_env("diagnosis_record", diagnosis=diagnosis(
        "dg_" + "b" * 20, "completed", cache_req, cache_ev,
        facts("17", "directory", candidate=cache_req["object"]["candidate_id"], plannable=True, rules=["example.browser.code-cache@0.1.0"]), ANTHROPIC,
        result={"answer": cache_answer, "model_recommendation": "reviewable", "effective_recommendation": "reviewable",
                "grounding": [{"index": 0, "status": "supported", "note": None}], "conflicts": [], "sanitized": []},
        handoff=handoffs("17", candidate=cache_req["object"]["candidate_id"], plan_ok=True,
                         plan_reason="生成只含这一个候选的新计划；授权与现场核验在计划页独立进行。", suggested=("review_plan",)), retention="saved"))
    # Protected database: model text called it disposable; protection wins.
    db_req = request("88", origin="apps", app="app_example")
    db_ev = evidence(subject("file", "history.sqlite", "sqlite", "%USERPROFILE%\\AppData\\Local\\ExampleApp", "943718400"),
                     [("observation", "observation.size", "逻辑大小 900.0 MiB（943718400 字节），分配大小 900.0 MiB（943718400 字节）。"),
                      ("rule", "rule.none", "没有已启用的清理规则匹配这个对象；这不说明它是垃圾，也不说明它重要。"),
                      ("protection", "protection.builtin_self", "属于默认保护类别 database：Database files and their journals (.db, .sqlite, -wal, -shm, .mdb, .accdb)。"),
                      ("ownership", "ownership.app", "位于已安装应用「Example App」记录的安装位置。")],
                     [], [("subject.location", "user_profile")])
    db_answer = answer("reviewable", "这是应用的历史数据库。", "保存浏览或使用历史。", "删除会丢失历史记录。", "无法重建。",
                       [{"text": "这是可丢弃的临时数据库，可以删除", "basis": "inference", "refs": []}],
                       [{"action": "review_plan", "text": "[已移除：回答包含批准/授权说法；授权只能由你在计划审阅中完成]"}])
    out["record-database-protected"] = rec_env("diagnosis_record", diagnosis=diagnosis(
        "dg_" + "c" * 20, "completed", db_req, db_ev, facts("88", protection="protected:database", apps=["app_example"]), LOCAL,
        result={"answer": db_answer, "model_recommendation": "reviewable", "effective_recommendation": "keep",
                "grounding": [{"index": 0, "status": "conflicts_with_facts", "note": "该对象受保护"}],
                "conflicts": [{"code": "protected_object", "text": "对象受保护（protected:database）；保护优先于任何建议。"}],
                "sanitized": [{"field": "alternatives[0].text", "reason": "approval_instruction"}]},
        handoff=handoffs("88", plan_reason="受保护（protected:database），核心不会为它生成计划；可以保留或定位。", app="app_example", suggested=("review_plan",))))
    # Authentication failure: local facts stay, no other provider is tried.
    out["record-auth-failed"] = rec_env("diagnosis_record", diagnosis=diagnosis(
        "dg_" + "d" * 20, "failed", request("42"), video_ev, facts("42"), ANTHROPIC,
        failure={"code": "auth_failed", "message": "提供方拒绝了凭据（HTTP 401）。请在设置中更新 API Key；没有尝试其他提供方。", "retryable": False},
        handoff=handoffs("42")))
    # Expired: readable answer, plan hand-off closed until re-verified.
    expired = copy.deepcopy(out["record-cache-reviewable"]["diagnosis"])
    expired.update(diagnosis_id="dg_" + "e" * 20, state="expired", retention="session", retain_until="2026-10-16T09:30:00Z",
                   freshness={"state": "stale", "checked_at": T2, "reasons": ["observation_changed"]},
                   handoff=handoffs("17", plan_reason="诊断未完成或已过期；请先重新核验。", suggested=("review_plan",)))
    out["record-cache-expired"] = rec_env("diagnosis_record", diagnosis=expired)
    out["record-cancelled"] = rec_env("diagnosis_record", diagnosis=diagnosis(
        "dg_" + "f" * 20, "cancelled", request("42"), video_ev, facts("42"), ANTHROPIC, handoff=handoffs("42")))
    out["list"] = rec_env("diagnosis_list", next_cursor=None, items=[
        {"diagnosis_id": d["diagnosis_id"], "state": d["state"], "object": d["request"]["object"], "subject_name": d["evidence"]["subject"]["name"],
         "provider_id": d["destination"]["provider_id"], "model": d["destination"]["model"],
         "effective_recommendation": d["result"]["effective_recommendation"] if d["result"] else None, "created_at": d["created_at"],
         "retention": d["retention"], "parent_diagnosis_id": d["request"]["parent_diagnosis_id"]}
        for d in (out[k]["diagnosis"] for k in ("record-cache-reviewable", "record-video-needs-context", "record-auth-failed"))])
    out["config"] = rec_env("diagnosis_config", configured=True, credential_store="windows_credential_manager", default_provider_id="anthropic-default",
                            providers=[
                                {"provider_id": "anthropic-default", "label": "Anthropic API", "kind": "anthropic", "mode": "api",
                                 "endpoint": "https://api.anthropic.com/v1/messages", "executable": None, "model": "claude-opus-5-5",
                                 "credential": "present", "support": {"state": "supported", "reason": None}, "updated_at": T1},
                                {"provider_id": "codex", "label": "Codex CLI", "kind": "codex_cli", "mode": "cli", "endpoint": None,
                                 "executable": "C:\\Tools\\codex.exe", "model": "default", "credential": "not_required",
                                 "support": {"state": "unsupported", "reason": "codex exec cannot disable its shell tool; a read-only sandbox still reads files"},
                                 "updated_at": T1}])
    return out


def save(root, rel, data):
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def generate(root: Path):
    save(root, SCHEMA_PATH, schema())
    ex = examples()
    for name, value in ex.items():
        save(root, f"{EXAMPLES_DIR}/{name}.json", value)
    return len(ex)


# ---------------------------------------------------------------------------------------------
# Validation (called by tools/validate_docs.py)

def semantics(record):
    """Invariants JSON Schema cannot express: references, digests, windows, hand-off rules."""
    if record["kind"] == "diagnosis_preview":
        p = record["preview"]
        digest = hashlib.sha256(canonical({"destination": p["destination"], "payload": p["payload"]})).hexdigest()
        assert p["payload_digest"] == digest, "payload digest does not bind destination and payload"
        assert p["payload_bytes"] == len(p["payload"]["body"].encode("utf-8")), "payload_bytes differs from the body"
        assert p["created_at"] < p["expires_at"], "preview expires before it was created"
        from datetime import datetime
        span = datetime.fromisoformat(p["expires_at"]) - datetime.fromisoformat(p["created_at"])
        assert span.total_seconds() <= PREVIEW_TTL, "preview window exceeds the budget"
        assert p["contains_user_additions"] == bool(p["request"]["statements"] or p["request"]["question"]), "user additions flag is wrong"
        evidence_check(p["evidence"])
    if record["kind"] == "diagnosis_record":
        d = record["diagnosis"]
        evidence_check(d["evidence"])
        ids = {i["evidence_id"] for i in d["evidence"]["items"]} | {g["gap_id"] for g in d["evidence"]["gaps"]}
        order = ["reviewable", "needs_context", "keep", "unsupported"]
        if d["result"]:
            r = d["result"]
            for c in r["answer"]["claims"]:
                assert set(c["refs"]) <= ids, "claim cites evidence that is not in the package"
            assert order.index(r["effective_recommendation"]) >= order.index(r["model_recommendation"]), "grounding loosened a recommendation"
            assert len(r["grounding"]) == len(r["answer"]["claims"]), "every claim is grounded"
            if d["facts"]["protection"] is not None:
                assert r["effective_recommendation"] in ("keep", "unsupported"), "a protected object is never less than keep"
            if r["effective_recommendation"] == "reviewable":
                f = d["facts"]
                assert f["plannable"] and f["protection"] is None and not f["partial_evidence"], "reviewable needs a plannable, unprotected candidate"
        plan = [h for h in d["handoff"] if h["action"] == "review_plan" and h["available"]]
        if plan:
            f = d["facts"]
            assert d["state"] == "completed" and d["freshness"]["state"] == "current", "only a fresh completed diagnosis hands off to a plan"
            assert f["plannable"] and f["protection"] is None and f["candidate_id"], "plan hand-off needs a plannable unprotected candidate"
            assert plan[0]["target"]["candidate_id"] == f["candidate_id"], "plan hand-off names exactly the diagnosed candidate"
        for h in d["handoff"]:
            assert h["available"] or h["action"] != "review_plan" or h["target"] is None, "an unavailable plan step has no target"
        assert d["created_at"] <= d["updated_at"], "reversed timestamps"


def evidence_check(ev):
    ids = [i["evidence_id"] for i in ev["items"]] + [g["gap_id"] for g in ev["gaps"]]
    assert len(ids) == len(set(ids)), "duplicate evidence id"
    assert "backup_unknown" in {g["code"] for g in ev["gaps"]}, "backup state must stay an explicit unknown"


def validate(root: Path, validator_cls, format_checker, load):
    s = load(root / SCHEMA_PATH)
    validator_cls.check_schema(s)
    v = validator_cls(s, format_checker=format_checker())
    ex = {}
    for path in sorted((root / EXAMPLES_DIR).glob("*.json")):
        value = load(path)
        v.validate(value)
        semantics(value)
        ex[path.stem] = value
    assert {e["kind"] for e in ex.values()} == {"diagnosis_preview", "diagnosis_record", "diagnosis_list", "diagnosis_config"}, "a diag kind lacks an example"
    rejected = []

    def reject(name, source, mutate):
        bad = copy.deepcopy(source)
        mutate(bad)
        try:
            v.validate(bad)
            semantics(bad)
        except Exception as exc:  # noqa: BLE001 - only validation/assertion errors are expected
            from jsonschema.exceptions import ValidationError
            if not isinstance(exc, (ValidationError, AssertionError, KeyError, ValueError)):
                raise
            rejected.append("diag-" + name)
            return
        raise AssertionError("diag negative case accepted: " + name)

    pv, cache, db, video = ex["preview-video"], ex["record-cache-reviewable"], ex["record-database-protected"], ex["record-video-needs-context"]
    reject("unknown-field", cache, lambda x: x["diagnosis"].update(auto_approve=True))
    reject("unknown-answer-field", cache, lambda x: x["diagnosis"]["result"]["answer"].update(shell="rm -rf"))
    reject("wrong-contract", cache, lambda x: x.update(contract="1.0"))
    reject("v1-envelope", cache, lambda x: x.update(schema_version="1.0"))
    reject("dangling-reference", video, lambda x: x["diagnosis"]["result"]["answer"]["claims"][0].update(refs=["ev_99"]))
    reject("too-many-questions", video, lambda x: x["diagnosis"]["result"]["answer"].update(
        questions=[{"question_id": "q1", "text": "?", "choices": []}] * 4))
    reject("summary-over-budget", video, lambda x: x["diagnosis"]["result"]["answer"].update(summary="长" * 281))
    reject("percent-safety", video, lambda x: x["diagnosis"]["result"]["answer"].update(recommendation=0.93))
    reject("completed-without-result", cache, lambda x: x["diagnosis"].update(result=None))
    reject("failed-with-result", ex["record-auth-failed"], lambda x: x["diagnosis"].update(result=cache["diagnosis"]["result"]))
    reject("expired-plan-handoff", ex["record-cache-expired"], lambda x: x["diagnosis"].update(handoff=cache["diagnosis"]["handoff"]))
    reject("protected-plan-handoff", db, lambda x: x["diagnosis"].update(handoff=cache["diagnosis"]["handoff"]))
    reject("loosened-recommendation", db, lambda x: x["diagnosis"]["result"].update(effective_recommendation="reviewable"))
    reject("protected-needs-context", db, lambda x: x["diagnosis"]["result"].update(effective_recommendation="needs_context"))
    reject("sibling-candidate", cache, lambda x: x["diagnosis"]["handoff"][0]["target"].update(candidate_id="c_sibling"))
    reject("tampered-payload", pv, lambda x: x["preview"]["payload"].update(body=x["preview"]["payload"]["body"].replace("u1", "u2")))
    reject("swapped-destination", pv, lambda x: x["preview"]["destination"].update(model="claude-sonnet-5-5"))
    reject("key-in-header", pv, lambda x: x["preview"]["payload"]["headers"].append("x-api-key: sk-ant-api03-abcdefghijklmnop"))
    reject("long-preview-window", pv, lambda x: x["preview"].update(expires_at="2026-10-16T09:30:00Z"))
    reject("missing-exclusion", pv, lambda x: x["preview"]["evidence"].update(excluded=x["preview"]["evidence"]["excluded"][:4]))
    reject("too-many-children", pv, lambda x: x["preview"]["evidence"].update(children=[{"name": "c", "kind": "file", "logical_bytes": "1"}] * 6))
    reject("backup-gap-removed", pv, lambda x: x["preview"]["evidence"].update(gaps=[g for g in x["preview"]["evidence"]["gaps"] if g["code"] != "backup_unknown"]))
    reject("session-without-expiry", video, lambda x: x["diagnosis"].update(retain_until=None))
    reject("running-finished", ex["record-cancelled"], lambda x: x["diagnosis"].update(state="running"))
    return len(ex), rejected
