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


def fid(label):
    """Deterministic synthetic NTFS-style file ID (not a real file)."""
    return "ntfs64:" + hashlib.sha256(label.encode("utf-8")).hexdigest()[:16]


def save(rel, data):
    p = ROOT / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def main():
    defs = {}
    defs["path"] = obj({"platform": enum("windows", "macos"), "display": STR,
                        "raw_encoding": enum("windows_utf16le_base64", "unix_bytes_base64"),
                        "raw_base64": {"type": "string", "minLength": 4, "contentEncoding": "base64"}})
    defs["identity"] = obj({"volume_id": STR, "file_id": {"type": "string", "pattern": "^(ntfs64:[0-9a-f]{16}|fileid128:[0-9a-f]{32}|inode64:[0-9a-f]{16})$"}, "generation": nullable(STR),
                            "quality": enum("strong", "weak"), "observed_at": TIME})
    # docs/platforms/macos.md §6: the blocking layer behind a permission_required report, as far as it can be proven
    # (the message names it too). Optional: absent when the message names no cause; nothing is guessed.
    permission_cause = enum("undetermined", "privacy_prompt_pending")
    defs["issue"] = obj({"code": STR, "scope_display": STR, "message": STR, "permission_cause": permission_cause},
                        required=["code", "scope_display", "message"])
    defs["capability"] = obj({"name": STR, "state": enum("available", "permission_required", "unsupported", "unverified"),
                              "reason": nullable(STR)})
    defs["capabilities"] = record("capabilities", {"platform": enum("windows", "macos"),
        "engine_version": STR, "supported_schema_versions": arr(STR, 1), "capabilities": arr(ref("capability"), 1)})
    defs["scan_result"] = record("scan_result", {"scan_id": STR, "scope_id": STR, "revision": COUNT,
        "backend": STR, "status": enum("completed", "partial", "cancelled", "failed", "interrupted"),
        "completeness": enum("verified_scope", "partial_scope"), "started_at": TIME, "finished_at": TIME,
        "entries_seen": COUNT, "skipped_ranges": COUNT, "logical_bytes": nullable(UINT),
        "allocated_bytes": nullable(UINT), "unknown_size_count": COUNT, "issues": arr(ref("issue"))})
    # SCN-03: a failed scan has no usable subtotal, so its sizes are unknown (null), never "0"; every
    # other terminal state reports the known part of the confirmed rows ("0" only when that is zero).
    defs["scan_result"]["allOf"] = [
        {"if": {"properties": {"status": {"const": "failed"}}},
         "then": {"properties": {"logical_bytes": {"type": "null"}, "allocated_bytes": {"type": "null"}}},
         "else": {"properties": {"logical_bytes": UINT}}}]
    candidate_props = {"candidate_id": STR, "scan_id": STR, "scan_revision": COUNT,
        "entry_id": STR, "rule_id": STR, "rule_version": STR, "risk": enum("low", "medium", "high", "blocked"),
        "path": ref("path"), "reason": STR, "impact": STR, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT), "reclaimable_estimate_bytes": nullable(UINT),
        "estimate_basis": enum("metadata_estimate", "shared_blocks_unknown", "not_computed"),
        "allowed_modes": {**arr(enum("trash", "quarantine", "delete")), "uniqueItems": True}}
    defs["candidate"] = record("candidate", candidate_props)
    defs["candidate_item"] = obj(candidate_props)
    # ANA-02 / WFA-17: optional explanation fields, the same wording as the `clean` preview (absent in older records;
    # the rule-derived ones also when the current rule pack no longer has the candidate's rule version): category, tier
    # and why (rule ceiling, then every lowering code), rebuild and offline cost, the rule's age threshold and the time
    # it measures (never a last-use time), what getting the object back can do per offered mode, evidence references.
    defs["recovery_limit"] = obj({"mode": enum("trash", "quarantine", "delete"),
        "recovery": enum("system_recycle_bin", "in_app_restore", "unavailable"), "limit": STR})
    explanation = {"category": STR, "tier": enum("default_selected", "review_only", "blocked"), "tier_reason": STR,
        "rebuild_class": enum("local_rebuild", "network_redownload", "not_rebuildable"),
        "regeneration": nullable(STR), "offline_effect": nullable(STR),
        "age_threshold": nullable(obj({"min_age_days": COUNT, "basis": enum("modified_time", "created_time")})),
        "recovery_limits": arr(ref("recovery_limit")), "evidence_refs": arr(STR)}
    for name in ("candidate", "candidate_item"):
        defs[name]["properties"].update(explanation)
    # Read-side records (task 2.3): live scan status, cancel receipts, revision-bound pages.
    defs["scan_status"] = record("scan_status", {"scan_id": STR, "scope_id": STR,
        "state": enum("created", "running", "stopping"), "host": obj({"kind": enum("cli", "desktop"), "process_id": COUNT,
            "started_at": TIME, "last_heartbeat_at": TIME}), "cancel_requested": {"type": "boolean"},
        "entries_seen": COUNT, "skipped_ranges": COUNT, "known_logical_bytes": UINT, "last_issue": nullable(ref("issue"))})
    defs["scan_cancel_receipt"] = record("scan_cancel_receipt", {"scan_id": STR, "accepted": {"type": "boolean"},
        "state_at_receipt": enum("created", "running", "stopping", "already_terminal"), "requested_at": TIME,
        "note": {"const": "Receipt only: the scan stops at the next safe boundary; query scan show for the terminal record."}})
    page_size = {"type": "integer", "minimum": 1, "maximum": 500}
    # Write-side status records (task 2.4).
    defs["authorization_status"] = record("authorization_status", {"plan_id": STR, "plan_digest": HASH,
        "status": enum("required", "authorized", "expired", "revoked"),
        "source": nullable(enum("interactive_review", "policy")), "policy_id": nullable(STR), "policy_version": nullable(STR),
        "principal": nullable(STR), "authorized_at": nullable(TIME), "expires_at": nullable(TIME),
        "quota": nullable(obj({"window": enum("single_plan", "rolling_24h"), "used_bytes": UINT, "limit_bytes": UINT,
            "reserved_unknown_bytes": UINT, "used_items": COUNT, "limit_items": COUNT}))})
    defs["operation_status"] = record("operation_status", {"operation_id": STR, "plan_id": STR, "plan_digest": HASH,
        "state": enum("created", "validating", "running", "stopping"), "started_at": TIME, "cancel_requested": {"type": "boolean"},
        "succeeded": COUNT, "skipped": COUNT, "failed": COUNT, "unknown": COUNT, "remaining": COUNT,
        "host": obj({"kind": enum("cli", "desktop"), "process_id": COUNT, "last_heartbeat_at": TIME})})
    defs["quarantine_item"] = obj({"quarantine_id": STR, "plan_id": STR, "operation_id": STR, "original_path": ref("path"),
        "quarantined_path": ref("path"), "logical_bytes": UINT, "quarantined_at": TIME, "retention_reminder_at": TIME,
        # REC-03: a due reminder is a review prompt only; the item is never deleted because of it.
        "reminder_due": {"type": "boolean"},
        "state": enum("recoverable", "target_conflict", "volume_offline", "volume_changed", "identity_changed", "missing",
            "parent_changed", "manifest_incomplete", "unknown"),
        "restore_capability": enum("available", "unsupported", "unknown"),
        "available_actions": {**arr(enum("restore", "purge")), "uniqueItems": True},
        "metadata_restored": arr(enum("timestamps", "attributes", "acl", "xattrs", "alternate_streams"))})
    # REC-04 history: one row per operation with truthful recovery availability. WFA-40: every plan/v2 kind names
    # itself (process end, uninstall and its leftovers, updates, and the restores of startup, uninstall and maintenance
    # backups); `restore`/`purge` stay the quarantine recovery of recovery/v1.
    defs["operation_summary"] = obj({"operation_id": STR, "plan_id": STR,
        "plan_contract": nullable(enum("plan/v1", "plan/v2", "recovery/v1")),
        "action": nullable(enum("trash", "quarantine", "delete", "restore", "purge", "startup", "maintenance", "process", "update",
            "uninstall", "uninstall_leftovers", "uninstall_restore", "startup_restore", "maintenance_restore")),
        "state": enum("created", "validating", "running", "stopping", "completed", "partial", "cancelled", "failed", "reconciliation_required"),
        "started_at": TIME, "finished_at": nullable(TIME), "authorization_source": nullable(STR), "rule_refs": arr(STR),
        "counts": nullable(obj({"succeeded": COUNT, "skipped": COUNT, "failed": COUNT, "not_started": COUNT, "unknown": COUNT})),
        "processed_logical_bytes": nullable(UINT),
        "recovery": obj({"availability": enum("in_app_restore", "system_recycle_bin", "unavailable", "not_applicable", "unknown"),
            "guidance": STR})})
    defs["operation_list"] = record("operation_list", {"items": {**arr(ref("operation_summary")), "maxItems": 500},
        "page_size": {"type": "integer", "minimum": 1, "maximum": 500}, "next_cursor": nullable(STR)})
    defs["quarantine_list"] = record("quarantine_list", {"items": {**arr(ref("quarantine_item")), "maxItems": 500},
        "page_size": {"type": "integer", "minimum": 1, "maximum": 500}, "next_cursor": nullable(STR), "total_logical_bytes": UINT})
    defs["operation_cancel_receipt"] = record("operation_cancel_receipt", {"operation_id": STR, "accepted": {"type": "boolean"},
        "state_at_receipt": enum("created", "validating", "running", "stopping", "already_terminal"), "requested_at": TIME,
        "note": {"const": "Receipt only: no new actions start; completed actions are not undone."}})
    defs["entry_item"] = obj({"entry_id": STR, "parent_entry_id": nullable(STR), "object_id": STR, "path": ref("path"),
        "kind": enum("file", "directory", "symlink", "reparse", "other"), "logical_bytes": nullable(UINT),
        "allocated_bytes": nullable(UINT), "size_basis": enum("entity", "subtree_known", "unknown"),
        "unknown_descendants": nullable(COUNT), "link_count": nullable(COUNT),
        "boundary": nullable(enum("symlink", "junction", "mount_point", "other_volume", "cloud_provider")),
        "identity_quality": enum("strong", "weak")})
    # DIN-05: optional read-only "cleanable by hand" hint on a directory (absent = no basis confirmed);
    # never a selection, tier or protection decision.
    defs["entry_item"]["properties"]["cleanable_hint"] = obj({"basis": enum("build_artifact", "catalog_rule", "cachedir_tag"),
        "rule_id": nullable(STR), "detail": STR, "flow": enum("purge", "manual_trash")})
    defs["entries_page"] = record("entries_page", {"scan_id": STR, "scope_id": STR, "revision": COUNT,
        "sort": enum("logical_desc", "allocated_desc", "name_asc"), "filter": obj({"kind": enum("any", "file", "directory"),
            "name_contains": nullable(STR), "parent_entry_id": nullable(STR)}),
        "page_size": page_size, "items": {**arr(ref("entry_item")), "maxItems": 500}, "next_cursor": nullable(STR),
        "unknown_size_items": COUNT})
    defs["candidates_page"] = record("candidates_page", {"candidate_set_id": STR, "scan_id": STR, "revision": COUNT,
        "page_size": page_size, "items": {**arr(ref("candidate_item")), "maxItems": 500}, "next_cursor": nullable(STR)})
    defs["analysis_result"] = record("analysis_result", {"candidate_set_id": STR, "scan_id": STR, "scan_revision": COUNT,
        "rule_set_digest": HASH, "scan_completeness": enum("verified_scope", "partial_scope"), "candidate_count": COUNT,
        "blocked_count": COUNT, "pending_evidence_count": COUNT, "known_logical_bytes": UINT, "unknown_size_count": COUNT,
        "issues": arr(ref("issue"))})
    # CM-OBS-v1 metadata precondition record (internal; bound into plans by fingerprint).
    native_time = obj({"unit": enum("windows_filetime_100ns", "unix_nanoseconds"), "value": UINT})
    source = enum("directory_enumeration", "handle_query", "bulk_attributes", "stat", "volume_query")
    sourced = ["logical_bytes", "allocated_bytes", "modified", "change_marker", "link_count", "reparse_tag", "cloud"]
    defs["observation_record"] = obj({"contract": {"const": "obs/v1"}, "scope_root": ref("identity"), "volume_id": STR,
        "entity": ref("identity"), "parent_chain": arr(ref("identity"), 1), "path": ref("path"),
        "kind": enum("file", "directory", "symlink", "reparse", "other"), "logical_bytes": nullable(UINT),
        "allocated_bytes": nullable(UINT), "modified": nullable(native_time), "change_marker": nullable(native_time),
        "link_count": nullable({"type": "integer", "minimum": 0, "maximum": 4294967295}),
        "reparse_tag": nullable({"type": "integer", "minimum": 0, "maximum": 4294967295}),
        "cloud": enum("not_cloud", "hydrated", "placeholder", "unknown"),
        "field_sources": obj({f: nullable(source) for f in sourced})})
    defs["plan_item"] = obj({"action_id": STR, "candidate_id": STR, "entry_id": STR,
        "path": ref("path"), "identity": ref("identity"), "observed_fingerprint_sha256": HASH,
        "logical_bytes": UINT, "allocated_bytes": nullable(UINT), "must_not_follow_links": {"const": True}})
    # Optional (absent = file): a directory item is removed only when empty after its planned contents.
    defs["plan_item"]["properties"]["kind"] = enum("file", "directory")
    defs["plan_body"] = obj({"engine_contract": {"const": "plan/v1"}, "plan_id": STR, "scope_id": STR,
        "scan_id": STR, "scan_revision": COUNT, "rule_set_digest": HASH, "policy_digest": HASH,
        "created_at": TIME, "expires_at": TIME, "mode": enum("trash", "quarantine", "delete"),
        "items": arr(ref("plan_item"), 1)})
    # plan/v2 (add-mole-parity-suite): typed actions, plan kinds and per-kind allowed action sets.
    defs["plan_target"] = obj({"entry_id": STR, "candidate_id": nullable(STR), "path": ref("path"),
        "identity": ref("identity"), "observed_fingerprint_sha256": HASH, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT)})
    fs_action = {"target": ref("plan_target"), "target_location": enum("original", "platform_trash"),
        "must_not_follow_links": {"const": True}}
    action_props = {
        "fs_trash": fs_action, "fs_quarantine": fs_action, "fs_delete": fs_action,
        "tool_adapter": {"adapter_id": STR, "adapter_version": STR,
            "executable": obj({"path": ref("path"), "sha256": HASH}),
            "argv": arr({"type": "string", "maxLength": 512}), "preview_digest": HASH,
            "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": 3600}},
        "app_quit": {"app_instance_id": STR, "app_identity": STR, "force": enum("never", "ask_user")},
        "registry_backup_delete": {"hive": enum("HKCU", "HKLM"), "key_path": STR, "value_name": nullable(STR),
            "owner_evidence": STR, "backup_required": {"const": True}},
        "registry_restore": {"backup_id": STR, "hive": enum("HKCU", "HKLM"), "key_path": STR,
            "on_existing": {"const": "skip"}},
        "startup_toggle": {"startup_item_id": STR, "source": enum("run_key", "startup_folder", "scheduled_task",
            "store_startup_task", "sm_login_item", "sm_background_item", "launchd_user", "launchd_system"),
            "operation": enum("enable", "disable", "remove_with_backup", "restore"), "scope": enum("user", "system")},
        "maintenance_task": {"task_id": STR, "task_version": STR, "requires_admin": {"type": "boolean"}},
        "settings_change": {"setting_id": STR, "from_value": STR, "to_value": STR, "restore_on_exit": {"type": "boolean"}},
        # STS-04: one process instance (PID + exact creation time: Windows FILETIME ticks, macOS microseconds since the
        # epoch); a reused PID never matches. The same time for people is the optional `started_at_utc` (added below).
        "process_end": {"pid": {"type": "integer", "minimum": 0, "maximum": 4294967295}, "started_at": {"type": "string", "pattern": "^[0-9]{1,20}$"},
            "name": STR, "image_path": nullable(ref("path")), "image_sha256": nullable(HASH)},
    }
    for name, props in action_props.items():
        defs["action_" + name] = obj({"action_id": STR, "action_type": {"const": name}, **props})
    defs["plan_action"] = {"oneOf": [ref("action_" + name) for name in action_props]}
    # Optional, digest-covered bindings (parity 6.x/11.x): target kind (absent = file), the scanned members a whole-folder
    # trash moves, the recycled item a purge step belongs to, the observed startup state and the recorded prior state.
    defs["plan_target"]["properties"]["kind"] = enum("file", "directory")
    for name in ("fs_trash", "fs_quarantine", "fs_delete"):
        defs["action_" + name]["properties"]["subtree"] = obj({"items": COUNT, "logical_bytes": UINT, "members_sha256": HASH})
        defs["action_" + name]["properties"]["trash_entry_id"] = STR
    defs["action_startup_toggle"]["properties"].update({"definition": STR, "observed_sha256": HASH, "backup_id": STR})
    defs["action_registry_restore"]["properties"].update({"value_name": STR, "backup_sha256": HASH})
    # UNI-06: a leftover key delete binds the key's observed content; anything changed since planning is skipped.
    defs["action_registry_backup_delete"]["properties"]["observed_sha256"] = HASH
    # Parity 14.5: a volume task (Spotlight rebuild) binds the mount point chosen when planning.
    defs["action_maintenance_task"]["properties"]["volume"] = STR
    # WFA-93: the process start time in RFC 3339 UTC for people (digest-covered, informational; absent in older plans).
    defs["action_process_end"]["properties"]["started_at_utc"] = TIME
    kind_actions = {
        "cleanup": ["fs_trash", "fs_quarantine", "fs_delete", "tool_adapter"],
        "manual_selection": ["fs_trash", "fs_delete"],
        "uninstall": ["app_quit", "tool_adapter", "fs_trash", "fs_quarantine", "registry_backup_delete", "startup_toggle"],
        "maintenance": ["maintenance_task", "settings_change"],
        "startup": ["startup_toggle"],
        "update": ["app_quit", "tool_adapter"],
        "recovery": ["registry_restore", "startup_toggle", "settings_change"],
        "process": ["process_end"],
    }
    defs["plan_body_v2"] = obj({"engine_contract": {"const": "plan/v2"}, "plan_id": STR,
        "plan_kind": enum(*kind_actions), "policy_matchable": {"type": "boolean"}, "requires_admin": {"type": "boolean"},
        "scope_id": nullable(STR), "scan_id": nullable(STR), "scan_revision": nullable(COUNT),
        "rule_set_digest": HASH, "policy_digest": HASH, "created_at": TIME, "expires_at": TIME,
        "items": arr(ref("plan_action"), 1)})
    rules_by_kind = []
    for kind, allowed in kind_actions.items():
        item = {"oneOf": [ref("action_" + a) for a in allowed]}
        if kind == "manual_selection":
            # Manual selection only moves to the platform trash, or permanently deletes what is already in it.
            item = {"oneOf": [
                {"allOf": [ref("action_fs_trash"), {"properties": {"target_location": {"const": "original"}}}]},
                {"allOf": [ref("action_fs_delete"), {"properties": {"target_location": {"const": "platform_trash"}}}]}]}
        then = {"properties": {"items": {"items": item}}}
        if kind != "cleanup":
            then["properties"]["policy_matchable"] = {"const": False}
        if kind == "process":
            then["properties"]["items"]["maxItems"] = 1  # each process end is confirmed on its own
        rules_by_kind.append({"if": {"properties": {"plan_kind": {"const": kind}}}, "then": then})
    defs["plan_body_v2"]["allOf"] = rules_by_kind
    # recovery/v1 (REC-02/03): restore or purge quarantined items under the same review/apply service.
    defs["recovery_item"] = obj({"action_id": STR, "quarantine_id": STR, "quarantined_path": ref("path"),
        "quarantined_identity": ref("identity"), "original_parent_identity": ref("identity"), "original_path": ref("path"),
        "target": obj({"kind": enum("original", "alternate", "permanent_delete"), "path": nullable(ref("path"))}),
        "on_conflict": {"const": "skip"}, "logical_bytes": UINT})
    defs["recovery_body"] = obj({"engine_contract": {"const": "recovery/v1"}, "plan_id": STR,
        "recovery_kind": enum("restore", "purge"), "created_at": TIME, "expires_at": TIME,
        "items": arr(ref("recovery_item"), 1)})
    defs["recovery_body"]["allOf"] = [
        {"if": {"properties": {"recovery_kind": {"const": "restore"}}},
         "then": {"properties": {"items": {"items": {"properties": {"target": {"properties": {
             "kind": {"enum": ["original", "alternate"]}, "path": ref("path")}}}}}}}},
        {"if": {"properties": {"recovery_kind": {"const": "purge"}}},
         "then": {"properties": {"items": {"items": {"properties": {"target": {"properties": {
             "kind": {"const": "permanent_delete"}, "path": {"type": "null"}}}}}}}}}]
    defs["plan"] = record("plan", {"body": {"oneOf": [ref("plan_body"), ref("plan_body_v2"), ref("recovery_body")]}, "plan_digest": HASH,
        "authorization_status": enum("required", "authorized", "expired", "revoked")})
    # CLI-11 (parity 8.1): a plan made from a dry-run selection lists, outside the signed body, the
    # selected objects whose observed state changed since the scan; they are not in the plan.
    defs["plan"]["properties"]["stale_items"] = arr(obj({"object_id": STR, "reason": enum("missing", "changed"), "detail": STR}))
    # Uninstall stage 1 (D7): the leftovers it would probably leave, by group; an estimate only, never authorized.
    uninstall_groups = ("bundle", "caches_logs", "settings", "personal_data", "startup", "system")
    defs["plan"]["properties"]["leftover_estimate"] = {"type": "object", "propertyNames": enum(*uninstall_groups), "additionalProperties": COUNT}
    # UNI-11: one uninstall plan for several applications: each application's actions and estimate, and the
    # applications refused exactly as a single uninstall of them would be (left out; the others are unaffected).
    defs["plan"]["properties"]["uninstall_apps"] = arr(obj({"app_instance_id": STR, "name": STR, "action_ids": arr(STR, 1),
        "leftover_estimate": defs["plan"]["properties"]["leftover_estimate"]}), 1)
    defs["plan"]["properties"]["uninstall_refused"] = arr(obj({"app_instance_id": STR, "name": nullable(STR),
        "code": enum("invalid_argument", "policy_denied", "unsupported", "permission_required", "conflict", "io_error", "internal_error"),
        "message": STR, "settings_uri": nullable(STR)}))
    uninstall_app_outcome = obj({"app_instance_id": STR, "name": STR,
        "status": enum("completed", "partial", "failed", "skipped", "not_started", "unknown", "pending"),
        "code": nullable(STR), "message": nullable(STR), "action_ids": arr(STR, 1), "leftovers_reviewable": {"type": "boolean"}})
    defs["action_result"] = obj({"action_id": STR, "status": enum("succeeded", "skipped", "failed", "not_started", "unknown"),
        "code": STR, "message": STR, "recovery": enum("trash", "quarantine", "unavailable", "not_applicable", "unknown")})
    defs["operation_report"] = record("operation_report", {"operation_id": STR, "plan_id": STR,
        "plan_digest": HASH, "status": enum("completed", "partial", "cancelled", "failed", "reconciliation_required"),
        "started_at": TIME, "finished_at": TIME, "succeeded": COUNT, "skipped": COUNT, "failed": COUNT,
        "not_started": COUNT, "unknown": COUNT, "items": arr(ref("action_result")),
        # REC-05: processed vs moved-into-quarantine vs what the actions free by themselves, apart
        # from the observed (signed, unattributed) volume free-space change.
        "processed_logical_bytes": UINT, "quarantined_logical_bytes": UINT, "estimated_freed_bytes": UINT,
        "volume_free_before_bytes": nullable(UINT),
        "volume_free_after_bytes": nullable(UINT), "observed_free_delta_bytes": nullable(SINT),
        # R3-01: who authorized it (`interactive_review`, `policy:ID@VERSION`), the same value `operations list` shows;
        # an interrupted operation's provisional report carries it too, and a reconciliation never replaces it.
        "authorization_source": STR})
    # UNI-11: a stage-1 uninstall also reports each application's own result (optional; other plans omit it).
    defs["operation_report"]["properties"]["uninstall_apps"] = arr(uninstall_app_outcome, 1)
    # WFA-40: when `operations reconcile` last resolved unconfirmed actions (optional). The reconciliation is recorded next
    # to the operation's authorization source (review or policy), never in its place; history lists it too.
    defs["operation_report"]["properties"]["reconciled_at"] = TIME
    defs["operation_summary"]["properties"]["reconciled_at"] = TIME
    defs["error"] = record("error", {"code": enum("invalid_argument", "schema_mismatch", "needs_approval", "policy_denied",
        "permission_required", "plan_stale", "cursor_stale", "conflict", "unsupported", "io_error", "internal_error", "cancelled",
        "protection_unavailable", "index_unavailable"),
        "message": STR, "retryable": {"type": "boolean"}, "required_action": nullable(STR),
        "details": obj({"plan_id": nullable(STR), "operation_id": nullable(STR), "scope_id": nullable(STR)})})
    # WFA-42/43: how each stored policy relates to a cleanup plan now (read-only; quota is reserved only at apply time):
    # the one that would authorize it and, for the others, the failing condition. A `plan` record lists them while a
    # policy could still authorize the plan; a needs_approval error of a cleanup plan lists them in `details` (optional).
    defs["policy_match"] = obj({"policy_id": STR, "policy_version": STR, "state": enum("enabled", "paused", "revoked"),
        "covers": {"type": "boolean"}, "reason": nullable(enum("not_enabled", "expired", "other_principal", "scope_differs",
            "mode_differs", "rule_not_covered", "tier_not_default_selected", "size_unknown", "single_limit", "quota_exceeded")),
        "message": STR})
    defs["policy_match"]["allOf"] = [{"if": {"properties": {"covers": {"const": True}}},
        "then": {"properties": {"reason": {"type": "null"}}}, "else": {"properties": {"reason": {"type": "string"}}}}]
    defs["plan"]["properties"]["policy_matches"] = arr(ref("policy_match"))
    # The receipt that authorizes, or (once used and expired) authorized, this exact cleanup plan: a terminal review or a
    # policy by ID and version (optional; null when none).
    defs["plan"]["properties"]["authorized_by"] = nullable(obj({"source": enum("interactive_review", "policy"),
        "policy_id": nullable(STR), "policy_version": nullable(STR), "authorized_at": TIME}))
    defs["error"]["properties"]["details"]["properties"]["policy_matches"] = arr(ref("policy_match"))
    defs["progress"] = record("progress", {"task_id": STR, "sequence": COUNT,
        "phase": enum("discovering", "indexing", "analyzing", "validating", "applying", "stopping"),
        "completed_units": COUNT, "total_units": nullable(COUNT), "emitted_at": TIME})
    # CLI-10 command groups (add-mole-parity-suite): requests and read-model responses.
    tier = enum("default_selected", "review_only", "blocked")
    page = {"next_cursor": nullable(STR), "page_size": {"type": "integer", "minimum": 1, "maximum": 500}}
    selection = {"scan_id": nullable(STR), "cursor": nullable(STR), "page_size": {"type": "integer", "minimum": 1, "maximum": 500},
        "include_tiers": {**arr(enum("default_selected", "review_only"), 1), "uniqueItems": True},
        "object_ids": {**arr(STR), "uniqueItems": True}, "mode": nullable(enum("trash", "quarantine", "delete"))}
    command_params = {
        "clean.list": obj({"scan_id": nullable(STR), "cursor": nullable(STR), "page_size": selection["page_size"]}),
        "clean.preview": obj(selection), "clean.plan": obj(selection),
        "purge.roots.list": obj({}), "purge.roots.set": obj({"roots": arr(ref("path"), 1)}),
        "purge.preview": obj({**selection, "recent_days": {"type": "integer", "minimum": 0, "maximum": 365}}),
        "purge.plan": obj({**selection, "recent_days": {"type": "integer", "minimum": 0, "maximum": 365}}),
        "installer.preview": obj(selection), "installer.plan": obj(selection),
        "trash.plan": obj({"scan_id": STR, "entry_ids": {**arr(STR, 1), "uniqueItems": True}}),
        "trash.purge_plan": obj({"trash_entry_ids": {**arr(STR, 1), "uniqueItems": True}}),
        # Read-only (MTR-02/04/05): one entry's coverage, protection and holders; the current user's recycled items.
        "trash.inspect": obj({"scan_id": STR, "entry_id": STR}), "trash.list": obj({}),
        "insights.list": obj({"scan_id": nullable(STR)}),
        "apps.list": obj({"cursor": nullable(STR), "page_size": selection["page_size"]}),
        # UNI-11: one application, or several in one plan (`--instance` repeated).
        "apps.uninstall.plan": {**obj({"app_instance_id": STR, "app_instance_ids": {**arr(STR, 1), "uniqueItems": True, "maxItems": 20},
            "end_running": {"type": "boolean"}}, ["end_running"]),
            "oneOf": [{"required": ["app_instance_id"]}, {"required": ["app_instance_ids"]}]},
        # Parity 9.x: route list, grouped leftovers of a finished uninstall operation, their plan, orphans, key restores.
        "apps.uninstall.list": obj({}), "apps.uninstall.leftovers": obj({"operation_id": STR}),
        "apps.uninstall.leftovers_plan": obj({"operation_id": STR, "object_ids": {**arr(STR), "uniqueItems": True}}),
        "apps.uninstall.orphans": obj({}), "apps.uninstall.restore_plan": obj({"operation_id": STR}),
        # UNI-08 on macOS: the orphaned leftovers a person named, in their own reviewed Trash plan (never preselected).
        "apps.uninstall.orphans_plan": obj({"object_ids": {**arr(STR, 1), "uniqueItems": True}}),
        "apps.updates.check": obj({"app_instance_ids": arr(STR)}), "apps.updates.plan": obj({"app_instance_ids": arr(STR, 1)}),
        # Parity 10.3 (UPD-04), 12.3/12.4 (STS-04/05), 7.3 (DIN-03/04).
        "apps.updates.config": obj({"checks_enabled": nullable({"type": "boolean"})}),
        "process.list": obj({"sort": enum("cpu", "memory", "disk", "network", "name"), "search": nullable(STR),
            "limit": {"type": "integer", "minimum": 1, "maximum": 500}, "pid": nullable({"type": "integer", "minimum": 0, "maximum": 4294967295})}),
        "process.pin": obj({"name": STR, "pinned": {"type": "boolean"}}),
        "process.end_plan": obj({"pid": {"type": "integer", "minimum": 0, "maximum": 4294967295}}),
        "status.alerts": obj({"enabled": nullable({"type": "boolean"}), "cpu_threshold_percent": nullable({"type": "number", "minimum": 1, "maximum": 100}),
            "window_seconds": nullable({"type": "integer", "minimum": 10, "maximum": 3600}),
            "cooldown_seconds": nullable({"type": "integer", "minimum": 60, "maximum": 86400})}),
        "insights.overview": obj({"older_than_days": {"type": "integer", "minimum": 1, "maximum": 3650}}),
        "insights.downloads": obj({"older_than_days": {"type": "integer", "minimum": 1, "maximum": 3650}, "limit": {"type": "integer", "minimum": 1, "maximum": 500}}),
        "startup.list": obj({}), "startup.plan": obj({"startup_item_id": STR,
            "operation": enum("enable", "disable", "remove_with_backup", "restore")}),
        "startup.restore_plan": obj({"operation_id": STR}),
        "optimize.list": obj({"search": nullable(STR)}), "optimize.plan": obj({"task_ids": nullable({**arr(STR, 1), "uniqueItems": True})}),
        # MNT-06/07: read-only diagnosis and run history.
        "optimize.diagnose": obj({"window_seconds": {"type": "integer", "minimum": 1, "maximum": 30}}),
        "optimize.history": obj({"limit": {"type": "integer", "minimum": 1, "maximum": 500}}),
        "status.snapshot": obj({}), "status.watch": obj({"interval_seconds": {"type": "integer", "minimum": 1, "maximum": 3600},
            "max_samples": nullable({"type": "integer", "minimum": 1, "maximum": 100000})}),
        "protect.list": obj({}), "protect.add": obj({"entry": ref("protection_entry_input")}), "protect.remove": obj({"protection_id": STR}),
        "protect.check": obj({"path": ref("path"), "rule_id": nullable(STR)}),
        "doctor.report": obj({"redact": {"type": "boolean"}, "repair_index": {"type": "boolean"}}, required=["redact"]),
        "completion.generate": obj({"shell": enum("bash", "zsh", "fish", "powershell"), "write_profile": {"type": "boolean"}}),
        # LIF-01/02, MVP 14.1: `manifest` overrides the build's update channel (URL or local path of a signed envelope).
        "self.update.check": obj({"manifest": nullable(STR)}), "self.update.apply": obj({"manifest": nullable(STR)}),
        "self.remove.preview": obj({"keep_user_data": {"type": "boolean"}, "install_dir": nullable(STR)}),
        "self.install": obj({"dir": STR, "add_to_path": {"type": "boolean"}}),
    }
    defs["protection_entry_input"] = obj({"match": enum("exact", "glob", "descendants", "ancestors", "rule", "maintenance_task"),
        "path": nullable(ref("path")), "glob": nullable({"type": "string", "minLength": 1, "maxLength": 260}),
        "rule_id": nullable(STR), "task_id": nullable(STR), "note": nullable(STR)})
    for name, params in command_params.items():
        defs["params_" + name.replace(".", "_")] = params
    defs["cli_request"] = record("cli_request", {"command": enum(*command_params), "params": {"type": "object"},
        "output": enum("json", "jsonl", "human"), "non_interactive": {"type": "boolean"}, "dry_run": {"type": "boolean"}})
    defs["cli_request"]["allOf"] = [{"if": {"properties": {"command": {"const": name}}},
        "then": {"properties": {"params": ref("params_" + name.replace(".", "_"))}}} for name in command_params]
    defs["probe_result"] = obj({"probe": STR, "result": enum("in_use", "not_in_use", "unknown")})
    defs["preview_item"] = obj({"object_id": STR, "entry_id": nullable(STR), "path": ref("path"), "category": STR,
        "rule_id": nullable(STR), "rule_version": nullable(STR), "tier": tier, "tier_reason": STR,
        "rebuild_class": enum("local_rebuild", "network_redownload", "not_rebuildable"),
        "registry_status": enum("covered", "version_untested", "not_registered"), "selected_by_default": {"type": "boolean"},
        "skip_reason": nullable(enum("in_use", "probe_unknown", "protected", "hard_denied", "recent", "vcs_tracked",
            "mounted", "identity_changed", "partial_evidence", "admin_required")),
        "probes": arr(ref("probe_result")), "logical_bytes": nullable(UINT), "reclaimable_estimate_bytes": nullable(UINT),
        "impact": STR})
    # Purge/installer previews (PUR/INC): optional source label and notes; version control undecidable.
    defs["preview_item"]["properties"].update({"source_label": nullable(STR), "notes": arr(STR)})
    defs["preview_item"]["properties"]["skip_reason"]["anyOf"][0]["enum"].append("vcs_unknown")
    # ANA-02 / WFA-17: the `clean` preview carries the candidate's explanation fields too (optional).
    defs["preview_item"]["properties"].update({k: explanation[k] for k in ("regeneration", "offline_effect", "age_threshold", "recovery_limits")})
    defs["selection_preview"] = record("selection_preview", {"command": STR, "scan_id": nullable(STR), "scan_revision": nullable(COUNT),
        "dry_run": {"const": True}, "non_interactive": {"type": "boolean"}, "items": arr(ref("preview_item")), **page,
        "summary": obj({"selected_count": COUNT, "selected_known_bytes": UINT, "selected_unknown_size_count": COUNT,
            "review_only_count": COUNT, "blocked_count": COUNT, "skipped_count": COUNT}), "convertible_to_plan": {"type": "boolean"}})
    # CAT-01/03/05 `clean list`: categories, rules with tested-registry coverage, owner-tool adapter previews.
    defs["catalog_rule"] = obj({"rule_id": STR, "version": STR, "status": enum("experimental", "verified", "enabled"), "max_tier": tier,
        "effective_tier_ceiling": tier, "rebuild_class": enum("local_rebuild", "network_redownload", "not_rebuildable"),
        "action_kind": enum("filesystem", "tool_adapter", "review_only_row"), "adapter_ref": nullable(STR), "owner_id": STR,
        "installed_version": nullable(STR), "registry_status": enum("covered", "version_untested", "not_registered"),
        "tested_range": nullable(STR), "tested_at": nullable(STR), "evidence_refs": arr(STR), "summary": STR})
    defs["adapter_preview"] = obj({"adapter_id": STR, "version": STR, "tool_id": STR, "status": enum("available", "unavailable", "timed_out", "failed"),
        "reason": nullable(STR), "executable": nullable(STR), "executable_sha256": nullable(HASH), "script": nullable(STR),
        "script_sha256": nullable(HASH), "argv": arr(STR, 1), "exit_code": nullable({"type": "integer"}), "duration_ms": COUNT,
        "output_summary": {"type": "string", "maxLength": 1048576}, "reported_path": nullable(STR), "cleanup_argv": arr(STR, 1), "cleanup_effect": STR})
    defs["cleanup_catalog"] = record("cleanup_catalog", {"platform": enum("windows", "macos"), "catalog_version": STR, "rule_set_digest": HASH,
        "categories": arr(obj({"category": STR, "label": STR, "wave": {"type": "integer", "minimum": 1},
            "status": enum("available", "unverified", "unsupported"), "rules": arr(ref("catalog_rule"))})),
        "adapters": arr(ref("adapter_preview"))})
    capability_state = enum("available", "permission_required", "unsupported", "unverified")
    defs["app_instance"] = obj({"app_instance_id": STR, "name": STR, "version": nullable(STR),
        "source": enum("app_bundle", "homebrew", "app_store", "registry_uninstall", "msi", "msix_store", "winget", "scoop", "unknown"),
        "location": nullable(ref("path")), "publisher": nullable(STR), "identity_evidence": arr(STR),
        "uninstall_capability": obj({"state": capability_state, "reason": nullable(STR)}),
        "update_status": enum("up_to_date", "update_available", "cannot_check", "not_checked"), "protected_class": {"type": "boolean"}})
    defs["app_list"] = record("app_list", {"items": arr(ref("app_instance")), **page})
    defs["startup_item"] = obj({"startup_item_id": STR, "name": STR, "source": action_props["startup_toggle"]["source"],
        "definition": STR, "target": nullable(ref("path")), "publisher": nullable(STR),
        "publisher_evidence": enum("authenticode", "package_identity", "code_signature", "none"), "scope": enum("user", "system"),
        "state": enum("enabled", "disabled", "broken", "target_unreachable", "unknown"), "toggle_capability": capability_state,
        "protected": {"type": "boolean"}, "reason": nullable(STR)})
    defs["startup_list"] = record("startup_list", {"items": arr(ref("startup_item")), "issues": arr(ref("issue"))})
    # Manual trash (MTR-02/04/05): index facts only (nothing opened or hydrated), path data instead of a shell reveal.
    hit = obj({"path": ref("path"), "reason": STR})
    defs["trash_inspection"] = record("trash_inspection", {"scan_id": STR, "entry_id": STR, "path": ref("path"),
        "parent_display": nullable(STR), "object_kind": enum("file", "directory", "symlink", "reparse", "other"),
        "cloud": enum("not_cloud", "hydrated", "placeholder", "unknown"), "logical_bytes": nullable(UINT), "allocated_bytes": nullable(UINT),
        "files": COUNT, "directories": COUNT, "protected": arr(hit), "protected_count": COUNT, "coverage_gaps": arr(hit), "gap_count": COUNT,
        "trashable": {"type": "boolean"}, "refusal": nullable(obj({"code": STR, "reason": STR, "message": STR})),
        "holders": nullable(arr(STR)), "available_actions": {**arr(enum("show_path", "copy_path", "move_to_trash")), "uniqueItems": True},
        "guidance": STR})
    defs["trash_list"] = record("trash_list", {"items": arr(obj({"trash_item_id": STR, "bin_location": STR,
        "original_path": nullable(STR), "deleted_at": nullable(TIME), "object_kind": enum("file", "directory", "symlink", "reparse", "other"),
        "logical_bytes": nullable(UINT), "purge_capability": enum("available", "unsupported"), "reason": nullable(STR)})),
        "issues": arr(ref("issue"))})
    defs["maintenance_task_info"] = obj({"task_id": STR, "task_version": STR, "platform": enum("windows", "macos"), "title": STR,
        "problem": STR, "impact": STR, "requires_admin": {"type": "boolean"}, "applies_when": STR, "estimated_seconds": COUNT,
        "applicability": enum("applicable", "not_needed", "not_applicable", "unknown"), "applicability_reason": nullable(STR),
        "user_protected": {"type": "boolean"}, "reversible": {"type": "boolean"}})
    # MNT-05: what is deliberately not offered, with the reason (a search for it finds no task).
    defs["maintenance_catalog"] = record("maintenance_catalog", {"platform": enum("windows", "macos"), "query": nullable(STR),
        "items": arr(ref("maintenance_task_info")), "prohibited": arr(obj({"id": STR, "title": STR, "reason": STR})),
        "elevation": enum("already_elevated", "one_prompt_per_run", "unavailable")})
    outcome = enum("applied", "unchanged", "skipped", "unavailable", "attention", "failed")
    # MNT-03/04/07: per-task outcome with reason, how admin tasks were elevated, who ran it; never undone.
    defs["maintenance_task_run"] = obj({"task_id": STR, "task_version": STR, "requires_admin": {"type": "boolean"}, "outcome": outcome,
        "code": STR, "reason": STR, "elevated": {"type": "boolean"}, "started_at": TIME, "finished_at": TIME})
    run_props = {"operation_id": STR, "plan_id": STR, "plan_digest": HASH, "entry": enum("cli", "desktop"), "principal": STR,
        "elevation": enum("not_needed", "already_elevated", "granted", "declined", "unavailable"), "started_at": TIME, "finished_at": TIME,
        "items": arr(ref("maintenance_task_run")), "counts": obj({k: COUNT for k in outcome["enum"]}), "all_clear": {"type": "boolean"},
        "undo": {"const": "unavailable"}}
    defs["maintenance_run"] = obj(run_props)
    defs["maintenance_report"] = record("maintenance_report", run_props)
    defs["maintenance_history"] = record("maintenance_history", {"items": arr(ref("maintenance_run")), "undo": {"const": "unavailable"}, "note": STR})
    # MNT-06: read-only; findings name processes and next steps, nothing is ended or changed.
    defs["maintenance_diagnosis"] = record("maintenance_diagnosis", {"sampled_at": TIME,
        "window_seconds": {"type": "integer", "minimum": 1, "maximum": 30}, "read_only": {"const": True},
        "findings": arr(obj({"code": enum("runaway_process", "busy_process", "memory_pressure", "commit_pressure", "low_disk_space",
            "idle_virtual_machine", "attached_disk_image", "many_startup_items"), "severity": enum("info", "warning"), "subject": STR, "detail": STR,
            "process_id": nullable(COUNT), "duration_seconds": nullable(COUNT),
            "next_step": enum("status.processes", "status.memory", "analyze", "startup.list", "explorer")})),
        "top_processes": arr(obj({"process_id": COUNT, "name": STR, "image": nullable(STR), "cpu_core_percent": {"type": "number", "minimum": 0},
            "working_set_bytes": UINT})),
        "memory_load_percent": nullable({"type": "number", "minimum": 0, "maximum": 100}),
        "commit_used_percent": nullable({"type": "number", "minimum": 0, "maximum": 100}), "note": STR})
    defs["metric"] = obj({"name": STR, "availability": enum("available", "unavailable", "permission_required"),
        "value": nullable({"type": "number"}), "unit": STR, "reason": nullable(STR)})
    defs["status_snapshot"] = record("status_snapshot", {"sampled_at": TIME, "stale": {"type": "boolean"},
        "metrics": arr(ref("metric"), 1), "health": obj({"score": nullable({"type": "integer", "minimum": 0, "maximum": 100}),
            "weights": obj({"cpu": COUNT, "memory": COUNT, "disk": COUNT, "thermal": COUNT, "io": COUNT}),
            "lowered_by": arr(STR), "dimensions": arr(obj({"dimension": enum("cpu", "memory", "disk", "thermal", "io"), "metric": STR,
                "value": nullable({"type": "number"}), "full_score_at": {"type": "number"}, "zero_score_at": {"type": "number"},
                "score": nullable({"type": "integer", "minimum": 0, "maximum": 100})}))}),
        "diagnosis": obj({"code": STR, "message": STR, "link": nullable(STR)})})
    # STS-07 (optional): network totals count physical interfaces only; loopback is excluded, tunnel/virtual adapters are
    # listed separately; interfaces of unknown kind are counted and flag the total as possibly double counting.
    net_rate = {"type": "number", "minimum": 0}
    net_if = lambda *classes: obj({"name": STR, "class": enum(*classes), "received_bytes_per_sec": net_rate, "sent_bytes_per_sec": net_rate})
    defs["status_snapshot"]["properties"]["network"] = obj({"classification": enum("interface_type", "unavailable"),
        "received_bytes_per_sec": net_rate, "sent_bytes_per_sec": net_rate, "may_double_count": {"type": "boolean"},
        "counted": arr(net_if("physical", "unknown")), "separate": arr(net_if("tunnel", "virtual")), "loopback_excluded": COUNT, "note": STR})
    defs["protection_entry"] = obj({"protection_id": STR, "layer": enum("hard_deny", "default_category", "user"),
        "match": defs["protection_entry_input"]["properties"]["match"], "display": STR, "removable": {"type": "boolean"},
        "note": nullable(STR), "added_at": nullable(TIME)})
    defs["protection_list"] = record("protection_list", {"items": arr(ref("protection_entry"))})
    protection_layer = enum("hard_deny", "default_category", "user")
    defs["protection_check"] = record("protection_check", {"path": ref("path"), "decision": enum("allowed", "denied"),
        "layer": nullable(protection_layer), "reason": nullable(STR),
        "layers": arr(obj({"layer": protection_layer, "result": enum("clear", "denied", "unavailable"), "detail": STR,
            "protection_id": nullable(STR)}), 3)})
    defs["insight"] = obj({"insight_id": STR, "kind": STR, "title": STR, "path": nullable(ref("path")), "logical_bytes": nullable(UINT),
        "count": nullable(COUNT), "size_basis": enum("file_metadata", "directory_walk", "system_query", "allocated", "estimate", "not_measured"),
        "size_note": nullable(STR), "observed_at": nullable(TIME), "explanation": STR, "handling": enum("read_only_guidance",
            "open_system_settings", "open_owner_tool", "manual_trash_review", "installer_review"),
        "guidance": arr(obj({"tool": STR, "steps": STR, "requires_admin": {"type": "boolean"}, "impact": STR}))})
    defs["insight"]["properties"]["permission_cause"] = permission_cause  # optional, from size_note (§6)
    defs["insight_list"] = record("insight_list", {"items": arr(ref("insight"))})
    # LIF-04: capability matrix, permission context, configuration, error-code table, recent error codes
    # and store self-check; redacted by default, exported only after a previewed terminal confirmation.
    defs["doctor_report"] = record("doctor_report", {"generated_at": TIME, "redacted": {"type": "boolean"},
        "engine_version": STR, "platform": enum("windows", "macos"), "capabilities": arr(ref("capability")),
        "checks": arr(obj({"area": enum("platform", "permissions", "config", "store"), "check": STR,
            "status": enum("ok", "warning", "failed", "unknown"), "detail": STR})),
        "error_codes": arr(obj({"code": defs["error"]["properties"]["code"], "exit_code": {"type": "integer", "minimum": 0, "maximum": 255},
            "retryable": {"type": "boolean"}})),
        "recent_errors": arr(obj({"code": STR, "count": COUNT, "last_seen": nullable(TIME)})),
        "export": nullable(obj({"path": STR, "written": {"type": "boolean"}}))})
    # BUG-WFA-01: `doctor --repair-index` only. An unusable scan index is moved aside (kept, never deleted) and an empty
    # one is created; a healthy or missing index is left alone. Optional: absent without the flag.
    defs["doctor_report"]["properties"]["index_repair"] = obj({"status": enum("repaired", "not_needed"),
        "previous_state": enum("healthy", "missing", "corrupt", "incompatible"), "kept_files": arr(STR), "message": STR})
    defs["purge_roots"] = record("purge_roots", {"roots": arr(obj({"path": ref("path"), "source": enum("default", "user"),
        "exists": {"type": "boolean"}}))})
    defs["installer_locations"] = record("installer_locations", {"locations": arr(obj({"path": ref("path"),
        "source_label": enum("downloads", "desktop", "browser_downloads"), "exists": {"type": "boolean"}})),
        "max_depth": {"type": "integer", "minimum": 1, "maximum": 4}})
    # LIF-01: how this copy was installed; package-manager copies are never replaced (`manager_command` names their update).
    install_method = enum("standalone", "homebrew", "winget", "scoop", "npm", "cargo", "unpackaged", "desktop_bundle")
    defs["release_info"] = record("release_info", {"current_version": STR, "latest_version": nullable(STR), "release_id": nullable(STR),
        "install_method": install_method, "status": enum("update_available", "up_to_date", "use_package_manager", "cannot_check",
        "verification_failed", "incompatible"), "reason": nullable(STR), "checked_at": TIME, "manager_command": nullable(STR)})
    defs["self_update_result"] = record("self_update_result", {"status": enum("updated", "up_to_date", "verification_failed", "locked",
        "use_package_manager", "cannot_check", "incompatible", "rolled_back", "rollback_incomplete"), "from_version": STR,
        "to_version": nullable(STR), "signature_verified": {"type": "boolean"}, "checksum_verified": {"type": "boolean"},
        "reason": nullable(STR), "install_method": install_method, "manager_command": nullable(STR), "backup_path": nullable(STR)})
    # LIF-02/RES-07: the reviewed self-uninstall plan; removal happens only after a typed terminal confirmation of this digest.
    defs["self_remove_preview"] = record("self_remove_preview", {"status": enum("preview", "removed", "partial"),
        "remove": arr(obj({"item_id": STR, "component": enum("cli", "desktop", "resident", "helper", "caches", "startup_entry",
            "system_setting", "user_data"), "action": enum("remove_file", "remove_empty_dir", "remove_path_entry", "remove_profile_block",
            "remove_credential"), "display": STR, "outcome": nullable(enum("removed", "scheduled", "skipped", "failed")),
            "reason": nullable(STR)})),
        "keep": arr(obj({"data": enum("configuration", "protection_list", "history", "quarantine", "registry_backups"), "display": STR})),
        "quarantine_items": COUNT, "quarantine_logical_bytes": UINT, "system_settings_to_restore": arr(STR),
        "warnings": arr(obj({"code": STR, "message": STR})), "notes": arr(STR), "install_method": install_method,
        "uninstall_command": nullable(STR), "plan_digest": HASH, "requires_interactive_confirmation": {"const": True}})
    # MVP 14.1: copy this release into a chosen directory; never over files; other `cleanmanager` programs on PATH are reported.
    defs["self_install_result"] = record("self_install_result", {"status": enum("preview", "installed"), "version": STR,
        "install_dir": STR, "files": COUNT, "path_conflicts": arr(obj({"path": STR, "relation": enum("same_release", "different")})),
        "path_entry": enum("not_requested", "would_add", "added", "already_present", "refused_conflict", "unsupported", "failed"),
        "reason": nullable(STR), "requires_interactive_confirmation": {"const": True}})
    # LIF-03: `print` and `preview` never write; only a typed terminal confirmation reaches `installed`.
    defs["completion_script"] = record("completion_script", {"shell": enum("bash", "zsh", "fish", "powershell"),
        "action": enum("print", "preview", "already_installed", "installed"), "script": STR,
        "profile_path": nullable(ref("path")), "written": {"type": "boolean"}})
    kinds = ["capabilities", "scan_result", "candidate", "plan", "operation_report", "error", "progress",
             "scan_status", "scan_cancel_receipt", "entries_page", "candidates_page", "analysis_result",
             "authorization_status", "operation_status", "operation_cancel_receipt", "quarantine_list", "operation_list",
             "cli_request", "selection_preview", "app_list", "startup_list", "maintenance_catalog", "maintenance_report",
             "status_snapshot", "protection_list", "protection_check", "insight_list", "doctor_report", "purge_roots", "release_info",
             "self_update_result", "self_remove_preview", "completion_script", "cleanup_catalog", "self_install_result"]
    kinds.append("installer_locations")  # INC-01 declared locations
    kinds += ["trash_inspection", "trash_list"]  # MTR-02..05 read models
    kinds += ["maintenance_history", "maintenance_diagnosis"]  # MNT-06/07
    # Parity 12.3/12.4 (STS-04..06): process list with explanations, pins, opt-in sustained-CPU alerts.
    NUM, BOOL, U32 = {"type": "number"}, {"type": "boolean"}, {"type": "integer", "minimum": 0, "maximum": 4294967295}
    defs["signature"] = obj({"state": enum("signed", "catalog_signed", "unsigned", "unknown"), "publisher": nullable(STR), "reason": nullable(STR)})
    defs["process_row"] = obj({"pid": U32, "parent_pid": nullable(U32), "parent_name": nullable(STR), "name": STR,
        "image_path": nullable(ref("path")), "started_at": nullable({"type": "string", "pattern": "^[0-9]{1,20}$"}),
        # WFA-93: `started_at` is the exact identity key plans bind; people and agents read `started_at_utc`.
        "started_at_utc": nullable(TIME),
        "cpu_percent": nullable(NUM), "memory_bytes": nullable(UINT), "disk_bytes_per_sec": nullable(NUM), "network_bytes_per_sec": nullable(NUM),
        "signature": ref("signature"), "owner_app": nullable(STR), "identified": BOOL, "purpose": nullable(STR), "unidentified_reason": nullable(STR),
        "ai_diagnosis": nullable(obj({"available": {"const": True}, "hint": STR})), "pinned": BOOL,
        "end_capability": obj({"state": enum("available", "unsupported"),
            "protection": nullable(enum("critical_system", "os_critical", "clean_manager", "security_software", "other_user")), "reason": STR}),
        "notes": arr(STR)})
    # STS-04: an unidentified process always carries the AI-diagnosis entry; an identified one always has a purpose.
    defs["process_row"]["allOf"] = [
        {"if": {"properties": {"identified": {"const": False}}}, "then": {"properties": {"ai_diagnosis": {"type": "object"}, "purpose": {"type": "null"}}}},
        {"if": {"properties": {"identified": {"const": True}}}, "then": {"properties": {"purpose": STR}}}]
    defs["process_list"] = record("process_list", {"sampled_at": TIME, "window_ms": COUNT, "sort": enum("cpu", "memory", "disk", "network", "name"),
        "search": nullable(STR), "total": COUNT, "matched": COUNT, "unidentified": COUNT, "items": arr(ref("process_row")), "notes": arr(STR)})
    defs["process_pins"] = record("process_pins", {"pins": arr(STR)})
    defs["status_alert"] = record("status_alert", {"pid": U32, "name": STR, "image_path": nullable(STR), "min_cpu_percent": NUM,
        "threshold_percent": NUM, "window_seconds": COUNT, "since": TIME, "raised_at": TIME, "link": STR})
    defs["status_alert_settings"] = record("status_alert_settings", {"enabled": BOOL, "cpu_threshold_percent": {"type": "number", "minimum": 1, "maximum": 100},
        "window_seconds": {"type": "integer", "minimum": 10, "maximum": 3600}, "cooldown_seconds": {"type": "integer", "minimum": 60, "maximum": 86400}, "note": STR})
    # Parity 7.3 (DIN-03/04): overview cards (known size or unknown + observation time) and the old-downloads review.
    defs["insight_card"] = obj({"card_id": STR, "kind": enum("old_downloads", "virtual_machines", "snapshots", "system_files", "component_store",
            "previous_installation", "update_cache", "developer_tools", "model_libraries", "device_backups", "purgeable_space"), "title": STR,
        "logical_bytes": nullable(UINT), "count": nullable(COUNT), "size_note": nullable(STR), "observed_at": nullable(TIME),
        "link": obj({"view": enum("analyze", "insights.downloads", "insights"), "path": nullable(ref("path")), "insight_ids": arr(STR)})})
    defs["insight_card"]["allOf"] = [{"if": {"properties": {"logical_bytes": {"type": "null"}}}, "then": {"properties": {"size_note": STR}}}]
    defs["insight_card"]["properties"]["permission_cause"] = permission_cause  # optional, from size_note (§6)
    # An unreadable or pending Downloads folder has no known file count: item_count is null with the total, never 0.
    old_downloads_summary = obj({"older_than_days": COUNT, "item_count": nullable(COUNT), "total_logical_bytes": nullable(UINT), "observed_at": TIME})
    old_downloads_summary["properties"]["permission_cause"] = permission_cause  # optional, from the review note (§6)
    defs["insight_overview"] = record("insight_overview", {"generated_at": TIME, "cards": arr(ref("insight_card")),
        "old_downloads": old_downloads_summary})
    defs["old_downloads_review"] = record("old_downloads_review", {"root": nullable(ref("path")), "older_than_days": COUNT, "observed_at": TIME,
        "items": arr(obj({"path": ref("path"), "logical_bytes": UINT, "modified_at": TIME, "age_days": COUNT, "installer_like": BOOL,
            "selected": {"const": False}})),
        "total_logical_bytes": nullable(UINT), "item_count": nullable(COUNT), "shown": COUNT, "note": nullable(STR),
        "handling": {"const": "manual_trash_review"}, "guidance": STR})
    defs["old_downloads_review"]["properties"]["permission_cause"] = permission_cause  # optional, from note (§6)
    # Parity 10.x (UPD-01..04): "up to date" only with a channel that answered and a check time.
    update_route = enum("package_manager", "open_store", "open_app", "open_official_page", "none")
    defs["app_update"] = obj({"app_instance_id": STR, "name": STR, "current_version": nullable(STR), "available_version": nullable(STR),
        "channel": nullable(enum("winget", "microsoft_store", "scoop", "self_updater", "sparkle", "mac_app_store", "homebrew", "electron")),
        "channel_id": nullable(STR),
        "state": enum("update_available", "up_to_date", "cannot_check", "not_checked"), "reason": nullable(STR), "checked_at": nullable(TIME),
        "route": update_route, "official_url": nullable(STR)})
    defs["app_update"]["allOf"] = [
        {"if": {"properties": {"state": {"enum": ["up_to_date", "update_available"]}}},
         "then": {"properties": {"channel": {"enum": ["winget", "microsoft_store", "scoop", "sparkle", "mac_app_store", "homebrew", "electron"]},
                                  "checked_at": TIME}}},
        {"if": {"properties": {"state": {"const": "cannot_check"}}}, "then": {"properties": {"reason": STR}}}]
    defs["app_update_list"] = record("app_update_list", {"checks_enabled": BOOL, "checked": BOOL, "checked_at": nullable(TIME),
        "counts": obj({"update_available": COUNT, "up_to_date": COUNT, "cannot_check": COUNT, "not_checked": COUNT}), "items": arr(ref("app_update"))})
    defs["app_update_settings"] = record("app_update_settings", {"checks_enabled": BOOL, "periodic_checks": {"const": False},
        "last_checked_at": nullable(TIME), "note": STR})
    defs["app_update_plan"] = record("app_update_plan", {"plan": nullable(obj({"body": ref("plan_body_v2"), "plan_digest": HASH,
            "authorization_status": enum("required", "authorized", "expired", "revoked")})),
        "routed": arr(obj({"app_instance_id": STR, "name": STR, "route": update_route, "reason": STR, "url": nullable(STR)})), "next": nullable(STR)})
    kinds += ["process_list", "process_pins", "status_alert", "status_alert_settings", "insight_overview", "old_downloads_review",
              "app_update_list", "app_update_settings", "app_update_plan"]
    # Parity 9.x (UNI-01..10): uninstall routes and the grouped leftover review.
    defs["app_uninstall_route"] = obj({"app_instance_id": STR, "name": STR, "version": nullable(STR), "publisher": nullable(STR),
        "source": enum("registry_uninstall", "msi", "msix_store", "scoop", "npm_global", "app_bundle", "homebrew", "app_store", "unknown"),
        "location": nullable(STR),
        "last_used_at": nullable(TIME), "protected_class": {"type": "boolean"}, "uninstall": obj({"state": enum("available", "unsupported", "unverified"),
            "adapter": nullable(obj({"adapter_id": enum("windows.registered-uninstaller", "windows.msiexec-uninstall", "windows.msix-remove-package",
                "winget.uninstall", "scoop.uninstall", "npm.uninstall-global", "macos.trash-app-bundle", "homebrew.uninstall-cask",
                "homebrew.uninstall-formula"), "display": STR, "package_manager": {"type": "boolean"}})),
            "reason": nullable(STR), "settings_uri": nullable(STR)})})
    defs["app_uninstall_list"] = record("app_uninstall_list", {"items": arr(ref("app_uninstall_route")), "issues": arr(ref("issue"))})
    defs["uninstall_leftover"] = obj({"object_id": STR, "kind": enum("directory", "file", "registry_key", "startup_entry", "shortcut", "service"),
        "group": enum(*uninstall_groups), "display": STR, "evidence": arr(STR), "marker": nullable(enum("shared_owner", "owner_unknown", "app_reappeared")),
        "tier": enum("default_selected", "review_only", "blocked", "kept"), "selected": {"type": "boolean"}, "warnings": arr(STR),
        "logical_bytes": nullable(UINT), "note": nullable(STR), "requires_admin": {"type": "boolean"},
        "action": enum("fs_trash", "registry_backup_delete", "startup_toggle", "manual"), "guidance": nullable(STR)})
    # UNI-05: only caches and logs may be preselected; kept (marked) and blocked objects never are.
    defs["uninstall_leftover"]["allOf"] = [
        {"if": {"properties": {"selected": {"const": True}}}, "then": {"properties": {"group": {"const": "caches_logs"}, "tier": {"const": "default_selected"}}}},
        {"if": {"required": ["marker"], "properties": {"marker": {"type": "string"}}}, "then": {"properties": {"tier": {"const": "kept"}}}}]
    leftover_app = obj({"app_instance_id": STR, "name": STR, "publisher": nullable(STR),
        "source": enum("registry_uninstall", "msi", "msix", "scoop", "npm_global", "app_bundle", "homebrew", "app_store", "unknown")})
    leftover_groups = arr(obj({"group": enum(*uninstall_groups), "items": arr(ref("uninstall_leftover"), 1)}))
    defs["uninstall_leftovers"] = record("uninstall_leftovers", {"operation_id": nullable(STR),
        "app": nullable(leftover_app), "groups": leftover_groups, "default_selected": COUNT, "external_references": arr(STR)})
    # UNI-11: the same review per application of the operation (optional; one entry for a single uninstall). An
    # application whose uninstaller failed, was skipped or did not run lists no objects: it is still installed.
    defs["uninstall_leftovers"]["properties"]["apps"] = arr(obj({"app": leftover_app,
        "status": uninstall_app_outcome["properties"]["status"], "code": nullable(STR), "message": nullable(STR),
        "leftovers_reviewable": {"type": "boolean"}, "groups": leftover_groups, "default_selected": COUNT, "external_references": arr(STR)}), 1)
    kinds += ["app_uninstall_list", "uninstall_leftovers"]
    # D15: capabilities whose release gate has not passed are off unless experimental features are on.
    defs["experimental_features"] = record("experimental_features", {"experimental_features": {"type": "boolean"},
        "capabilities": arr(obj({"capability": STR, "gate": enum("G1", "G2", "G3", "G4", "AID"), "gate_passed": {"type": "boolean"},
            "enabled": {"type": "boolean"}}), 1), "note": STR})
    kinds += ["experimental_features"]
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
        reason="example.cache@0.1.0: Synthetic verified-owner cache files. Age threshold: modified at least 7 day(s) before the analysis, by its modification time (when it was last used is not known)",
        impact="Cache would need rebuilding",
        logical_bytes="104857600", allocated_bytes="104857600", reclaimable_estimate_bytes=None,
        estimate_basis="not_computed", allowed_modes=["trash"],
        category="app_caches", tier="default_selected", tier_reason="rule example.cache@0.1.0 allows default selection (rebuilt locally by its owner when needed)",
        rebuild_class="local_rebuild", regeneration="Rebuilt by the example app on its next start.",
        offline_effect="None: the cache is rebuilt from local data.", age_threshold={"min_age_days": 7, "basis": "modified_time"},
        recovery_limits=[{"mode": "trash", "recovery": "system_recycle_bin",
            "limit": "restored only with the system's own Recycle Bin tools; emptying the Recycle Bin, or the Recycle Bin discarding old items when it is full, makes it unrecoverable; frees no space until the Recycle Bin is emptied; a volume without a usable Recycle Bin refuses the plan"}],
        evidence_refs=["synthetic_fixture"])
    body = {"engine_contract": "plan/v1", "plan_id": "p_demo", "scope_id": "scope_demo", "scan_id": "s_demo",
        "scan_revision": 1, "rule_set_digest": "1"*64, "policy_digest": "2"*64,
        "created_at": "2026-10-14T12:01:00Z", "expires_at": "2026-10-14T12:16:00Z", "mode": "trash",
        "items": [{"action_id": "a_demo", "candidate_id": "c_demo", "entry_id": "e_demo", "path": path,
                   "identity": {"volume_id": "synthetic-volume", "file_id": fid("file"), "generation": None,
                                "quality": "strong", "observed_at": t1}, "observed_fingerprint_sha256": "3"*64,
                   "logical_bytes": "104857600", "allocated_bytes": "104857600", "must_not_follow_links": True}]}
    digest = hashlib.sha256(canonical(body)).hexdigest()
    examples = {
        "capabilities": rec("capabilities", platform="windows", engine_version="0.0.0-design-example",
            supported_schema_versions=["1.0"], capabilities=[{"name": "scan.portable", "state": "available", "reason": None},
            {"name": "scan.ntfs", "state": "permission_required", "reason": "Synthetic elevated scanner requirement"}]),
        "scan-partial": scan, "candidate": candidate,
        "plan": rec("plan", body=body, plan_digest=digest, authorization_status="required"),
        "plan-from-preview-stale": rec("plan", body=body, plan_digest=digest, authorization_status="required",
            stale_items=[{"object_id": "c_gone", "reason": "missing", "detail": "C:\\Users\\Example\\AppData\\Local\\ExampleApp\\Cache\\old2.tmp no longer exists"}]),
        "needs-approval": rec("error", code="needs_approval", message="Plan needs user authorization", retryable=False,
            required_action="Review the plan in the trusted UI", details={"plan_id": "p_demo", "operation_id": None, "scope_id": "scope_demo"}),
        # MTR-03 / WFA-35: a trash-mode plan on a volume without a usable bin is refused when it is created.
        "trash-unavailable": rec("error", code="unsupported",
            message="trash_unavailable: no usable Recycle Bin on W:\\ (the Recycle Bin is turned off for this volume (its Recycle Bin properties say \"Don't move files to the Recycle Bin. Remove files immediately when deleted.\")); there is no permanent-delete alternative",
            retryable=False,
            required_action="Create the plan again with --mode quarantine, or turn the Recycle Bin on for it (right-click the Recycle Bin › Properties › select the volume › Custom size; a policy setting has to be changed by its administrator) and plan again; affected: W:\\",
            details={"plan_id": None, "operation_id": None, "scope_id": None}),
        "operation-trash": rec("operation_report", operation_id="op_demo", plan_id="p_demo", plan_digest=digest,
            status="completed", started_at="2026-10-14T12:02:00Z", finished_at="2026-10-14T12:02:01Z",
            succeeded=1, skipped=0, failed=0, not_started=0, unknown=0,
            items=[{"action_id": "a_demo", "status": "succeeded", "code": "moved_to_trash", "message": "Synthetic same-volume move", "recovery": "trash"}],
            processed_logical_bytes="104857600", quarantined_logical_bytes="0", estimated_freed_bytes="0",
            volume_free_before_bytes="1000000000", volume_free_after_bytes="1000000000", observed_free_delta_bytes="0",
            authorization_source="interactive_review"),
        "progress": rec("progress", task_id="s_demo", sequence=0, phase="discovering", completed_units=12, total_units=None, emitted_at=t1)
    }
    for name, example in examples.items():
        save("examples/contracts/" + name + ".json", example)
    mac = json.loads(json.dumps(candidate))
    mac["path"] = {"platform": "macos", "display": "/Users/example/Library/Caches/ExampleApp/old.tmp",
                   "raw_encoding": "unix_bytes_base64", "raw_base64": base64.b64encode(b"/Users/example/Library/Caches/ExampleApp/old.tmp").decode("ascii")}
    for limit in mac["recovery_limits"]:
        limit["limit"] = limit["limit"].replace("Recycle Bin", "Trash")
    save("examples/contracts/candidate-macos.json", mac)
    # plan/v2 golden vectors: every action type appears in at least one synthetic plan.
    ident = {"volume_id": "synthetic-volume", "file_id": fid("file"), "generation": None, "quality": "strong", "observed_at": t1}
    def target(entry, display, raw_path=None):
        p = {"platform": "windows", "display": display, "raw_encoding": "windows_utf16le_base64",
             "raw_base64": base64.b64encode(display.encode("utf-16-le")).decode("ascii")}
        return {"entry_id": entry, "candidate_id": None, "path": p, "identity": {**ident, "file_id": fid(entry)},
                "observed_fingerprint_sha256": "3"*64, "logical_bytes": "1048576", "allocated_bytes": "1048576"}
    def fs(action_id, kind, entry, display, location="original"):
        return {"action_id": action_id, "action_type": kind, "target": target(entry, display),
                "target_location": location, "must_not_follow_links": True}
    adapter = {"action_type": "tool_adapter", "adapter_id": "example.adapter.cache-clean-unreferenced", "adapter_version": "0.1.0",
               "executable": {"path": {"platform": "windows", "display": "C:\\Example\\tool.exe", "raw_encoding": "windows_utf16le_base64",
                   "raw_base64": base64.b64encode("C:\\Example\\tool.exe".encode("utf-16-le")).decode("ascii")}, "sha256": "4"*64},
               "argv": ["cache", "clean", "--unreferenced"], "preview_digest": "5"*64, "timeout_seconds": 120}
    def wpath_early(display):
        return {"platform": "windows", "display": display, "raw_encoding": "windows_utf16le_base64",
                "raw_base64": base64.b64encode(display.encode("utf-16-le")).decode("ascii")}
    def plan_v2(plan_id, kind, items, policy_matchable=False, requires_admin=False):
        body = {"engine_contract": "plan/v2", "plan_id": plan_id, "plan_kind": kind, "policy_matchable": policy_matchable,
                "requires_admin": requires_admin, "scope_id": "scope_demo", "scan_id": "s_demo", "scan_revision": 1,
                "rule_set_digest": "1"*64, "policy_digest": "2"*64, "created_at": "2026-10-15T12:00:00Z",
                "expires_at": "2026-10-15T12:15:00Z", "items": items}
        return rec("plan", body=body, plan_digest=hashlib.sha256(canonical(body)).hexdigest(), authorization_status="required")
    v2 = {
        "plan-v2-cleanup": plan_v2("p_v2_cleanup", "cleanup", [
            fs("a01", "fs_trash", "e_cache", "C:\\Users\\Example\\AppData\\Local\\ExampleApp\\Cache"),
            fs("a02", "fs_quarantine", "e_logs", "C:\\Users\\Example\\AppData\\Local\\ExampleApp\\Logs"),
            fs("a03", "fs_delete", "e_tmp", "C:\\Users\\Example\\AppData\\Local\\Temp\\old.tmp"),
            {"action_id": "a04", **adapter}], policy_matchable=True),
        "plan-v2-manual-selection": plan_v2("p_v2_manual", "manual_selection", [
            fs("a01", "fs_trash", "e_video", "D:\\Videos\\example.mov")]),
        "plan-v2-manual-trash-purge": plan_v2("p_v2_manual_purge", "manual_selection", [
            fs("a01", "fs_delete", "e_trashed", "D:\\$Recycle.Bin\\S-1-5-21-example\\$Rexample.mov", "platform_trash")]),
        # A whole folder: one trash action binding every scanned member (MTR-02 coverage), plus a disable/enable record change.
        "plan-v2-manual-folder": plan_v2("p_v2_manual_folder", "manual_selection", [
            {**fs("a01", "fs_trash", "e_folder", "D:\\Projects\\old-renders"), "target": {**target("e_folder", "D:\\Projects\\old-renders"),
             "kind": "directory"}, "subtree": {"items": 42, "logical_bytes": "1048576",
             "members_sha256": "6" * 64}}]),
        "plan-v2-startup-disable": plan_v2("p_v2_startup_disable", "startup", [
            {"action_id": "s0000000", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "run_key",
             "operation": "disable", "scope": "user", "definition": "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run : Example Updater",
             "observed_sha256": "7" * 64}]),
        "plan-v2-uninstall": plan_v2("p_v2_uninstall", "uninstall", [
            {"action_id": "a01", "action_type": "app_quit", "app_instance_id": "app_demo", "app_identity": "Example.App", "force": "ask_user"},
            {"action_id": "a02", **adapter, "adapter_id": "example.adapter.vendor-uninstaller", "argv": []},
            fs("a03", "fs_trash", "e_appdata", "C:\\Users\\Example\\AppData\\Roaming\\ExampleApp\\Cache"),
            fs("a04", "fs_quarantine", "e_shortcut", "C:\\Users\\Example\\Desktop\\ExampleApp.lnk"),
            {"action_id": "a05", "action_type": "registry_backup_delete", "hive": "HKCU", "key_path": "Software\\ExampleVendor\\ExampleApp",
             "value_name": None, "owner_evidence": "uninstall entry publisher and product code", "backup_required": True},
            {"action_id": "a06", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "run_key",
             "operation": "remove_with_backup", "scope": "user"}]),
        "plan-v2-maintenance": plan_v2("p_v2_maintenance", "maintenance", [
            {"action_id": "a01", "action_type": "maintenance_task", "task_id": "windows.dns-flush", "task_version": "1.0.0", "requires_admin": False},
            {"action_id": "a02", "action_type": "maintenance_task", "task_id": "windows.trim-when-due", "task_version": "1.0.0", "requires_admin": True},
            {"action_id": "a03", "action_type": "settings_change", "setting_id": "display.keep_awake", "from_value": "off",
             "to_value": "on_60_minutes", "restore_on_exit": True}], requires_admin=True),
        "plan-v2-startup": plan_v2("p_v2_startup", "startup", [
            {"action_id": "a01", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "scheduled_task",
             "operation": "disable", "scope": "user"}]),
        "plan-v2-recovery": plan_v2("p_v2_recovery", "recovery", [
            {"action_id": "a01", "action_type": "registry_restore", "backup_id": "rb_demo", "hive": "HKCU",
             "key_path": "Software\\ExampleVendor\\ExampleApp", "on_existing": "skip"},
            {"action_id": "a02", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "run_key",
             "operation": "restore", "scope": "user", "backup_id": "rb_demo_toggle"},
            {"action_id": "a03", "action_type": "settings_change", "setting_id": "display.keep_awake", "from_value": "on_60_minutes",
             "to_value": "off", "restore_on_exit": False}]),
        "plan-v2-process": plan_v2("p_v2_process", "process", [
            {"action_id": "a01", "action_type": "process_end", "pid": 4242, "started_at": "134050000000000000",
             "started_at_utc": "2025-10-15T11:06:40Z", "name": "example.exe",
             "image_path": wpath_early(r"C:\Users\Example\AppData\Local\Example\example.exe"), "image_sha256": "7" * 64}]),
    }
    # UNI-11: one stage-1 plan for two applications (each its own close request and uninstaller); a third, protected
    # one was refused as it would be on its own and is not in the plan.
    def uninstaller(action_id, program):
        return {**adapter, "action_id": action_id, "adapter_id": "windows.registered-uninstaller", "adapter_version": "1",
                "executable": {"path": wpath_early(program), "sha256": "4" * 64}, "argv": [], "timeout_seconds": 3600}
    batch = plan_v2("up_batch", "uninstall", [
        {"action_id": "q0000000", "action_type": "app_quit", "app_instance_id": "app_demo", "app_identity": "C:\\Program Files\\Example App", "force": "never"},
        uninstaller("u0000001", "C:\\Program Files\\Example App\\uninst.exe"),
        {"action_id": "u0010000", "action_type": "app_quit", "app_instance_id": "app_tool", "app_identity": "C:\\Tools\\Example Tool", "force": "never"},
        uninstaller("u0010001", "C:\\Tools\\Example Tool\\unins000.exe")])
    batch["leftover_estimate"] = {"caches_logs": 3, "settings": 2, "bundle": 2}
    batch["uninstall_apps"] = [
        {"app_instance_id": "app_demo", "name": "Example App", "action_ids": ["q0000000", "u0000001"], "leftover_estimate": {"caches_logs": 2, "settings": 1, "bundle": 1}},
        {"app_instance_id": "app_tool", "name": "Example Tool", "action_ids": ["u0010000", "u0010001"], "leftover_estimate": {"caches_logs": 1, "settings": 1, "bundle": 1}}]
    batch["uninstall_refused"] = [{"app_instance_id": "app_agent", "name": "Example Endpoint Agent", "code": "policy_denied",
        "message": "Security or device-management agent: CleanManager offers no uninstall; remove it through your administrator or the vendor's official uninstall process",
        "settings_uri": None}]
    v2["plan-v2-uninstall-batch"] = batch
    for name, example in v2.items():
        save("examples/contracts/" + name + ".json", example)
    # CLI-10 fixtures (synthetic; no real application, metric or path is claimed).
    def wpath(display):
        return {"platform": "windows", "display": display, "raw_encoding": "windows_utf16le_base64",
                "raw_base64": base64.b64encode(display.encode("utf-16-le")).decode("ascii")}
    def item(oid, display, cat, t, reason, rebuild, selected, skip=None, probes=(), size="1048576"):
        return {"object_id": oid, "entry_id": "e_" + oid, "path": wpath(display), "category": cat,
                "rule_id": "example.browser.code-cache", "rule_version": "0.1.0", "tier": t, "tier_reason": reason,
                "rebuild_class": rebuild, "registry_status": "covered" if t == "default_selected" else "version_untested",
                "selected_by_default": selected, "skip_reason": skip, "probes": [{"probe": p, "result": r} for p, r in probes],
                "logical_bytes": size, "reclaimable_estimate_bytes": None, "impact": "Synthetic impact text"}
    maintenance_run = {"operation_id": "op_maint", "plan_id": "p_v2_maintenance", "plan_digest": "3" * 64, "entry": "cli",
        "principal": "user:EXAMPLE\\alice", "elevation": "declined", "started_at": "2026-10-15T12:00:00Z", "finished_at": "2026-10-15T12:00:02Z",
        "items": [{"task_id": "windows.dns-flush", "task_version": "1.0.0", "requires_admin": False, "outcome": "applied", "code": "flushed",
                   "reason": "DNS resolver cache flushed", "elevated": False, "started_at": "2026-10-15T12:00:01Z", "finished_at": "2026-10-15T12:00:01Z"},
                  {"task_id": "windows.trim-when-due", "task_version": "1.0.0", "requires_admin": True, "outcome": "skipped",
                   "code": "elevation_declined", "reason": "administrator authorization was declined; not run", "elevated": False,
                   "started_at": "2026-10-15T12:00:01Z", "finished_at": "2026-10-15T12:00:01Z"}],
        "counts": {"applied": 1, "unchanged": 0, "skipped": 1, "unavailable": 0, "attention": 0, "failed": 0}, "all_clear": True,
        "undo": "unavailable"}
    cli = {
        "request-clean-preview": rec("cli_request", command="clean.preview", output="json", non_interactive=True, dry_run=True,
            params={"scan_id": "s_demo", "cursor": None, "page_size": 50, "include_tiers": ["default_selected"], "object_ids": [], "mode": None}),
        "request-apps-uninstall": rec("cli_request", command="apps.uninstall.plan", output="json", non_interactive=True, dry_run=False,
            params={"app_instance_id": "app_demo", "end_running": False}),
        "preview-clean": rec("selection_preview", command="clean.preview", scan_id="s_demo", scan_revision=1, dry_run=True,
            non_interactive=True, next_cursor=None, page_size=50, convertible_to_plan=True, items=[
                item("o1", "C:\\Users\\Example\\AppData\\Local\\FixtureBrowser\\Code Cache", "browsers", "default_selected",
                     "Verified local cache", "local_rebuild", True, probes=[("owner_process", "not_in_use")]),
                item("o2", "C:\\Users\\Example\\AppData\\Local\\FixtureTool\\packages", "developer", "review_only",
                     "Needs network to rebuild", "network_redownload", False),
                item("o3", "C:\\Users\\Example\\AppData\\Local\\FixtureBrowser2\\Code Cache", "browsers", "default_selected",
                     "Verified local cache", "local_rebuild", False, skip="in_use", probes=[("owner_process", "in_use")]),
                item("o4", "C:\\Users\\Example\\.ssh", "user_essentials", "blocked", "Credentials are protected",
                     "not_rebuildable", False, skip="protected", size=None)],
            summary={"selected_count": 1, "selected_known_bytes": "1048576", "selected_unknown_size_count": 0,
                     "review_only_count": 1, "blocked_count": 1, "skipped_count": 2}),
        "cleanup-catalog": rec("cleanup_catalog", platform="windows", catalog_version="catalog/1", rule_set_digest="0" * 64, categories=[
            {"category": "browsers", "label": "Browsers", "wave": 1, "status": "available", "rules": [
                {"rule_id": "example.browser.code-cache", "version": "0.1.0", "status": "enabled", "max_tier": "default_selected",
                 "effective_tier_ceiling": "review_only", "rebuild_class": "local_rebuild", "action_kind": "filesystem", "adapter_ref": None,
                 "owner_id": "example.browser", "installed_version": "2.0.1", "registry_status": "version_untested",
                 "tested_range": "1.0..2.0", "tested_at": "2026-10-15", "evidence_refs": ["synthetic:schema-example"],
                 "summary": "Synthetic browser code cache"}]},
            {"category": "trash", "label": "Recycle Bin / Trash", "wave": 1, "status": "unsupported", "rules": []}],
            adapters=[{"adapter_id": "example.adapter.cache-clean-unreferenced", "version": "1.0.0", "tool_id": "example.tool",
                       "status": "unavailable", "reason": "Synthetic tool not found in a trusted location", "executable": None,
                       "executable_sha256": None, "script": None, "script_sha256": None, "argv": ["cache", "path"], "exit_code": None,
                       "duration_ms": 0, "output_summary": "", "reported_path": None, "cleanup_argv": ["cache", "clean"],
                       "cleanup_effect": "Synthetic: the tool removes unreferenced entries"}]),
        "needs-approval-uninstall": rec("error", code="needs_approval", message="Uninstall plan needs user review", retryable=False,
            required_action="Review the plan in the desktop app or with plan review", details={"plan_id": "p_v2_uninstall",
            "operation_id": None, "scope_id": None}),
        "unsupported-capability": rec("error", code="unsupported", message="Fan control is not available on this platform",
            retryable=False, required_action=None, details={"plan_id": None, "operation_id": None, "scope_id": None}),
        # D15: a gated capability is off until its gate passes or experimental features are turned on.
        "experimental-off": rec("error", code="unsupported", message="maintenance.run is off until release gate G3 passes; experimental features are "
            "disabled. To evaluate it on this machine, turn them on with `cleanmanager experimental --state on` or 设置 › 实验功能 in the desktop app",
            retryable=False, required_action="cleanmanager experimental --state on", details={"plan_id": None, "operation_id": None, "scope_id": None}),
        "index-unavailable": rec("error", code="index_unavailable", message="index database is damaged: file is not a database (the SQLite header is damaged); history and recovery are not affected: "
            "repair the index with `cleanmanager doctor --repair-index` (the damaged file is kept) and scan again",
            retryable=False, required_action="cleanmanager doctor --repair-index", details={"plan_id": None, "operation_id": None, "scope_id": None}),
        "protection-unavailable": rec("error", code="protection_unavailable", message="Protection list could not be parsed",
            retryable=False, required_action="Repair or reset the protection list", details={"plan_id": None, "operation_id": None, "scope_id": None}),
        "app-list": rec("app_list", next_cursor=None, page_size=50, items=[
            {"app_instance_id": "app_demo", "name": "Example App", "version": "1.2.3", "source": "registry_uninstall",
             "location": wpath("C:\\Program Files\\Example App"), "publisher": "Example Vendor",
             "identity_evidence": ["uninstall entry", "authenticode publisher"],
             "uninstall_capability": {"state": "available", "reason": None}, "update_status": "not_checked", "protected_class": False},
            {"app_instance_id": "app_security", "name": "Example Endpoint Agent", "version": None, "source": "msi",
             "location": None, "publisher": "Example Security", "identity_evidence": ["msi product code"],
             "uninstall_capability": {"state": "unsupported", "reason": "Endpoint security agents use their vendor process"},
             "update_status": "cannot_check", "protected_class": True}]),
        "startup-list": rec("startup_list", items=[
            {"startup_item_id": "st_demo", "name": "Example Updater", "source": "run_key",
             "definition": "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run : Example Updater",
             "target": wpath("C:\\Program Files\\Example App\\updater.exe"), "publisher": "Example Vendor",
             "publisher_evidence": "authenticode", "scope": "user", "state": "enabled", "toggle_capability": "available",
             "protected": False, "reason": None},
            {"startup_item_id": "st_broken", "name": "Removed Tool", "source": "startup_folder",
             "definition": "C:\\Users\\Example\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\Removed Tool.lnk",
             "target": wpath("C:\\Program Files\\Removed Tool\\tool.exe"), "publisher": None, "publisher_evidence": "none",
             "scope": "user", "state": "broken", "toggle_capability": "available", "protected": False,
             "reason": "Target is missing on a mounted volume"}],
            issues=[{"code": "permission_denied", "scope_display": "Task Scheduler \\Example", "message": "2 task folders could not be read"}]),
        "trash-inspection": rec("trash_inspection", scan_id="s_demo", entry_id="1842", path=wpath("D:\\Projects\\old-renders"),
            parent_display="D:\\Projects", object_kind="directory", cloud="not_cloud", logical_bytes="1048576", allocated_bytes=None,
            files=40, directories=2, protected=[{"path": wpath("D:\\Projects\\old-renders\\.git"), "reason": "default protection class source_worktree"}],
            protected_count=1, coverage_gaps=[], gap_count=0, trashable=False,
            refusal={"code": "policy_denied", "reason": "protected_descendants",
                     "message": "1 protected object(s) inside; select unprotected descendants instead"},
            holders=None, available_actions=["show_path", "copy_path"],
            guidance="Show or copy the path; reveal it in Explorer from the desktop app. Nothing was opened or downloaded."),
        "trash-list": rec("trash_list", items=[
            {"trash_item_id": "tb_00c0ffee00c0ffee", "bin_location": "D:\\$Recycle.Bin\\S-1-5-21-example\\$RAB12CD.mov",
             "original_path": "D:\\Videos\\example.mov", "deleted_at": "2026-10-14T09:00:00Z", "object_kind": "file",
             "logical_bytes": "1048576", "purge_capability": "available", "reason": None},
            {"trash_item_id": "tb_0badc0de0badc0de", "bin_location": "D:\\$Recycle.Bin\\S-1-5-21-example\\$RCD34EF",
             "original_path": "D:\\Projects\\linked", "deleted_at": "2026-10-13T09:00:00Z", "object_kind": "directory",
             "logical_bytes": None, "purge_capability": "unsupported",
             "reason": "contains a link or special object; empty it with Explorer (Recycle Bin, Delete) instead"}],
            issues=[]),
        "maintenance-catalog": rec("maintenance_catalog", platform="windows", query=None, elevation="one_prompt_per_run", items=[
            {"task_id": "windows.dns-flush", "task_version": "1.0.0", "platform": "windows", "title": "Flush DNS resolver cache",
             "problem": "Stale name resolution", "impact": "First lookups are slower briefly", "requires_admin": False,
             "applies_when": "The Windows DNS client interface is present", "estimated_seconds": 1, "applicability": "applicable",
             "applicability_reason": None, "user_protected": False, "reversible": False},
            {"task_id": "windows.trim-when-due", "task_version": "1.0.0", "platform": "windows", "title": "Retrim solid-state drives when due",
             "problem": "SSD retrim overdue", "impact": "Short disk activity", "requires_admin": True,
             "applies_when": "A fixed SSD volume was last optimized more than 30 days ago", "estimated_seconds": 60, "applicability": "not_needed",
             "applicability_reason": "C: last optimized 2026-10-10T03:00:00Z", "user_protected": False, "reversible": False}],
            prohibited=[{"id": "registry-optimizer", "title": "Registry cleaning or optimization",
                         "reason": "No measurable benefit and can break applications; there is no task for it"}]),
        "maintenance-report": rec("maintenance_report", **maintenance_run),
        "maintenance-history": rec("maintenance_history", items=[maintenance_run], undo="unavailable",
            note="Maintenance tasks are not undone; each run lists its tasks, outcomes and reasons."),
        "maintenance-diagnosis": rec("maintenance_diagnosis", sampled_at=t1, window_seconds=2, read_only=True, findings=[
            {"code": "runaway_process", "severity": "warning", "subject": "example.exe", "process_id": 4242, "duration_seconds": 3600,
             "detail": "99% of a CPU now and 97% on average for 60 min (C:\\Apps\\example.exe); it is not ended automatically",
             "next_step": "status.processes"},
            {"code": "low_disk_space", "severity": "warning", "subject": "C:", "process_id": None, "duration_seconds": None,
             "detail": "5.0 GiB free (1%)", "next_step": "analyze"}],
            top_processes=[{"process_id": 4242, "name": "example.exe", "image": "C:\\Apps\\example.exe", "cpu_core_percent": 99.0,
                            "working_set_bytes": "2147483648"}],
            memory_load_percent=62.0, commit_used_percent=48.5,
            note="Read-only: no process is ended and no setting is changed. Open the status view or Task Manager to act."),
        "status-snapshot": rec("status_snapshot", sampled_at=t1, stale=False, metrics=[
            {"name": "cpu.total_percent", "availability": "available", "value": 37.5, "unit": "percent", "reason": None},
            {"name": "thermal.cpu_celsius", "availability": "unavailable", "value": None, "unit": "celsius",
             "reason": "No public sensor interface on this synthetic host"}],
            health={"score": 82, "weights": {"cpu": 30, "memory": 25, "disk": 20, "thermal": 15, "io": 10}, "lowered_by": ["disk"],
                    "dimensions": [{"dimension": "cpu", "metric": "cpu.total_percent", "value": 37.5, "full_score_at": 60, "zero_score_at": 95,
                                    "score": 100},
                                   {"dimension": "thermal", "metric": "thermal.max_zone_celsius", "value": None, "full_score_at": 70,
                                    "zero_score_at": 95, "score": None}]},
            diagnosis={"code": "low_free_space", "message": "System volume free space is low", "link": "analyze"},
            network={"classification": "interface_type", "received_bytes_per_sec": 120000.0, "sent_bytes_per_sec": 30000.0,
                     "may_double_count": False,
                     "counted": [{"name": "Wi-Fi", "class": "physical", "received_bytes_per_sec": 120000.0, "sent_bytes_per_sec": 30000.0}],
                     "separate": [{"name": "Example VPN", "class": "tunnel", "received_bytes_per_sec": 100000.0, "sent_bytes_per_sec": 25000.0},
                                  {"name": "vEthernet (WSL)", "class": "virtual", "received_bytes_per_sec": 0.0, "sent_bytes_per_sec": 0.0}],
                     "loopback_excluded": 1,
                     "note": "Physical interfaces only; loopback is excluded and tunnel or virtual adapters are listed separately, not added"}),
        "protection-list": rec("protection_list", items=[
            {"protection_id": "hd_windows", "layer": "hard_deny", "match": "descendants", "display": "C:\\Windows (system_root)",
             "removable": False, "note": None, "added_at": None},
            {"protection_id": "dc_ai_sessions_config", "layer": "default_category", "match": "descendants",
             "display": "AI tool sessions and configuration (.claude, .codex); sessions are reviewable one by one", "removable": False,
             "note": None, "added_at": None},
            {"protection_id": "u_1", "layer": "user", "match": "descendants", "display": "C:\\Users\\Example\\Projects\\keep-cache",
             "removable": True, "note": "Build cache I reuse offline", "added_at": t1}]),
        "protection-check": rec("protection_check", path=wpath("C:\\Users\\Example\\Projects\\keep-cache\\index"), decision="denied",
            layer="user", reason="protected by user entry u_1", layers=[
                {"layer": "hard_deny", "result": "clear", "detail": "not a hard-deny location", "protection_id": None},
                {"layer": "default_category", "result": "clear", "detail": "no default protection class", "protection_id": None},
                {"layer": "user", "result": "denied", "detail": "protected by user entry u_1", "protection_id": "u_1"}]),
        "insight-list": rec("insight_list", items=[
            {"insight_id": "i_hiberfil", "kind": "windows.hibernation_file", "title": "Hibernation file", "path": wpath("C:\\hiberfil.sys"),
             "logical_bytes": "6442450944", "count": None, "size_basis": "file_metadata", "size_note": None, "observed_at": t1,
             "explanation": "Stores memory contents for hibernate and fast startup", "handling": "read_only_guidance",
             "guidance": [{"tool": "powercfg", "steps": "powercfg /hibernate off", "requires_admin": True,
                           "impact": "Disables hibernate and Fast Startup; Windows removes the file itself"}]},
            {"insight_id": "i_old_downloads", "kind": "old_downloads", "title": "Old downloads", "path": None, "logical_bytes": None, "count": 12,
             "size_basis": "not_measured", "size_note": "Sizes come from the latest disk scan", "observed_at": t1,
             "explanation": "Files older than 90 days in Downloads", "handling": "manual_trash_review", "guidance": []}]),
        "doctor-report": rec("doctor_report", generated_at=t1, redacted=True, engine_version="0.0.0-design-example", platform="windows",
            capabilities=[{"name": "uninstall.registry", "state": "unverified", "reason": "Synthetic report"}],
            checks=[{"area": "config", "check": "config.protection_list", "status": "ok", "detail": "3 entries"},
                    {"area": "store", "check": "store.open", "status": "unknown", "detail": "Synthetic report at ~\\AppData\\Local\\CleanManager\\store"}],
            error_codes=[{"code": "needs_approval", "exit_code": 3, "retryable": False}],
            recent_errors=[{"code": "precondition_changed", "count": 2, "last_seen": t1}],
            export={"path": "~\\Desktop\\cleanmanager-doctor.json", "written": False}),
        "doctor-report-index-repair": rec("doctor_report", generated_at=t1, redacted=True, engine_version="0.0.0-design-example", platform="windows",
            capabilities=[{"name": "uninstall.registry", "state": "unverified", "reason": "Synthetic report"}],
            checks=[{"area": "store", "check": "store.open", "status": "ok", "detail": "Synthetic audit store check"},
                    {"area": "store", "check": "store.index", "status": "ok", "detail": "Synthetic index check after the repair"}],
            error_codes=[{"code": "index_unavailable", "exit_code": 8, "retryable": False}], recent_errors=[], export=None,
            index_repair={"status": "repaired", "previous_state": "corrupt",
                "kept_files": ["~\\AppData\\Local\\CleanManager\\store\\index.db.damaged-20261022T101500Z"],
                "message": "Synthetic: the scan index was moved aside and an empty one was created; run scan again"}),
    }
    cli.update({
        "purge-roots": rec("purge_roots", roots=[{"path": wpath("C:\\Users\\Example\\Projects"), "source": "default", "exists": True},
            {"path": wpath("D:\\Code"), "source": "user", "exists": False}]),
        "plan-v2-update": plan_v2("p_v2_update", "update", [
            {"action_id": "a01", "action_type": "app_quit", "app_instance_id": "app_demo", "app_identity": "Example.App", "force": "ask_user"},
            {"action_id": "a02", **adapter, "adapter_id": "example.adapter.winget-upgrade", "argv": ["upgrade", "--id", "Example.App", "--exact"]}]),
        "release-info": rec("release_info", current_version="0.0.0-design-example", latest_version=None, release_id=None,
            install_method="winget", status="use_package_manager", reason="Installed by a package manager", checked_at=t1,
            manager_command="winget upgrade --id Example.CleanManager --exact"),
        "self-update-result": rec("self_update_result", status="verification_failed", from_version="0.0.0-design-example",
            to_version="0.0.1-design-example", signature_verified=True, checksum_verified=False,
            reason="checksum of cleanmanager.exe does not match the signed manifest; nothing was replaced", install_method="standalone",
            manager_command=None, backup_path=None),
        "self-remove-preview": rec("self_remove_preview", status="preview", remove=[
                {"item_id": "i001", "component": "system_setting", "action": "remove_path_entry", "display": r"user PATH entry D:\Tools\CleanManager",
                 "outcome": None, "reason": None},
                {"item_id": "i002", "component": "caches", "action": "remove_file", "display": r"~\AppData\Local\CleanManager\store\index.db",
                 "outcome": None, "reason": None},
                {"item_id": "i003", "component": "cli", "action": "remove_file", "display": r"D:\Tools\CleanManager\cleanmanager.exe",
                 "outcome": None, "reason": None}],
            keep=[{"data": "configuration", "display": "Settings"}, {"data": "protection_list", "display": "Protection list"},
                  {"data": "history", "display": "Operation history"}, {"data": "quarantine", "display": "Quarantine items"}],
            quarantine_items=2, quarantine_logical_bytes="2097152", system_settings_to_restore=[r"user PATH entry D:\Tools\CleanManager"],
            warnings=[{"code": "quarantine_not_empty", "message": "the quarantine still holds 2 item(s), 2097152 bytes; restore, purge or keep them"}],
            notes=["No service, scheduled task, helper or background process is installed. The only startup entry CleanManager creates is the desktop tray's opt-in value HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\\CleanManager.Resident (not present); no other system setting is changed."],
            install_method="standalone", uninstall_command=None, plan_digest="0" * 64, requires_interactive_confirmation=True),
        "self-install-result": rec("self_install_result", status="preview", version="0.0.0-design-example", install_dir=r"D:\Tools\CleanManager",
            files=40, path_conflicts=[{"path": r"C:\Users\<user>\scoop\shims\cleanmanager.exe", "relation": "different"}],
            path_entry="refused_conflict", reason="1 other cleanmanager program(s) on PATH are left untouched", requires_interactive_confirmation=True),
        "completion-powershell": rec("completion_script", shell="powershell", action="print", script="# synthetic completion script",
            profile_path=None, written=False),
        "completion-install-preview": rec("completion_script", shell="powershell", action="preview",
            script="# >>> cleanmanager completion >>>\n& 'C:\\Tools\\cleanmanager.exe' completion print powershell --format text | Out-String | Invoke-Expression\n# <<< cleanmanager completion <<<\n",
            profile_path=wpath("C:\\Users\\Example\\Documents\\PowerShell\\Microsoft.PowerShell_profile.ps1"), written=False),
    })
    def entry_item(eid, display, kind, logical, basis="entity", parent="e_root", boundary=None, quality="strong"):
        return {"entry_id": eid, "parent_entry_id": parent, "object_id": "o_" + eid, "path": wpath(display), "kind": kind,
                "logical_bytes": logical, "allocated_bytes": None, "size_basis": basis, "unknown_descendants": None,
                "link_count": 1 if kind == "file" else None, "boundary": boundary, "identity_quality": quality}
    cand_item = {k: v for k, v in candidate.items() if k not in ("schema_version", "kind", "request_id")}
    cli.update({
        "scan-status-running": rec("scan_status", scan_id="s_demo", scope_id="scope_demo", state="running",
            host={"kind": "cli", "process_id": 4242, "started_at": t0, "last_heartbeat_at": t1}, cancel_requested=False,
            entries_seen=12, skipped_ranges=1, known_logical_bytes="104857600",
            last_issue={"code": "permission_required", "scope_display": "synthetic protected subtree", "message": "Synthetic access denial"}),
        "scan-cancel-receipt": rec("scan_cancel_receipt", scan_id="s_demo", accepted=True, state_at_receipt="running",
            requested_at=t1, note="Receipt only: the scan stops at the next safe boundary; query scan show for the terminal record."),
        "scan-interrupted": {**scan, "status": "interrupted", "issues": scan["issues"] + [{"code": "host_exited",
            "scope_display": "scan host", "message": "Hosting process exited before a terminal record"}]},
        "scan-failed": {**scan, "status": "failed", "entries_seen": 0, "skipped_ranges": 0, "logical_bytes": None,
            "allocated_bytes": None, "unknown_size_count": 0, "issues": [{"code": "root_is_link",
            "scope_display": "C:\\Users\\Example\\Link", "message": "the root is a junction or volume mount point; links are never followed and nothing behind it was read. Scan the folder it points to directly"}]},
        "entries-page": rec("entries_page", scan_id="s_demo", scope_id="scope_demo", revision=1, sort="logical_desc",
            filter={"kind": "any", "name_contains": None, "parent_entry_id": "e_root"}, page_size=3, next_cursor="cur_opaque_1",
            unknown_size_items=1, items=[
                entry_item("e_videos", "C:\\Users\\Example\\Videos", "directory", "73400320", "subtree_known"),
                entry_item("e_iso", "C:\\Users\\Example\\image.iso", "file", "31457280"),
                entry_item("e_denied", "C:\\Users\\Example\\Private", "directory", None, "unknown")]),
        "entries-page-cleanable": rec("entries_page", scan_id="s_demo", scope_id="scope_demo", revision=1, sort="logical_desc",
            filter={"kind": "directory", "name_contains": None, "parent_entry_id": "e_app"}, page_size=3, next_cursor=None,
            unknown_size_items=0, items=[
                {**entry_item("e_nm", "C:\\Users\\Example\\source\\app\\node_modules", "directory", "524288000", "subtree_known", "e_app"),
                 "cleanable_hint": {"basis": "build_artifact", "rule_id": "purge.node-modules", "detail": "package.json", "flow": "purge"}},
                {**entry_item("e_tool", "C:\\Users\\Example\\source\\app\\.tool-cache", "directory", "41943040", "subtree_known", "e_app"),
                 "cleanable_hint": {"basis": "cachedir_tag", "rule_id": None, "detail": "CACHEDIR.TAG", "flow": "manual_trash"}},
                entry_item("e_src", "C:\\Users\\Example\\source\\app\\src", "directory", "2097152", "subtree_known", "e_app")]),
        "candidates-page": rec("candidates_page", candidate_set_id="cs_demo", scan_id="s_demo", revision=1, page_size=50,
            next_cursor=None, items=[cand_item]),
        "analysis-result": rec("analysis_result", candidate_set_id="cs_demo", scan_id="s_demo", scan_revision=1,
            rule_set_digest="1"*64, scan_completeness="partial_scope", candidate_count=1, blocked_count=0,
            pending_evidence_count=1, known_logical_bytes="104857600", unknown_size_count=0,
            issues=[{"code": "partial_evidence", "scope_display": "synthetic protected subtree",
                     "message": "A rule needing complete descendants was left pending"}]),
    })
    q_ident = {"volume_id": "synthetic-volume", "file_id": fid("quarantined"), "generation": None,
               "quality": "strong", "observed_at": t1}
    recovery_body = {"engine_contract": "recovery/v1", "plan_id": "rp_demo", "recovery_kind": "restore",
        "created_at": "2026-10-15T12:00:00Z", "expires_at": "2026-10-15T12:15:00Z", "items": [{
            "action_id": "a01", "quarantine_id": "q_demo", "quarantined_path": wpath("C:\\.cm-quarantine\\q_demo"),
            "quarantined_identity": q_ident, "original_parent_identity": {**q_ident, "file_id": fid("parent")},
            "original_path": wpath("C:\\Users\\Example\\report.cache"),
            "target": {"kind": "original", "path": wpath("C:\\Users\\Example\\report.cache")},
            "on_conflict": "skip", "logical_bytes": "1048576"}]}
    report_unknown = {**examples["operation-trash"], "operation_id": "op_unknown", "status": "reconciliation_required",
        "succeeded": 1, "unknown": 1, "items": [
            {"action_id": "a_demo", "status": "succeeded", "code": "moved_to_trash", "message": "Synthetic move", "recovery": "trash"},
            {"action_id": "a_demo2", "status": "unknown", "code": "outcome_unconfirmed",
             "message": "Process stopped after the OS call and before the result was recorded", "recovery": "unknown"}],
        "volume_free_after_bytes": None, "observed_free_delta_bytes": None}
    cli.update({
        "recovery-plan-restore": rec("plan", body=recovery_body, plan_digest=hashlib.sha256(canonical(recovery_body)).hexdigest(),
            authorization_status="required"),
        "authorization-policy": rec("authorization_status", plan_id="p_demo", plan_digest=digest, status="authorized",
            source="policy", policy_id="pol_demo", policy_version="1", principal="user:example", authorized_at=t1,
            expires_at="2026-10-14T12:16:00Z", quota={"window": "rolling_24h", "used_bytes": "576716800",
            "limit_bytes": "2147483648", "reserved_unknown_bytes": "0", "used_items": 12, "limit_items": 2000}),
        "authorization-required": rec("authorization_status", plan_id="p_demo", plan_digest=digest, status="required",
            source=None, policy_id=None, policy_version=None, principal=None, authorized_at=None, expires_at=None, quota=None),
        "operation-status-running": rec("operation_status", operation_id="op_demo", plan_id="p_demo", plan_digest=digest,
            state="running", started_at="2026-10-14T12:02:00Z", cancel_requested=False, succeeded=1, skipped=0, failed=0,
            unknown=0, remaining=2, host={"kind": "cli", "process_id": 4242, "last_heartbeat_at": "2026-10-14T12:02:01Z"}),
        "operation-cancel-receipt": rec("operation_cancel_receipt", operation_id="op_demo", accepted=True,
            state_at_receipt="running", requested_at="2026-10-14T12:02:01Z",
            note="Receipt only: no new actions start; completed actions are not undone."),
        "operation-reconciliation": report_unknown,
        "quarantine-list": rec("quarantine_list", page_size=50, next_cursor=None, total_logical_bytes="2097152", items=[
            {"quarantine_id": "q_demo", "plan_id": "p_demo", "operation_id": "op_demo",
             "original_path": wpath("C:\\Users\\Example\\report.cache"), "quarantined_path": wpath("C:\\.cm-quarantine\\q_demo"),
             "logical_bytes": "1048576", "quarantined_at": t1, "retention_reminder_at": "2026-11-13T12:00:02Z", "reminder_due": True,
             "state": "recoverable", "restore_capability": "available", "available_actions": ["restore", "purge"],
             "metadata_restored": ["timestamps", "attributes", "acl", "alternate_streams"]},
            {"quarantine_id": "q_conflict", "plan_id": "p_demo", "operation_id": "op_demo",
             "original_path": wpath("C:\\Users\\Example\\notes.cache"), "quarantined_path": wpath("C:\\.cm-quarantine\\q_conflict"),
             "logical_bytes": "1048576", "quarantined_at": t1, "retention_reminder_at": "2026-11-13T12:00:02Z", "reminder_due": False,
             "state": "target_conflict", "restore_capability": "available", "available_actions": ["restore", "purge"],
             "metadata_restored": ["timestamps", "attributes", "acl", "alternate_streams"]}]),
        "operation-list": rec("operation_list", page_size=3, next_cursor="WyJvcGVyYXRpb25zIiwyLDNd", items=[
            {"operation_id": "op_restore", "plan_id": "rp_demo", "plan_contract": "recovery/v1", "action": "restore", "state": "partial",
             "started_at": "2026-10-15T12:01:00Z", "finished_at": "2026-10-15T12:01:01Z", "authorization_source": "interactive_review",
             "rule_refs": [], "counts": {"succeeded": 1, "skipped": 1, "failed": 0, "not_started": 0, "unknown": 0},
             "processed_logical_bytes": "1048576", "recovery": {"availability": "not_applicable",
                 "guidance": "Objects were moved back to their original locations."}},
            {"operation_id": "op_demo", "plan_id": "p_demo", "plan_contract": "plan/v1", "action": "trash", "state": "completed",
             "started_at": "2026-10-14T12:02:00Z", "finished_at": "2026-10-14T12:02:01Z", "authorization_source": "interactive_review",
             "rule_refs": ["example.cache@0.1.0"], "counts": {"succeeded": 1, "skipped": 0, "failed": 0, "not_started": 0, "unknown": 0},
             "processed_logical_bytes": "104857600", "recovery": {"availability": "system_recycle_bin",
                 "guidance": "Restore from the system Recycle Bin (Explorer, Recycle Bin, Restore); CleanManager cannot restore it."}},
            {"operation_id": "op_unknown", "plan_id": "p_demo2", "plan_contract": "plan/v1", "action": "delete",
             "state": "reconciliation_required", "started_at": "2026-10-14T12:00:00Z", "finished_at": None, "authorization_source": None,
             "rule_refs": ["example.cache@0.1.0"], "counts": None, "processed_logical_bytes": None, "recovery": {"availability": "unknown",
                 "guidance": "Outcome unconfirmed; run `cleanmanager operations reconcile op_unknown` before deciding anything."}}]),
    })
    # PUR/INC previews: purge artifacts by rebuildability, installers review-only with a source label.
    def purge_item(oid, display, rule, t, reason, rebuild, selected, skip=None, notes=(), source=None):
        it = {**item(oid, display, "project_build_output" if rebuild == "local_rebuild" else "project_dependencies", t, reason, rebuild, selected, skip),
              "rule_id": rule, "rule_version": "1.0.0", "registry_status": "covered", "notes": list(notes)}
        if source:
            it.update({"category": "installers", "source_label": source})
        return it
    cli["preview-purge"] = rec("selection_preview", command="purge.preview", scan_id="s_demo", scan_revision=1, dry_run=True,
        non_interactive=True, next_cursor=None, page_size=3, convertible_to_plan=True, items=[
            purge_item("c_t", "C:\\Users\\Example\\Projects\\app\\target", "purge.target", "default_selected",
                       "build output next to its project marker; rebuilt locally", "local_rebuild", True, notes=["project: ... (cargo.toml)"]),
            purge_item("c_n", "C:\\Users\\Example\\Projects\\web\\node_modules", "purge.node-modules", "review_only",
                       "dependencies: review only, reinstalling needs the network", "network_redownload", False, skip="recent"),
            purge_item("c_d", "C:\\Users\\Example\\Projects\\web\\dist", "purge.dist", "blocked", "vcs_tracked:3_files", "local_rebuild", False,
                       skip="vcs_tracked")],
        summary={"selected_count": 1, "selected_known_bytes": "1048576", "selected_unknown_size_count": 0,
                 "review_only_count": 1, "blocked_count": 1, "skipped_count": 2})
    cli["preview-installer"] = rec("selection_preview", command="installer.preview", scan_id="s_demo", scan_revision=1, dry_run=True,
        non_interactive=True, next_cursor=None, page_size=2, convertible_to_plan=True, items=[
            purge_item("c_e", "C:\\Users\\Example\\Downloads\\ExampleSetup.exe", "installer.exe", "review_only",
                       "installers are review-only: choose them one by one or by source", "network_redownload", False,
                       notes=["recognized: nsis: installer data header after the image", "signed by Example Publisher"], source="downloads"),
            purge_item("c_i", "C:\\Users\\Example\\Downloads\\example.iso", "installer.iso", "review_only", "mounted_image: eject it first",
                       "network_redownload", False, skip="mounted", source="downloads")],
        summary={"selected_count": 0, "selected_known_bytes": "0", "selected_unknown_size_count": 0,
                 "review_only_count": 2, "blocked_count": 0, "skipped_count": 1})
    cli["installer-locations"] = rec("installer_locations", max_depth=2, locations=[
        {"path": wpath("C:\\Users\\Example\\Downloads"), "source_label": "downloads", "exists": True},
        {"path": wpath("C:\\Users\\Example\\Desktop"), "source_label": "desktop", "exists": True}])
    # Parity 12.3/12.4, 7.3, 10.x fixtures (synthetic).
    unsigned = {"state": "unsigned", "publisher": None, "reason": "no valid embedded or catalog signature"}
    cli["process-list"] = rec("process_list", sampled_at=t1, window_ms=1000, sort="cpu", search=None, total=2, matched=2, unidentified=1,
        notes=["disk_bytes_per_sec is the process's read+write transfer rate"], items=[
            {"pid": 4242, "parent_pid": 1000, "parent_name": "explorer.exe", "name": "example-helper.exe",
             "image_path": wpath("C:\\Users\\Example\\AppData\\Local\\Example\\example-helper.exe"), "started_at": "134050000000000000",
             "started_at_utc": "2025-10-15T11:06:40Z", "cpu_percent": 12.5, "memory_bytes": "104857600", "disk_bytes_per_sec": 0.0, "network_bytes_per_sec": None, "signature": unsigned,
             "owner_app": None, "identified": False, "purpose": None, "unidentified_reason": "Not in CleanManager's local process knowledge",
             "ai_diagnosis": {"available": True, "hint": "Purpose not identified locally. You can ask the optional AI diagnosis about this process"},
             "pinned": False, "end_capability": {"state": "available", "protection": None, "reason": "Ends only through a reviewed plan for this one process"},
             "notes": []},
            {"pid": 700, "parent_pid": 600, "parent_name": "wininit.exe", "name": "lsass.exe", "image_path": wpath("C:\\Windows\\System32\\lsass.exe"),
             "started_at": "134049000000000000", "started_at_utc": "2025-10-14T07:20:00Z", "cpu_percent": 0.1, "memory_bytes": "8388608", "disk_bytes_per_sec": 0.0, "network_bytes_per_sec": None,
             "signature": {"state": "catalog_signed", "publisher": "Microsoft Windows", "reason": None}, "owner_app": None, "identified": True,
             "purpose": "Local Security Authority: sign-in, credentials and security policy", "unidentified_reason": None, "ai_diagnosis": None,
             "pinned": False, "end_capability": {"state": "unsupported", "protection": "critical_system",
                                                 "reason": "Critical Windows process: ending it stops or restarts the system"}, "notes": []}])
    cli["status-alert"] = rec("status_alert", pid=4242, name="example-helper.exe", image_path="C:\\Users\\Example\\AppData\\Local\\Example\\example-helper.exe",
        min_cpu_percent=91.0, threshold_percent=80.0, window_seconds=120, since=t0, raised_at=t1, link="status.processes/4242")
    cli["insight-overview"] = rec("insight_overview", generated_at=t1, old_downloads={"older_than_days": 90, "item_count": 3,
            "total_logical_bytes": "734003200", "observed_at": t1}, cards=[
        {"card_id": "i_card_downloads", "kind": "old_downloads", "title": "Downloads older than 90 days", "logical_bytes": "734003200", "count": 3,
         "size_note": None, "observed_at": t1, "link": {"view": "insights.downloads", "path": wpath("C:\\Users\\Example\\Downloads"), "insight_ids": []}},
        {"card_id": "i_card_snapshots", "kind": "snapshots", "title": "Restore points and snapshots", "logical_bytes": None, "count": 4,
         "size_note": "Size unavailable: Requires administrator", "observed_at": t0, "link": {"view": "insights", "path": None, "insight_ids": ["i_shadow"]}}])
    cli["old-downloads-review"] = rec("old_downloads_review", root=wpath("C:\\Users\\Example\\Downloads"), older_than_days=90, observed_at=t1,
        items=[{"path": wpath("C:\\Users\\Example\\Downloads\\video.mov"), "logical_bytes": "629145600", "modified_at": t0, "age_days": 210,
                "installer_like": False, "selected": False}],
        total_logical_bytes="734003200", item_count=3, shown=1, note=None, handling="manual_trash_review",
        guidance="Review only: nothing is selected. Pick files yourself and move them to the Recycle Bin with the trash command.")
    cli["app-update-list"] = rec("app_update_list", checks_enabled=True, checked=True, checked_at=t1,
        counts={"update_available": 1, "up_to_date": 0, "cannot_check": 1, "not_checked": 0}, items=[
            {"app_instance_id": "app_editor", "name": "Example Editor", "current_version": "1.2.0", "available_version": "1.3.0", "channel": "winget",
             "channel_id": "Example.Editor", "state": "update_available", "reason": None, "checked_at": t1, "route": "package_manager", "official_url": None},
            {"app_instance_id": "app_tool", "name": "Unpackaged Tool", "current_version": "0.9", "available_version": None, "channel": None,
             "channel_id": None, "state": "cannot_check", "reason": "No update channel: not installed through winget, Microsoft Store or Scoop",
             "checked_at": t1, "route": "none", "official_url": None}])
    cli["experimental-features"] = rec("experimental_features", experimental_features=False, capabilities=[
            {"capability": "clean.catalog", "gate": "G1", "gate_passed": False, "enabled": False},
            {"capability": "maintenance.run", "gate": "G3", "gate_passed": False, "enabled": False}],
        note="Capabilities whose release gate has not passed are off unless experimental features are on. They are for evaluation on this machine only; turning this on passes no gate and changes no plan review.")
    cli["app-update-settings"] = rec("app_update_settings", checks_enabled=False, periodic_checks=False, last_checked_at=t1,
        note="Checks run only when you ask (apps updates); off means no update request of any kind is made.")
    upd_target = {"app_instance_id": "app_editor", "name": "Example Editor", "channel": "winget", "package_id": "Example.Editor",
                  "from_version": "1.2.0", "to_version": "1.3.0", "installed_signer": "Example Software, Inc.", "install_location": "C:\\Program Files\\Example",
                  "official_url": None}
    upd_body = {"engine_contract": "plan/v2", "plan_id": "up_demo", "plan_kind": "update", "policy_matchable": False, "requires_admin": False,
                "scope_id": None, "scan_id": None, "scan_revision": None, "rule_set_digest": "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945",
                "policy_digest": "2" * 64, "created_at": t1, "expires_at": "2026-10-14T12:15:02Z", "items": [
                    {"action_id": "a01", "action_type": "tool_adapter", "adapter_id": "winget.upgrade", "adapter_version": "1",
                     "executable": {"path": wpath("C:\\Program Files\\WindowsApps\\Microsoft.DesktopAppInstaller_8wekyb3d8bbwe\\winget.exe"), "sha256": "e" * 64},
                     "argv": ["upgrade", "--id", "Example.Editor", "--exact", "--version", "1.3.0", "--source", "winget", "--silent",
                              "--disable-interactivity", "--accept-source-agreements", "--accept-package-agreements"],
                     "preview_digest": hashlib.sha256(canonical(upd_target)).hexdigest(), "timeout_seconds": 900}]}
    cli["app-update-plan"] = rec("app_update_plan", plan={"body": upd_body, "plan_digest": hashlib.sha256(canonical(upd_body)).hexdigest(),
            "authorization_status": "required"},
        routed=[{"app_instance_id": "app_photos", "name": "Example Store App", "route": "open_store", "reason": "Microsoft Store updates its own apps",
                 "url": "ms-windows-store://downloadsandupdates"}],
        next="cleanmanager plan review up_demo (in a terminal), then cleanmanager plan apply up_demo")
    cli["process-pins"] = rec("process_pins", pins=["code.exe"])
    cli["status-alert-settings"] = rec("status_alert_settings", enabled=False, cpu_threshold_percent=80.0, window_seconds=120, cooldown_seconds=1800,
        note="Alerts are raised only while a status stream (status --watch) or the resident component runs; off means no process sampling for alerts.")
    # Parity 9.x (UNI-01..10): uninstall routes and the grouped leftover review of a finished uninstall operation.
    cli["app-uninstall-list"] = rec("app_uninstall_list", items=[
        {"app_instance_id": "app_demo", "name": "Example App", "version": "2.1", "publisher": "Example Vendor Ltd.", "source": "registry_uninstall",
         "last_used_at": None, "location": "C:\\Program Files\\Example App", "protected_class": False, "uninstall": {"state": "available",
             "adapter": {"adapter_id": "windows.registered-uninstaller", "display": "\"C:\\Program Files\\Example App\\uninst.exe\" (vendor uninstaller, its own UI)",
                 "package_manager": False}, "reason": None, "settings_uri": None}},
        {"app_instance_id": "app_tool", "name": "Example Tool", "version": None, "publisher": None, "source": "registry_uninstall", "location": None,
         "last_used_at": None,
         "protected_class": False, "uninstall": {"state": "unverified", "adapter": None,
             "reason": "no uninstall command is registered for this application; CleanManager never deletes the install folder as an uninstall",
             "settings_uri": "ms-settings:appsfeatures"}},
        {"app_instance_id": "app_agent", "name": "Example Endpoint Agent", "version": "7.0", "publisher": "Example Security", "source": "msi",
         "location": None, "last_used_at": None, "protected_class": True, "uninstall": {"state": "unsupported", "adapter": None,
             "reason": "Security or device-management agent: CleanManager offers no uninstall; remove it through your administrator or the vendor's official uninstall process",
             "settings_uri": None}}], issues=[])
    def leftover(oid, kind, group, display, tier, selected, action="fs_trash", marker=None, warnings=(), note=None, size=None, guidance=None):
        return {"object_id": oid, "kind": kind, "group": group, "display": display, "evidence": ["%APPDATA%\\ExampleApp matches the product name"],
                "marker": marker, "tier": tier, "selected": selected, "warnings": list(warnings), "logical_bytes": size, "note": note,
                "requires_admin": False, "action": action, "guidance": guidance}
    cli["uninstall-leftovers"] = rec("uninstall_leftovers", operation_id="op_demo",
        app={"app_instance_id": "app_demo", "name": "Example App", "publisher": "Example Vendor Ltd.", "source": "registry_uninstall"},
        groups=[{"group": "caches_logs", "items": [leftover("lo_cache", "directory", "caches_logs", "C:\\Users\\Example\\AppData\\Roaming\\ExampleApp\\Cache",
                    "default_selected", True, size="1048576")]},
                {"group": "personal_data", "items": [leftover("lo_models", "directory", "personal_data", "C:\\Users\\Example\\AppData\\Roaming\\ExampleApp\\models",
                    "review_only", False, note="models: large, usually re-downloadable by the app, but only if you still have access to them", size="4294967296"),
                    leftover("lo_shared", "directory", "personal_data", "C:\\Users\\Example\\AppData\\Local\\ExampleApp", "kept", False,
                    marker="shared_owner")]},
                {"group": "settings", "items": [leftover("lo_key", "registry_key", "settings", "HKCU\\SOFTWARE\\ExampleVendor\\ExampleApp", "review_only", False,
                    action="registry_backup_delete", note="backed up before it is deleted; restorable from history")]},
                {"group": "system", "items": [leftover("lo_svc", "service", "system", "service ExampleAppUpdater", "review_only", False, action="manual",
                    guidance="CleanManager does not delete services: stop it and run `sc.exe delete ExampleAppUpdater` from an administrator terminal")]}],
        default_selected=1, external_references=["C:\\Users\\Example\\.gitconfig mentions C:\\Program Files\\Example App (read-only hint; CleanManager does not edit it)"])
    # UNI-11: a batch request, its per-application results (one uninstaller failed, the other completed) and the
    # leftover review of that operation: only the application whose uninstaller succeeded has objects to review.
    cli["request-apps-uninstall-batch"] = rec("cli_request", command="apps.uninstall.plan", output="json", non_interactive=True, dry_run=False,
        params={"app_instance_ids": ["app_demo", "app_tool"], "end_running": False})
    def result(action_id, status, code, message):
        return {"action_id": action_id, "status": status, "code": code, "message": message, "recovery": "not_applicable"}
    cli["operation-uninstall-batch"] = rec("operation_report", operation_id="op_batch", plan_id="up_batch", plan_digest="6" * 64, status="partial",
        started_at="2026-10-15T12:00:00Z", finished_at="2026-10-15T12:01:10Z", succeeded=3, skipped=0, failed=1, not_started=0, unknown=0,
        items=[result("q0000000", "succeeded", "not_running", "Example App is not running"),
               result("u0000001", "failed", "uninstaller_failed", "uninstaller exited with code 1603 after 5120 ms; rescan: the install folder and the uninstall registration still present"),
               result("u0010000", "succeeded", "not_running", "Example Tool is not running"),
               result("u0010001", "succeeded", "done", "uninstaller exited with code 0 after 8410 ms; rescan: install folder and registration are gone")],
        processed_logical_bytes="0", quarantined_logical_bytes="0", estimated_freed_bytes="0", volume_free_before_bytes=None,
        volume_free_after_bytes=None, observed_free_delta_bytes=None, authorization_source="interactive_review", uninstall_apps=[
            {"app_instance_id": "app_demo", "name": "Example App", "status": "failed", "code": "uninstaller_failed",
             "message": "uninstaller exited with code 1603 after 5120 ms; rescan: the install folder and the uninstall registration still present",
             "action_ids": ["q0000000", "u0000001"], "leftovers_reviewable": False},
            {"app_instance_id": "app_tool", "name": "Example Tool", "status": "completed", "code": "done",
             "message": "uninstaller exited with code 0 after 8410 ms; rescan: install folder and registration are gone",
             "action_ids": ["u0010000", "u0010001"], "leftovers_reviewable": True}])
    tool_cache = leftover("lo_toolcache", "directory", "caches_logs", "C:\\Users\\Example\\AppData\\Local\\Example Tool\\Cache", "default_selected", True, size="2097152")
    demo_app = {"app_instance_id": "app_demo", "name": "Example App", "publisher": "Example Vendor Ltd.", "source": "registry_uninstall"}
    tool_app = {"app_instance_id": "app_tool", "name": "Example Tool", "publisher": None, "source": "registry_uninstall"}
    cli["uninstall-leftovers-batch"] = rec("uninstall_leftovers", operation_id="op_batch", app=None,
        groups=[{"group": "caches_logs", "items": [tool_cache]}], default_selected=1, external_references=[], apps=[
            {"app": demo_app, "status": "failed", "code": "uninstaller_failed", "message": "uninstaller exited with code 1603 after 5120 ms",
             "leftovers_reviewable": False, "groups": [], "default_selected": 0, "external_references": []},
            {"app": tool_app, "status": "completed", "code": "done", "message": "uninstaller exited with code 0 after 8410 ms",
             "leftovers_reviewable": True, "groups": [{"group": "caches_logs", "items": [tool_cache]}], "default_selected": 1, "external_references": []}])
    # WFA-42/43: which policy authorized a plan or would, and why the others do not; WFA-40: a reconciled operation
    # keeps its authorization and records the reconciliation apart.
    mismatch = {"policy_id": "pol_other", "policy_version": "1", "state": "enabled", "covers": False, "reason": "mode_differs",
        "message": "the plan's mode trash is not the policy's mode quarantine"}
    cli["plan-policy-matches"] = rec("plan", body=body, plan_digest=digest, authorization_status="required", authorized_by=None,
        policy_matches=[{"policy_id": "pol_demo", "policy_version": "2", "state": "enabled", "covers": True, "reason": None,
            "message": "would authorize this plan when it is applied (the last 24 hours used 0 of 2147483648 bytes and 0 of 2000 items; "
                       "this plan needs 104857600 bytes and 1 items)"}, mismatch])
    cli["plan-policy-authorized"] = rec("plan", body=body, plan_digest=digest, authorization_status="authorized",
        authorized_by={"source": "policy", "policy_id": "pol_demo", "policy_version": "2", "authorized_at": "2026-10-14T12:02:00Z"})
    cli["needs-approval-policy-mismatch"] = rec("error", code="needs_approval",
        message="plan p_demo needs approval: it has not been reviewed and no policy covers it (pol_other: " + mismatch["message"] + ")",
        retryable=False, required_action="Review the plan in the desktop app or run `cleanmanager plan review p_demo` in a terminal",
        details={"plan_id": "p_demo", "operation_id": None, "scope_id": None, "policy_matches": [mismatch]})
    cli["operation-reconciled"] = {**report_unknown, "status": "completed", "succeeded": 2, "unknown": 0, "items": [
            {"action_id": "a_demo", "status": "succeeded", "code": "moved_to_trash", "message": "Synthetic move", "recovery": "trash"},
            {"action_id": "a_demo2", "status": "succeeded", "code": "reconciled_performed", "message": "the action was found to have taken effect",
             "recovery": "trash"}],
        "processed_logical_bytes": "209715200", "volume_free_before_bytes": None, "finished_at": "2026-10-14T12:05:00Z",
        "reconciled_at": "2026-10-14T12:05:00Z"}
    for name, example in cli.items():
        save("examples/contracts/" + name + ".json", example)
    save("rules/examples/example-cache.json", {"schema_version": "1.0", "rule_id": "example.cache", "version": "0.1.0",
        "status": "experimental", "platforms": ["windows", "macos"], "root_resolver": "application_cache",
        "owner_app_id": "example.invalid.fixture-app", "relative_components": ["Cache"], "min_age_days": 14,
        "risk": "low", "allowed_modes": ["trash"], "requires_owner_inactive": True, "protect_descendants": True,
        "follow_links": False, "impact": "Synthetic rule only; no production cache path or cleanup support is claimed.", "evidence_refs": []})
    # CM-C14N-v1 golden vectors shared by Python and Rust (task 2.5).
    cases = [
        ("key-order-and-nesting", {"b": [3, {"z": None, "a": True}], "a": "x"}),
        ("escapes", {"s": "q\"\\/\n\r\t\b\f\u0001\u001f\u007f"}),
        ("unicode-kept-raw", {"name": "中文 文件\u2028😀", "e\u0301": "é"}),
        ("integers-at-bounds", {"min": -9223372036854775808, "max": 18446744073709551615, "zero": 0}),
        ("plan-like", {"items": [{"action_id": "a01", "logical_bytes": "18446744073709551615"}], "mode": "trash"}),
    ]
    golden = {"contract": "CM-C14N-v1", "cases": [], "rejected": [
        {"name": "duplicate-key", "input": '{"a":1,"a":2}', "reason": "duplicate_key"},
        {"name": "float", "input": '{"a":1.5}', "reason": "float"},
        {"name": "exponent", "input": '{"a":1e3}', "reason": "float"},
        {"name": "trailing-data", "input": '{"a":1} x', "reason": "malformed"},
        {"name": "non-ascii-key", "input": '{"名":1}', "reason": "non_ascii_key"}]}
    for name, value in cases:
        if name == "unicode-kept-raw":
            value = {"name": "中文 文件\u2028😀", "e_accent": "e\u0301"}
        text = json.dumps(value, ensure_ascii=False, indent=1)
        canon = canonical(value).decode("utf-8")
        golden["cases"].append({"name": name, "input": text, "canonical": canon,
                                "sha256": hashlib.sha256(canon.encode("utf-8")).hexdigest()})
    save("examples/c14n/golden.json", golden)
    root_ident = {"volume_id": "synthetic-volume", "file_id": fid("scope-root"), "generation": None, "quality": "strong", "observed_at": t1}
    observation = {"contract": "obs/v1", "scope_root": root_ident, "volume_id": "synthetic-volume",
        "entity": {**root_ident, "file_id": fid("entity"), "generation": "7"},
        "parent_chain": [{**root_ident, "file_id": fid("parent")}, root_ident],
        "path": wpath("C:\\Users\\Example\\AppData\\Local\\ExampleApp\\Cache\\数据.bin"), "kind": "file",
        "logical_bytes": "1048576", "allocated_bytes": None,
        "modified": {"unit": "windows_filetime_100ns", "value": "134050000000000000"}, "change_marker": None,
        "link_count": 1, "reparse_tag": 0, "cloud": "not_cloud",
        "field_sources": {"logical_bytes": "handle_query", "allocated_bytes": None, "modified": "handle_query",
            "change_marker": None, "link_count": "handle_query", "reparse_tag": "handle_query", "cloud": "directory_enumeration"}}
    save("examples/c14n/observation-v1.json", {"record": observation,
        "fingerprint_sha256": hashlib.sha256(canonical(observation)).hexdigest()})
    plans = sum(name.startswith("plan-v2-") for name in [*v2, *cli])
    print(f"Generated 2 schemas, {8 + len(v2) + len(cli)} protocol examples ({plans} plan/v2 golden vectors, "
          f"{len(cli) - (plans - len(v2))} CLI-10 fixtures) and 1 experimental rule example.")
    # Optional AI diagnosis extension: its own contract (schemas/diag/v1), never part of schemas/v1.
    import runpy
    print(f"Generated the diag/1 schema and {runpy.run_path(str(ROOT / 'tools/diag_contracts.py'))['generate'](ROOT)} diagnosis examples.")


if __name__ == "__main__":
    main()
