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
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main():
    defs = {}
    defs["path"] = obj({"platform": enum("windows", "macos"), "display": STR,
                        "raw_encoding": enum("windows_utf16le_base64", "unix_bytes_base64"),
                        "raw_base64": {"type": "string", "minLength": 4, "contentEncoding": "base64"}})
    defs["identity"] = obj({"volume_id": STR, "file_id": {"type": "string", "pattern": "^(ntfs64:[0-9a-f]{16}|fileid128:[0-9a-f]{32}|inode64:[0-9a-f]{16})$"}, "generation": nullable(STR),
                            "quality": enum("strong", "weak"), "observed_at": TIME})
    defs["issue"] = obj({"code": STR, "scope_display": STR, "message": STR})
    defs["capability"] = obj({"name": STR, "state": enum("available", "permission_required", "unsupported", "unverified"),
                              "reason": nullable(STR)})
    defs["capabilities"] = record("capabilities", {"platform": enum("windows", "macos"),
        "engine_version": STR, "supported_schema_versions": arr(STR, 1), "capabilities": arr(ref("capability"), 1)})
    defs["scan_result"] = record("scan_result", {"scan_id": STR, "scope_id": STR, "revision": COUNT,
        "backend": STR, "status": enum("completed", "partial", "cancelled", "failed", "interrupted"),
        "completeness": enum("verified_scope", "partial_scope"), "started_at": TIME, "finished_at": TIME,
        "entries_seen": COUNT, "skipped_ranges": COUNT, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT), "unknown_size_count": COUNT, "issues": arr(ref("issue"))})
    candidate_props = {"candidate_id": STR, "scan_id": STR, "scan_revision": COUNT,
        "entry_id": STR, "rule_id": STR, "rule_version": STR, "risk": enum("low", "medium", "high", "blocked"),
        "path": ref("path"), "reason": STR, "impact": STR, "logical_bytes": UINT,
        "allocated_bytes": nullable(UINT), "reclaimable_estimate_bytes": nullable(UINT),
        "estimate_basis": enum("metadata_estimate", "shared_blocks_unknown", "not_computed"),
        "allowed_modes": {**arr(enum("trash", "quarantine", "delete")), "uniqueItems": True}}
    defs["candidate"] = record("candidate", candidate_props)
    defs["candidate_item"] = obj(candidate_props)
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
    # REC-04 history: one row per operation with truthful recovery availability.
    defs["operation_summary"] = obj({"operation_id": STR, "plan_id": STR,
        "plan_contract": nullable(enum("plan/v1", "plan/v2", "recovery/v1")),
        "action": nullable(enum("trash", "quarantine", "delete", "restore", "purge")),
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
    }
    for name, props in action_props.items():
        defs["action_" + name] = obj({"action_id": STR, "action_type": {"const": name}, **props})
    defs["plan_action"] = {"oneOf": [ref("action_" + name) for name in action_props]}
    kind_actions = {
        "cleanup": ["fs_trash", "fs_quarantine", "fs_delete", "tool_adapter"],
        "manual_selection": ["fs_trash", "fs_delete"],
        "uninstall": ["app_quit", "tool_adapter", "fs_trash", "fs_quarantine", "registry_backup_delete", "startup_toggle"],
        "maintenance": ["maintenance_task", "settings_change"],
        "startup": ["startup_toggle"],
        "update": ["app_quit", "tool_adapter"],
        "recovery": ["registry_restore", "startup_toggle", "settings_change"],
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
        "volume_free_after_bytes": nullable(UINT), "observed_free_delta_bytes": nullable(SINT)})
    defs["error"] = record("error", {"code": enum("invalid_argument", "schema_mismatch", "needs_approval", "policy_denied",
        "permission_required", "plan_stale", "cursor_stale", "conflict", "unsupported", "io_error", "internal_error", "cancelled",
        "protection_unavailable"),
        "message": STR, "retryable": {"type": "boolean"}, "required_action": nullable(STR),
        "details": obj({"plan_id": nullable(STR), "operation_id": nullable(STR), "scope_id": nullable(STR)})})
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
        "insights.list": obj({"scan_id": nullable(STR)}),
        "apps.list": obj({"cursor": nullable(STR), "page_size": selection["page_size"]}),
        "apps.uninstall.plan": obj({"app_instance_id": STR, "include_groups": {**arr(enum("bundle", "caches_logs",
            "settings", "personal_data", "startup", "system"), 1), "uniqueItems": True}}),
        "apps.updates.check": obj({"app_instance_ids": arr(STR)}), "apps.updates.plan": obj({"app_instance_ids": arr(STR, 1)}),
        "startup.list": obj({}), "startup.plan": obj({"startup_item_id": STR,
            "operation": enum("enable", "disable", "remove_with_backup", "restore")}),
        "optimize.list": obj({}), "optimize.plan": obj({"task_ids": {**arr(STR, 1), "uniqueItems": True}}),
        "status.snapshot": obj({}), "status.watch": obj({"interval_seconds": {"type": "integer", "minimum": 1, "maximum": 3600},
            "max_samples": nullable({"type": "integer", "minimum": 1, "maximum": 100000})}),
        "protect.list": obj({}), "protect.add": obj({"entry": ref("protection_entry_input")}), "protect.remove": obj({"protection_id": STR}),
        "protect.check": obj({"path": ref("path"), "rule_id": nullable(STR)}),
        "doctor.report": obj({"redact": {"type": "boolean"}}),
        "completion.generate": obj({"shell": enum("bash", "zsh", "fish", "powershell"), "write_profile": {"type": "boolean"}}),
        "self.update.check": obj({}), "self.update.apply": obj({"release_id": STR}), "self.remove.preview": obj({"keep_user_data": {"type": "boolean"}}),
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
    defs["selection_preview"] = record("selection_preview", {"command": STR, "scan_id": nullable(STR), "scan_revision": nullable(COUNT),
        "dry_run": {"const": True}, "non_interactive": {"type": "boolean"}, "items": arr(ref("preview_item")), **page,
        "summary": obj({"selected_count": COUNT, "selected_known_bytes": UINT, "selected_unknown_size_count": COUNT,
            "review_only_count": COUNT, "blocked_count": COUNT, "skipped_count": COUNT}), "convertible_to_plan": {"type": "boolean"}})
    capability_state = enum("available", "permission_required", "unsupported", "unverified")
    defs["app_instance"] = obj({"app_instance_id": STR, "name": STR, "version": nullable(STR),
        "source": enum("app_bundle", "homebrew", "app_store", "registry_uninstall", "msi", "msix_store", "winget", "scoop", "unknown"),
        "location": nullable(ref("path")), "publisher": nullable(STR), "identity_evidence": arr(STR),
        "uninstall_capability": obj({"state": capability_state, "reason": nullable(STR)}),
        "update_status": enum("up_to_date", "update_available", "cannot_check", "not_checked"), "protected_class": {"type": "boolean"}})
    defs["app_list"] = record("app_list", {"items": arr(ref("app_instance")), **page})
    defs["startup_item"] = obj({"startup_item_id": STR, "name": STR, "source": action_props["startup_toggle"]["source"],
        "definition": STR, "target": nullable(ref("path")), "publisher": nullable(STR),
        "publisher_evidence": enum("authenticode", "package_identity", "none"), "scope": enum("user", "system"),
        "state": enum("enabled", "disabled", "broken", "target_unreachable"), "toggle_capability": capability_state,
        "protected": {"type": "boolean"}, "reason": nullable(STR)})
    defs["startup_list"] = record("startup_list", {"items": arr(ref("startup_item")), "issues": arr(ref("issue"))})
    defs["maintenance_task_info"] = obj({"task_id": STR, "task_version": STR, "platform": enum("windows", "macos"),
        "problem": STR, "impact": STR, "requires_admin": {"type": "boolean"},
        "applicability": enum("applicable", "not_applicable", "unknown"), "applicability_reason": nullable(STR),
        "user_protected": {"type": "boolean"}})
    defs["maintenance_catalog"] = record("maintenance_catalog", {"items": arr(ref("maintenance_task_info"))})
    outcome = enum("applied", "unchanged", "skipped", "unavailable", "attention", "failed")
    defs["maintenance_report"] = record("maintenance_report", {"operation_id": STR, "plan_id": STR,
        "elevation": enum("not_needed", "granted", "declined"), "items": arr(obj({"task_id": STR, "outcome": outcome, "reason": STR})),
        "counts": obj({k: COUNT for k in outcome["enum"]}), "all_clear": {"type": "boolean"}})
    defs["metric"] = obj({"name": STR, "availability": enum("available", "unavailable", "permission_required"),
        "value": nullable({"type": "number"}), "unit": STR, "reason": nullable(STR)})
    defs["status_snapshot"] = record("status_snapshot", {"sampled_at": TIME, "stale": {"type": "boolean"},
        "metrics": arr(ref("metric"), 1), "health": obj({"score": nullable({"type": "integer", "minimum": 0, "maximum": 100}),
            "weights": obj({"cpu": COUNT, "memory": COUNT, "disk": COUNT, "thermal": COUNT, "io": COUNT}),
            "lowered_by": arr(STR), "dimensions": arr(obj({"dimension": enum("cpu", "memory", "disk", "thermal", "io"), "metric": STR,
                "value": nullable({"type": "number"}), "full_score_at": {"type": "number"}, "zero_score_at": {"type": "number"},
                "score": nullable({"type": "integer", "minimum": 0, "maximum": 100})}))}),
        "diagnosis": obj({"code": STR, "message": STR, "link": nullable(STR)})})
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
        "count": nullable(COUNT), "size_basis": enum("file_metadata", "directory_walk", "system_query", "not_measured"),
        "size_note": nullable(STR), "observed_at": nullable(TIME), "explanation": STR, "handling": enum("read_only_guidance",
            "open_system_settings", "open_owner_tool", "manual_trash_review", "installer_review"),
        "guidance": arr(obj({"tool": STR, "steps": STR, "requires_admin": {"type": "boolean"}, "impact": STR}))})
    defs["insight_list"] = record("insight_list", {"items": arr(ref("insight"))})
    defs["doctor_report"] = record("doctor_report", {"generated_at": TIME, "redacted": {"type": "boolean"},
        "engine_version": STR, "platform": enum("windows", "macos"), "capabilities": arr(ref("capability")),
        "checks": arr(obj({"check": STR, "status": enum("ok", "warning", "failed", "unknown"), "detail": STR}))})
    defs["purge_roots"] = record("purge_roots", {"roots": arr(obj({"path": ref("path"), "source": enum("default", "user"),
        "exists": {"type": "boolean"}}))})
    install_method = enum("standalone", "homebrew", "winget", "scoop", "desktop_bundle")
    defs["release_info"] = record("release_info", {"current_version": STR, "latest_version": nullable(STR), "release_id": nullable(STR),
        "install_method": install_method, "status": enum("update_available", "up_to_date", "use_package_manager", "cannot_check"),
        "reason": nullable(STR), "checked_at": TIME})
    defs["self_update_result"] = record("self_update_result", {"status": enum("updated", "verification_failed", "locked",
        "use_package_manager"), "from_version": STR, "to_version": nullable(STR), "signature_verified": {"type": "boolean"},
        "checksum_verified": {"type": "boolean"}, "reason": nullable(STR)})
    defs["self_remove_preview"] = record("self_remove_preview", {"remove": arr(obj({"component": enum("cli", "desktop", "resident",
        "helper", "caches", "startup_entry"), "display": STR})), "keep": arr(obj({"data": enum("configuration", "protection_list",
        "history", "quarantine", "registry_backups"), "display": STR})), "quarantine_items": COUNT, "quarantine_logical_bytes": UINT,
        "system_settings_to_restore": arr(STR), "requires_interactive_confirmation": {"const": True}})
    defs["completion_script"] = record("completion_script", {"shell": enum("bash", "zsh", "fish", "powershell"), "script": STR,
        "profile_path": nullable(ref("path")), "written": {"type": "boolean"}})
    kinds = ["capabilities", "scan_result", "candidate", "plan", "operation_report", "error", "progress",
             "scan_status", "scan_cancel_receipt", "entries_page", "candidates_page", "analysis_result",
             "authorization_status", "operation_status", "operation_cancel_receipt", "quarantine_list", "operation_list",
             "cli_request", "selection_preview", "app_list", "startup_list", "maintenance_catalog", "maintenance_report",
             "status_snapshot", "protection_list", "protection_check", "insight_list", "doctor_report", "purge_roots", "release_info",
             "self_update_result", "self_remove_preview", "completion_script"]
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
        "needs-approval": rec("error", code="needs_approval", message="Plan needs user authorization", retryable=False,
            required_action="Review the plan in the trusted UI", details={"plan_id": "p_demo", "operation_id": None, "scope_id": "scope_demo"}),
        "operation-trash": rec("operation_report", operation_id="op_demo", plan_id="p_demo", plan_digest=digest,
            status="completed", started_at="2026-10-14T12:02:00Z", finished_at="2026-10-14T12:02:01Z",
            succeeded=1, skipped=0, failed=0, not_started=0, unknown=0,
            items=[{"action_id": "a_demo", "status": "succeeded", "code": "moved_to_trash", "message": "Synthetic same-volume move", "recovery": "trash"}],
            processed_logical_bytes="104857600", quarantined_logical_bytes="0", estimated_freed_bytes="0",
            volume_free_before_bytes="1000000000", volume_free_after_bytes="1000000000", observed_free_delta_bytes="0"),
        "progress": rec("progress", task_id="s_demo", sequence=0, phase="discovering", completed_units=12, total_units=None, emitted_at=t1)
    }
    for name, example in examples.items():
        save("examples/contracts/" + name + ".json", example)
    mac = json.loads(json.dumps(candidate))
    mac["path"] = {"platform": "macos", "display": "/Users/example/Library/Caches/ExampleApp/old.tmp",
                   "raw_encoding": "unix_bytes_base64", "raw_base64": base64.b64encode(b"/Users/example/Library/Caches/ExampleApp/old.tmp").decode("ascii")}
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
            {"action_id": "a01", "action_type": "maintenance_task", "task_id": "windows.dns-flush", "task_version": "0.1.0", "requires_admin": False},
            {"action_id": "a02", "action_type": "maintenance_task", "task_id": "windows.trim-when-due", "task_version": "0.1.0", "requires_admin": True},
            {"action_id": "a03", "action_type": "settings_change", "setting_id": "display.keep_awake", "from_value": "off",
             "to_value": "on_60_minutes", "restore_on_exit": True}], requires_admin=True),
        "plan-v2-startup": plan_v2("p_v2_startup", "startup", [
            {"action_id": "a01", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "scheduled_task",
             "operation": "disable", "scope": "user"}]),
        "plan-v2-recovery": plan_v2("p_v2_recovery", "recovery", [
            {"action_id": "a01", "action_type": "registry_restore", "backup_id": "rb_demo", "hive": "HKCU",
             "key_path": "Software\\ExampleVendor\\ExampleApp", "on_existing": "skip"},
            {"action_id": "a02", "action_type": "startup_toggle", "startup_item_id": "st_demo", "source": "run_key",
             "operation": "restore", "scope": "user"},
            {"action_id": "a03", "action_type": "settings_change", "setting_id": "display.keep_awake", "from_value": "on_60_minutes",
             "to_value": "off", "restore_on_exit": False}]),
    }
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
    cli = {
        "request-clean-preview": rec("cli_request", command="clean.preview", output="json", non_interactive=True, dry_run=True,
            params={"scan_id": "s_demo", "cursor": None, "page_size": 50, "include_tiers": ["default_selected"], "object_ids": [], "mode": None}),
        "request-apps-uninstall": rec("cli_request", command="apps.uninstall.plan", output="json", non_interactive=True, dry_run=False,
            params={"app_instance_id": "app_demo", "include_groups": ["bundle", "caches_logs"]}),
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
        "needs-approval-uninstall": rec("error", code="needs_approval", message="Uninstall plan needs user review", retryable=False,
            required_action="Review the plan in the desktop app or with plan review", details={"plan_id": "p_v2_uninstall",
            "operation_id": None, "scope_id": None}),
        "unsupported-capability": rec("error", code="unsupported", message="Fan control is not available on this platform",
            retryable=False, required_action=None, details={"plan_id": None, "operation_id": None, "scope_id": None}),
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
        "maintenance-catalog": rec("maintenance_catalog", items=[
            {"task_id": "windows.dns-flush", "task_version": "0.1.0", "platform": "windows", "problem": "Stale name resolution",
             "impact": "First lookups are slower briefly", "requires_admin": False, "applicability": "applicable",
             "applicability_reason": None, "user_protected": False},
            {"task_id": "windows.trim-when-due", "task_version": "0.1.0", "platform": "windows", "problem": "SSD retrim overdue",
             "impact": "Short disk activity", "requires_admin": True, "applicability": "not_applicable",
             "applicability_reason": "Last optimization is within the schedule", "user_protected": False}]),
        "maintenance-report": rec("maintenance_report", operation_id="op_maint", plan_id="p_v2_maintenance", elevation="declined",
            items=[{"task_id": "windows.dns-flush", "outcome": "applied", "reason": "Resolver cache flushed"},
                   {"task_id": "windows.trim-when-due", "outcome": "skipped", "reason": "Administrator authorization declined"}],
            counts={"applied": 1, "unchanged": 0, "skipped": 1, "unavailable": 0, "attention": 0, "failed": 0}, all_clear=False),
        "status-snapshot": rec("status_snapshot", sampled_at=t1, stale=False, metrics=[
            {"name": "cpu.total_percent", "availability": "available", "value": 37.5, "unit": "percent", "reason": None},
            {"name": "thermal.cpu_celsius", "availability": "unavailable", "value": None, "unit": "celsius",
             "reason": "No public sensor interface on this synthetic host"}],
            health={"score": 82, "weights": {"cpu": 30, "memory": 25, "disk": 20, "thermal": 15, "io": 10}, "lowered_by": ["disk"],
                    "dimensions": [{"dimension": "cpu", "metric": "cpu.total_percent", "value": 37.5, "full_score_at": 60, "zero_score_at": 95,
                                    "score": 100},
                                   {"dimension": "thermal", "metric": "thermal.max_zone_celsius", "value": None, "full_score_at": 70,
                                    "zero_score_at": 95, "score": None}]},
            diagnosis={"code": "low_free_space", "message": "System volume free space is low", "link": "analyze"}),
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
            checks=[{"check": "protection_list", "status": "ok", "detail": "3 entries"},
                    {"check": "index_store", "status": "unknown", "detail": "Synthetic report"}]),
    }
    cli.update({
        "purge-roots": rec("purge_roots", roots=[{"path": wpath("C:\\Users\\Example\\Projects"), "source": "default", "exists": True},
            {"path": wpath("D:\\Code"), "source": "user", "exists": False}]),
        "plan-v2-update": plan_v2("p_v2_update", "update", [
            {"action_id": "a01", "action_type": "app_quit", "app_instance_id": "app_demo", "app_identity": "Example.App", "force": "ask_user"},
            {"action_id": "a02", **adapter, "adapter_id": "example.adapter.winget-upgrade", "argv": ["upgrade", "--id", "Example.App", "--exact"]}]),
        "release-info": rec("release_info", current_version="0.0.0-design-example", latest_version=None, release_id=None,
            install_method="winget", status="use_package_manager", reason="Installed by a package manager", checked_at=t1),
        "self-update-result": rec("self_update_result", status="verification_failed", from_version="0.0.0-design-example",
            to_version=None, signature_verified=True, checksum_verified=False, reason="Checksum did not match the signed manifest"),
        "self-remove-preview": rec("self_remove_preview", remove=[{"component": "cli", "display": "Command-line tool"},
            {"component": "resident", "display": "Tray component and its login entry"}],
            keep=[{"data": "configuration", "display": "Settings"}, {"data": "protection_list", "display": "Protection list"},
                  {"data": "history", "display": "Operation history"}, {"data": "quarantine", "display": "Quarantine items"}],
            quarantine_items=2, quarantine_logical_bytes="2097152", system_settings_to_restore=["display.keep_awake"],
            requires_interactive_confirmation=True),
        "completion-powershell": rec("completion_script", shell="powershell", script="# synthetic completion script",
            profile_path=None, written=False),
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
        "entries-page": rec("entries_page", scan_id="s_demo", scope_id="scope_demo", revision=1, sort="logical_desc",
            filter={"kind": "any", "name_contains": None, "parent_entry_id": "e_root"}, page_size=3, next_cursor="cur_opaque_1",
            unknown_size_items=1, items=[
                entry_item("e_videos", "C:\\Users\\Example\\Videos", "directory", "73400320", "subtree_known"),
                entry_item("e_iso", "C:\\Users\\Example\\image.iso", "file", "31457280"),
                entry_item("e_denied", "C:\\Users\\Example\\Private", "directory", None, "unknown")]),
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


if __name__ == "__main__":
    main()
