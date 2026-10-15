"""Generate the proposed v1 schemas and synthetic examples; never scans/deletes user data."""
from __future__ import annotations
import base64
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STR = {"type": "string", "minLength": 1}
UINT = {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"}
SINT = {"type": "string", "pattern": "^(0|-?[1-9][0-9]*)$"}
COUNT = {"type": "integer", "minimum": 0, "maximum": 9007199254740991}
TIME = {"type": "string", "format": "date-time", "pattern": "Z$"}
HASH = {"type": "string", "pattern": "^[a-f0-9]{64}$"}


def obj(props, required=None):
    return {"type": "object", "properties": props, "required": list(props) if required is None else required, "additionalProperties": False}


def ref(name):
    return {"$ref": "#/$defs/" + name}


def nullable(value):
    return {"anyOf": [value, {"type": "null"}]}


def arr(value, minimum=0):
    return {"type": "array", "items": value, "minItems": minimum}


def enum(*values):
    return {"enum": list(values)}


def record(kind, props):
    return obj({"schema_version": {"const": "1.0"}, "kind": {"const": kind}, "request_id": STR, **props})


def canonical(body):
    return json.dumps(body, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")


def save(rel, data):
    p = ROOT / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main():
    defs = {}
    defs["path"] = obj({"platform": enum("windows", "macos"), "display": STR,
                        "raw_encoding": enum("windows_utf16le_base64", "unix_bytes_base64"),
                        "raw_base64": {"type": "string", "minLength": 4, "contentEncoding": "base64"}})
    defs["identity"] = obj({"volume_id": STR, "file_id": STR, "generation": nullable(STR),
                            "quality": enum("strong", "weak"), "observed_at": TIME})
    defs["issue"] = obj({"code": STR, "scope_display": STR, "message": STR})
    defs["capability"] = obj({"name": STR, "state": enum("available", "permission_required", "unsupported", "unverified"),
                              "reason": nullable(STR)})
    defs["capabilities"] = record("capabilities", {"platform": enum("windows", "macos"),
        "engine_version": STR, "supported_schema_versions": arr(STR, 1), "capabilities": arr(ref("capability"), 1)})
    defs["scan_result"] = record("scan_result", {"scan_id": STR, "scope_id": STR, "revision": COUNT,
        "backend": STR, "status": enum("completed", "partial", "cancelled", "failed"),
        "completeness": enum("verified_scope", "partial_scope"), "started_at": TIME, "finished_at": TIME,
        "entries_seen": COUNT, "skipped_ranges": COUNT, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT), "unknown_size_count": COUNT, "issues": arr(ref("issue"))})
    defs["candidate"] = record("candidate", {"candidate_id": STR, "scan_id": STR, "scan_revision": COUNT,
        "entry_id": STR, "rule_id": STR, "rule_version": STR, "risk": enum("low", "medium", "high", "blocked"),
        "path": ref("path"), "reason": STR, "impact": STR, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT), "reclaimable_estimate_bytes": nullable(UINT),
        "estimate_basis": enum("metadata_estimate", "shared_blocks_unknown", "not_computed"),
        "allowed_modes": {**arr(enum("trash", "quarantine", "delete")), "uniqueItems": True}})
    defs["plan_item"] = obj({"action_id": STR, "candidate_id": STR, "entry_id": STR,
        "path": ref("path"), "identity": ref("identity"), "observed_fingerprint_sha256": HASH,
        "logical_bytes": UINT, "allocated_bytes": nullable(UINT), "must_not_follow_links": {"const": True}})
    defs["plan_body"] = obj({"engine_contract": {"const": "plan/v1"}, "plan_id": STR, "scope_id": STR,
        "scan_id": STR, "scan_revision": COUNT, "rule_set_digest": HASH, "policy_digest": HASH,
        "created_at": TIME, "expires_at": TIME, "mode": enum("trash", "quarantine", "delete"),
        "items": arr(ref("plan_item"), 1)})
    defs["plan"] = record("plan", {"body": ref("plan_body"), "plan_digest": HASH,
        "authorization_status": enum("required", "authorized", "expired", "revoked")})
    defs["action_result"] = obj({"action_id": STR, "status": enum("succeeded", "skipped", "failed", "not_started", "unknown"),
        "code": STR, "message": STR, "recovery": enum("trash", "quarantine", "unavailable", "not_applicable", "unknown")})
    defs["operation_report"] = record("operation_report", {"operation_id": STR, "plan_id": STR,
        "plan_digest": HASH, "status": enum("completed", "partial", "cancelled", "failed", "reconciliation_required"),
        "started_at": TIME, "finished_at": TIME, "succeeded": COUNT, "skipped": COUNT, "failed": COUNT,
        "not_started": COUNT, "unknown": COUNT, "items": arr(ref("action_result")),
        "processed_logical_bytes": UINT, "volume_free_before_bytes": nullable(UINT),
        "volume_free_after_bytes": nullable(UINT), "observed_free_delta_bytes": nullable(SINT)})
    defs["error"] = record("error", {"code": enum("invalid_argument", "schema_mismatch", "needs_approval", "policy_denied",
        "permission_required", "plan_stale", "cursor_stale", "conflict", "unsupported", "io_error", "internal_error", "cancelled"),
        "message": STR, "retryable": {"type": "boolean"}, "required_action": nullable(STR),
        "details": obj({"plan_id": nullable(STR), "operation_id": nullable(STR), "scope_id": nullable(STR)})})
    defs["progress"] = record("progress", {"task_id": STR, "sequence": COUNT,
        "phase": enum("discovering", "indexing", "analyzing", "validating", "applying", "stopping"),
        "completed_units": COUNT, "total_units": nullable(COUNT), "emitted_at": TIME})
    kinds = ["capabilities", "scan_result", "candidate", "plan", "operation_report", "error", "progress"]
    schema = {"$schema": "https://json-schema.org/draft/2020-12/schema",
              "$id": "urn:cleanmanager:protocol:1.0", "title": "CleanManager proposed v1 record",
              "description": "Pre-implementation contract; examples are synthetic, not runtime evidence.",
              "oneOf": [ref(k) for k in kinds], "$defs": defs}
    save("schemas/v1/protocol.schema.json", schema)
    rule = obj({"schema_version": {"const": "1.0"}, "rule_id": STR, "version": STR,
        "status": enum("experimental", "verified", "enabled"), "platforms": {**arr(enum("windows", "macos"), 1), "uniqueItems": True},
        "root_resolver": enum("application_cache", "user_temp", "project_build_output"),
        "owner_app_id": STR, "relative_components": arr(STR), "min_age_days": {"type": "integer", "minimum": 1},
        "risk": enum("low", "medium", "high"), "allowed_modes": {**arr(enum("trash", "quarantine", "delete"), 1), "uniqueItems": True},
        "requires_owner_inactive": {"type": "boolean"}, "protect_descendants": {"const": True},
        "follow_links": {"const": False}, "impact": STR, "evidence_refs": arr(STR)})
    rule.update({"$schema": "https://json-schema.org/draft/2020-12/schema", "$id": "urn:cleanmanager:rule:1.0"})
    save("schemas/v1/rule.schema.json", rule)
    t0, t1 = "2026-10-14T12:00:00Z", "2026-10-14T12:00:02Z"
    path = {"platform": "windows", "display": "C:\\Users\\Example\\AppData\\Local\\ExampleApp\\Cache\\old.tmp",
            "raw_encoding": "windows_utf16le_base64"}
    path["raw_base64"] = base64.b64encode(path["display"].encode("utf-16-le")).decode("ascii")
    def rec(kind, **values):
        return {"schema_version": "1.0", "kind": kind, "request_id": "req_synthetic", **values}
    scan = rec("scan_result", scan_id="s_demo", scope_id="scope_demo", revision=1, backend="portable_walk",
        status="partial", completeness="partial_scope", started_at=t0, finished_at=t1, entries_seen=12,
        skipped_ranges=1, logical_bytes="104857600", allocated_bytes=None, unknown_size_count=1,
        issues=[{"code": "permission_required", "scope_display": "synthetic protected subtree", "message": "Synthetic access denial"}])
    candidate = rec("candidate", candidate_id="c_demo", scan_id="s_demo", scan_revision=1, entry_id="e_demo",
        rule_id="example.cache", rule_version="0.1.0", risk="low", path=path,
        reason="Synthetic verified-owner cache example", impact="Cache would need rebuilding",
        logical_bytes="104857600", allocated_bytes="104857600", reclaimable_estimate_bytes=None,
        estimate_basis="not_computed", allowed_modes=["trash"])
    body = {"engine_contract": "plan/v1", "plan_id": "p_demo", "scope_id": "scope_demo", "scan_id": "s_demo",
        "scan_revision": 1, "rule_set_digest": "1"*64, "policy_digest": "2"*64,
        "created_at": "2026-10-14T12:01:00Z", "expires_at": "2026-10-14T12:16:00Z", "mode": "trash",
        "items": [{"action_id": "a_demo", "candidate_id": "c_demo", "entry_id": "e_demo", "path": path,
                   "identity": {"volume_id": "synthetic-volume", "file_id": "synthetic-file", "generation": None,
                                "quality": "strong", "observed_at": t1}, "observed_fingerprint_sha256": "3"*64,
                   "logical_bytes": "104857600", "allocated_bytes": "104857600", "must_not_follow_links": True}]}
    digest = hashlib.sha256(canonical(body)).hexdigest()
    examples = {
        "capabilities": rec("capabilities", platform="windows", engine_version="0.0.0-design-example",
            supported_schema_versions=["1.0"], capabilities=[{"name": "scan.portable", "state": "available", "reason": None},
            {"name": "scan.ntfs", "state": "permission_required", "reason": "Synthetic elevated scanner requirement"}]),
        "scan-partial": scan, "candidate": candidate,
        "plan": rec("plan", body=body, plan_digest=digest, authorization_status="required"),
        "needs-approval": rec("error", code="needs_approval", message="Plan needs user authorization", retryable=False,
            required_action="Review the plan in the trusted UI", details={"plan_id": "p_demo", "operation_id": None, "scope_id": "scope_demo"}),
        "operation-trash": rec("operation_report", operation_id="op_demo", plan_id="p_demo", plan_digest=digest,
            status="completed", started_at="2026-10-14T12:02:00Z", finished_at="2026-10-14T12:02:01Z",
            succeeded=1, skipped=0, failed=0, not_started=0, unknown=0,
            items=[{"action_id": "a_demo", "status": "succeeded", "code": "moved_to_trash", "message": "Synthetic same-volume move", "recovery": "trash"}],
            processed_logical_bytes="104857600", volume_free_before_bytes="1000000000", volume_free_after_bytes="1000000000", observed_free_delta_bytes="0"),
        "progress": rec("progress", task_id="s_demo", sequence=0, phase="discovering", completed_units=12, total_units=None, emitted_at=t1)
    }
    for name, example in examples.items():
        save("examples/contracts/" + name + ".json", example)
    mac = json.loads(json.dumps(candidate))
    mac["path"] = {"platform": "macos", "display": "/Users/example/Library/Caches/ExampleApp/old.tmp",
                   "raw_encoding": "unix_bytes_base64", "raw_base64": base64.b64encode(b"/Users/example/Library/Caches/ExampleApp/old.tmp").decode("ascii")}
    save("examples/contracts/candidate-macos.json", mac)
    save("rules/examples/example-cache.json", {"schema_version": "1.0", "rule_id": "example.cache", "version": "0.1.0",
        "status": "experimental", "platforms": ["windows", "macos"], "root_resolver": "application_cache",
        "owner_app_id": "example.invalid.fixture-app", "relative_components": ["Cache"], "min_age_days": 14,
        "risk": "low", "allowed_modes": ["trash"], "requires_owner_inactive": True, "protect_descendants": True,
        "follow_links": False, "impact": "Synthetic rule only; no production cache path or cleanup support is claimed.", "evidence_refs": []})
    print("Generated 2 schemas, 8 protocol examples and 1 experimental rule example.")


if __name__ == "__main__":
    main()
