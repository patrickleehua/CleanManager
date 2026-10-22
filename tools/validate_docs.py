"""Validate documentation contracts, semantic invariants and local Markdown links."""
from __future__ import annotations
import argparse
import base64
import copy
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
# A project-local dependency directory is optional; no global packages are changed.
local_deps = ROOT / ".research" / "python-deps"
if local_deps.exists():
    sys.path.insert(0, str(local_deps))
from jsonschema import Draft202012Validator, FormatChecker


def load(path):
    def unique(pairs):
        result = {}
        for k, v in pairs:
            if k in result:
                raise ValueError("duplicate JSON key: " + k)
            result[k] = v
        return result
    return json.loads(path.read_text(encoding="utf-8-sig"), object_pairs_hook=unique)


def semantics(value):
    def walk(x):
        if isinstance(x, dict):
            for k, v in x.items():
                if k == "field_sources":
                    continue  # maps field names to provenance, not byte values
                if isinstance(v, str) and (k.endswith("_bytes") or k in ("logical_bytes", "allocated_bytes")):
                    n = int(v)
                    if k == "observed_free_delta_bytes":
                        assert abs(n) <= 2**64 - 1, "delta out of range"
                    else:
                        assert 0 <= n <= 2**64 - 1, "bytes out of range"
                walk(v)
            if "raw_encoding" in x:
                raw = base64.b64decode(x["raw_base64"], validate=True)
                if x["platform"] == "windows":
                    assert x["raw_encoding"] == "windows_utf16le_base64", "platform encoding mismatch"
                    assert len(raw) % 2 == 0, "odd UTF-16 byte length"
                    units = [int.from_bytes(raw[n:n+2], "little") for n in range(0, len(raw), 2)]
                    assert 0 not in units, "NUL path component"
                else:
                    assert x["raw_encoding"] == "unix_bytes_base64", "platform encoding mismatch"
                    assert b"\x00" not in raw, "NUL path component"
            if "started_at" in x and x.get("finished_at") is not None:
                assert datetime.fromisoformat(x["started_at"]) <= datetime.fromisoformat(x["finished_at"]), "reversed timestamps"
        elif isinstance(x, list):
            for v in x:
                walk(v)
    walk(value)
    kind = value.get("kind")
    if kind == "scan_result":
        if value["status"] == "completed":
            assert value["completeness"] == "verified_scope" and not value["issues"] and value["skipped_ranges"] == 0, "false complete scan"
        else:
            assert value["completeness"] == "partial_scope", "inconsistent incomplete scan"
    elif kind == "candidate":
        assert value["risk"] != "blocked" or not value["allowed_modes"], "blocked candidate has allowed modes"
        if value["estimate_basis"] in ("not_computed", "shared_blocks_unknown"):
            assert value["reclaimable_estimate_bytes"] is None, "unknown estimate must be null"
    elif kind == "plan":
        body = value["body"]
        assert datetime.fromisoformat(body["created_at"]) < datetime.fromisoformat(body["expires_at"]), "invalid plan expiry"
        actions = [x["action_id"] for x in body["items"]]
        assert actions == sorted(set(actions)), "action IDs must be sorted and unique"
        if body["engine_contract"] == "recovery/v1":
            entries = [x["quarantine_id"] for x in body["items"]]
        elif body["engine_contract"] == "plan/v2":
            entries = [x["target"]["entry_id"] for x in body["items"] if "target" in x]
            for x in body["items"]:
                if x.get("target_location") == "platform_trash":
                    assert body["plan_kind"] == "manual_selection" and x["action_type"] == "fs_delete", "only manual permanent delete may target the platform trash"
            needs_admin = any(x.get("requires_admin") or x.get("hive") == "HKLM" or x.get("scope") == "system" for x in body["items"])
            assert body["requires_admin"] == needs_admin, "plan requires_admin does not match its actions"
        else:
            entries = [x["entry_id"] for x in body["items"]]
        assert len(entries) == len(set(entries)), "duplicate plan entry"
        canonical = json.dumps(body, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")
        assert hashlib.sha256(canonical).hexdigest() == value["plan_digest"], "plan digest mismatch"
    elif kind == "operation_report":
        for name in ("succeeded", "skipped", "failed", "not_started", "unknown"):
            assert value[name] == sum(i["status"] == name for i in value["items"]), "action count mismatch"
        assert len({i["action_id"] for i in value["items"]}) == len(value["items"]), "duplicate action result"
        if value["status"] == "completed":
            assert value["succeeded"] == len(value["items"]) and value["unknown"] == 0, "false completed operation"
        if value["unknown"]:
            assert value["status"] == "reconciliation_required", "unknown action must require reconciliation"
        before, after, delta = (value[k] for k in ("volume_free_before_bytes", "volume_free_after_bytes", "observed_free_delta_bytes"))
        if before is None or after is None:
            assert delta is None, "delta requires both observations"
        else:
            assert delta is not None and int(delta) == int(after) - int(before), "incorrect observed delta"
        processed = int(value["processed_logical_bytes"])
        assert int(value["quarantined_logical_bytes"]) <= processed and int(value["estimated_freed_bytes"]) <= processed, "moved or freed more than processed"
        if any(i["recovery"] in ("quarantine", "trash") for i in value["items"] if i["status"] == "succeeded"):
            assert int(value["estimated_freed_bytes"]) < processed or processed == 0, "same-volume moves are not freed space"
    elif kind == "operation_list":
        assert len(value["items"]) <= value["page_size"], "page holds more items than page_size"
        for o in value["items"]:
            availability = o["recovery"]["availability"]
            if o["action"] in ("delete", "purge"):
                assert availability in ("unavailable", "not_applicable", "unknown"), "permanent deletes offer no undo"
            if o["action"] == "trash":
                assert availability in ("system_recycle_bin", "not_applicable", "unknown"), "trash is restored with the system UI only"
            if availability == "in_app_restore":
                # Quarantined objects, and startup changes through their recorded prior state (recovery plan).
                assert o["action"] in ("quarantine", "startup"), "only quarantined objects and startup changes are restorable in the product"
            if o["counts"] is None or o["counts"]["unknown"]:
                assert availability == "unknown", "unconfirmed outcomes cannot claim a recovery path"
    elif kind == "cli_request":
        params = value["params"]
        if value["non_interactive"]:
            assert "review_only" not in params.get("include_tiers", []), "non-interactive requests cannot widen beyond default_selected"
        if value["command"] == "protect.add" and params["entry"]["path"] is not None:
            shown = params["entry"]["path"]["display"]
            absolute = re.match(r"^[A-Za-z]:\\", shown) if params["entry"]["path"]["platform"] == "windows" else shown.startswith("/")
            assert absolute and ".." not in re.split(r"[\\/]", shown), "protection paths must be absolute without traversal"
    elif kind == "selection_preview":
        items = value["items"]
        for i in items:
            if i["selected_by_default"]:
                assert i["tier"] == "default_selected" and i["skip_reason"] is None, "only unskipped default_selected items may be preselected"
            if i["rebuild_class"] != "local_rebuild":
                assert i["tier"] != "default_selected", "network or non-rebuildable items cannot be default_selected"
            if i["tier"] == "blocked":
                assert i["skip_reason"] is not None, "blocked items need a reason"
        s = value["summary"]
        chosen = [i for i in items if i["selected_by_default"]]
        assert s["selected_count"] == len(chosen), "summary selected_count mismatch"
        assert int(s["selected_known_bytes"]) == sum(int(i["logical_bytes"]) for i in chosen if i["logical_bytes"] is not None), "summary bytes mismatch"
        assert s["selected_unknown_size_count"] == sum(i["logical_bytes"] is None for i in chosen), "summary unknown count mismatch"
        assert s["review_only_count"] == sum(i["tier"] == "review_only" for i in items), "summary review_only mismatch"
        assert s["blocked_count"] == sum(i["tier"] == "blocked" for i in items), "summary blocked mismatch"
        assert s["skipped_count"] == sum(i["skip_reason"] is not None for i in items), "summary skipped mismatch"
    elif kind in ("entries_page", "candidates_page"):
        assert len(value["items"]) <= value["page_size"], "page holds more items than page_size"
        key = "entry_id" if kind == "entries_page" else "candidate_id"
        ids = [i[key] for i in value["items"]]
        assert len(ids) == len(set(ids)), "duplicate item in page"
        if kind == "entries_page":
            assert value["unknown_size_items"] == sum(i["logical_bytes"] is None for i in value["items"]), "unknown size count mismatch"
            for i in value["items"]:
                assert (i["logical_bytes"] is None) == (i["size_basis"] == "unknown"), "unknown size must be null with basis unknown"
    elif kind == "quarantine_list":
        assert len(value["items"]) <= value["page_size"], "page holds more items than page_size"
        assert int(value["total_logical_bytes"]) == sum(int(i["logical_bytes"]) for i in value["items"]) or value["next_cursor"], "quarantine total mismatch"
    elif kind == "authorization_status":
        if value["status"] == "authorized":
            assert value["source"] is not None and value["principal"] is not None and value["expires_at"] is not None, "authorization needs a source, principal and expiry"
            if value["source"] == "policy":
                assert value["policy_id"] and value["quota"] is not None, "policy authorization needs policy id and quota"
                q = value["quota"]
                assert int(q["used_bytes"]) + int(q["reserved_unknown_bytes"]) <= int(q["limit_bytes"]) and q["used_items"] <= q["limit_items"], "quota exceeded"
        elif value["status"] == "required":
            assert value["source"] is None and value["authorized_at"] is None, "unauthorized plan cannot carry an authorization source"
    elif kind == "operation_status":
        assert value["state"] != "running" or value["remaining"] > 0 or value["unknown"] > 0, "running operation without remaining work"
    elif kind == "analysis_result":
        if value["scan_completeness"] == "verified_scope":
            assert value["pending_evidence_count"] == 0 or value["issues"], "pending evidence needs an issue"
    elif kind == "maintenance_report":
        for name, count in value["counts"].items():
            assert count == sum(i["outcome"] == name for i in value["items"]), "maintenance outcome count mismatch"
        if value["all_clear"]:
            assert value["counts"]["failed"] == 0 and value["counts"]["attention"] == 0, "all_clear with failed or attention tasks"
    elif kind == "maintenance_history":
        for run in value["items"]:
            for name, count in run["counts"].items():
                assert count == sum(i["outcome"] == name for i in run["items"]), "maintenance outcome count mismatch"
            assert not run["all_clear"] or (run["counts"]["failed"] == 0 and run["counts"]["attention"] == 0), "all_clear with failed or attention tasks"
    elif kind == "status_snapshot":
        for m in value["metrics"]:
            assert (m["value"] is not None) == (m["availability"] == "available"), "unavailable metrics must be null, never 0"
        assert sum(value["health"]["weights"].values()) == 100, "health weights must sum to 100"
        for d in value["health"]["dimensions"]:
            assert (d["value"] is None) == (d["score"] is None), "an unknown dimension has no score, never 0"
        net = value.get("network")
        if net is not None:
            assert abs(sum(i["received_bytes_per_sec"] for i in net["counted"]) - net["received_bytes_per_sec"]) < 1e-6, "network total is the counted interfaces only"
            assert abs(sum(i["sent_bytes_per_sec"] for i in net["counted"]) - net["sent_bytes_per_sec"]) < 1e-6, "network total is the counted interfaces only"
            assert net["may_double_count"] == (net["classification"] == "unavailable" or any(i["class"] == "unknown" for i in net["counted"])), "an unclassified total must say it may double count"
    elif kind == "insight_list":
        for i in value["items"]:
            assert (i["logical_bytes"] is None) == (i["size_basis"] == "not_measured"), "unknown size must be null with basis not_measured"
            assert i["logical_bytes"] is not None or i["size_note"], "an unknown size needs an explanation"
    elif kind == "protection_list":
        for e in value["items"]:
            assert e["removable"] == (e["layer"] == "user"), "only user protection entries are removable"
    elif kind == "protection_check":
        layers = [l["layer"] for l in value["layers"]]
        assert layers == ["hard_deny", "default_category", "user"], "layers are reported strongest first, once each"
        first = next((l for l in value["layers"] if l["result"] != "clear"), None)
        assert (value["decision"] == "denied") == (first is not None), "any denied or unavailable layer denies"
        assert value["layer"] == (first and first["layer"]) and (value["reason"] is None) == (first is None), "decision names the strongest denying layer"
    elif kind == "app_list":
        for a in value["items"]:
            if a["protected_class"]:
                assert a["uninstall_capability"]["state"] != "available", "protected applications cannot be uninstallable"
    elif kind == "progress" and value["total_units"] is not None:
        assert value["completed_units"] <= value["total_units"], "progress beyond total"
    if "root_resolver" in value and value.get("schema_version") == "1.0":
        assert all(c not in (".", "..") and not any(v in c for v in ("/", "\\", "\x00", ":")) for c in value["relative_components"]), "invalid relative component"
        if value["status"] in ("verified", "enabled"):
            assert value["evidence_refs"], "verified rules require evidence"


def rule_v2_semantics(value, registry):
    """Rule 2.0 invariants that JSON Schema cannot express on its own."""
    entries = value["match"]["relative_components"] + value["exclude"]
    assert all(".." not in c and "\x00" not in c for c in entries), "rule path escapes its resolved root"
    if value["status"] == "enabled":
        entry = registry.get(value["tested_registry_ref"])
        assert entry is not None, "enabled rule references a missing tested registry entry"
        assert (entry["rule_id"], entry["rule_version"]) == (value["rule_id"], value["version"]), "registry entry is for another rule version"
        assert entry["result"] == "passed" and not entry["synthetic"], "enabled rule needs a passed, non-synthetic registry entry"
        tested = {p["platform"] for p in entry["platforms"]}
        assert set(value["platforms"]) <= tested, "enabled rule claims an untested platform"


# Markdown links: only web/mail URLs may leave the repository. A drive-letter path, a `file:` URL, a UNC or
# rooted path, any other scheme, a target outside the checkout or inside a git-ignored local folder works on
# the author's machine only and is dead on GitHub (e.g. links into a local agent session or raw evidence).
EXTERNAL_LINK = re.compile(r"^(https?|mailto):", re.I)
IGNORED_TOP_DIRS = {".research", "target", "node_modules", ".playwright-mcp", ".venv", "venv"}


def link_problem(doc, target, root=ROOT):
    """None if the Markdown link target is acceptable, else the reason it is not."""
    target = target.strip().strip("<>").strip()
    if not target:
        return "empty link target"
    if target.startswith("#") or EXTERNAL_LINK.match(target):
        return None
    if re.match(r"^[A-Za-z]:", target):
        return "absolute Windows drive path"
    if re.match(r"^file:", target, re.I):
        return "file: URL"
    if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target):
        return "unsupported link scheme (only http, https and mailto may leave the repository)"
    if target.startswith(("/", "\\")):
        return "rooted or UNC path"
    from urllib.parse import unquote
    resolved = (doc.parent / unquote(target.split("#", 1)[0])).resolve()
    try:
        rel = resolved.relative_to(root.resolve())
    except ValueError:
        return "link outside the repository"
    if rel.parts and rel.parts[0] in IGNORED_TOP_DIRS:
        return "link into a git-ignored local folder"
    if not resolved.exists():
        return "missing target"
    return None


# Run records are published: a user's home directory must be redacted to `~` (or the user to `<user>`),
# as doctor does. Synthetic placeholders and the shared Windows/macOS profiles are fine.
HOME_PATH = re.compile(r"(?i)\b[a-z]:[\\/]+users[\\/]+(?!(?:<user>|public|default|all users)(?:[\\/]|$))[^\\/\s\"]+"
                       r"|(?<![\w.~])/Users/(?!(?:<user>|Shared)(?:/|$))[^/\s\"]+")


def unredacted_home(value):
    """The first string in a JSON value that still names a real user's home directory, else None."""
    if isinstance(value, str):
        return value if HOME_PATH.search(value) else None
    items = value.values() if isinstance(value, dict) else value if isinstance(value, list) else ()
    for item in items:
        hit = unredacted_home(item)
        if hit is not None:
            return hit
    return None


def device_claim_semantics(matrix, runs):
    """SCN-07: a platform claim is verified only by passed runs observed on a real device of that platform."""
    devices = {d["device_id"]: d for d in matrix["devices"]}
    assert len(devices) == len(matrix["devices"]), "duplicate device id"
    for d in matrix["devices"]:
        if d["actual"] is not None:
            assert d["actual"]["platform"] == d["expected"]["platform"], "device actual platform differs from expected"
    for claim in matrix["claims"]:
        assert claim["state"] in ("unverified", "verified", "failed"), "unknown claim state"
        if claim["state"] == "unverified":
            continue
        assert claim["run_ids"], "a verified or failed claim needs run records"
        for run_id in claim["run_ids"]:
            run = runs.get(run_id)
            assert run is not None, "claim references a missing run record"
            assert run["os"]["platform"] == claim["platform"], "claim backed by another platform's run"
            device = devices.get(run["device_id"])
            assert device is not None and device["actual"] is not None, "run comes from a device without observed facts"
            assert device["actual"]["platform"] == claim["platform"], "run device platform differs from claim"
            if claim["state"] == "verified":
                assert run["status"] == "passed" and run["actual"] is not None, "verified claim needs passed runs with actual results"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps({"status": "validation_in_progress", "scope": "Documentation only"}) + "\n", encoding="utf-8")
    schemas = {name: load(ROOT / "schemas" / "v1" / (name + ".schema.json")) for name in ("protocol", "rule", "rule-v2", "tested-registry", "run-record")}
    validators = {}
    for name, schema in schemas.items():
        Draft202012Validator.check_schema(schema)
        validators[name] = Draft202012Validator(schema, format_checker=FormatChecker())
    examples = {}
    for path in sorted((ROOT / "examples/contracts").glob("*.json")):
        value = load(path)
        validators["protocol"].validate(value)
        semantics(value)
        examples[path.stem] = value
    rules = list((ROOT / "rules/examples").glob("*.json"))
    for path in rules:
        value = load(path)
        validators["rule"].validate(value)
        semantics(value)
    registry = {}
    registry_paths = sorted((ROOT / "rules/registry/examples").glob("*.json"))
    for path in registry_paths:
        value = load(path)
        validators["tested-registry"].validate(value)
        assert value["registry_id"] == value["rule_id"] + "@" + value["rule_version"], "registry id mismatch"
        assert value["registry_id"] not in registry, "duplicate registry entry"
        registry[value["registry_id"]] = value
    rules_v2 = {}
    for path in sorted((ROOT / "rules/examples/v2").glob("*.json")):
        value = load(path)
        validators["rule-v2"].validate(value)
        rule_v2_semantics(value, registry)
        assert value["status"] == "experimental", "example rules must stay experimental"
        rules_v2[path.stem] = value
    obs_schema = {"$schema": "https://json-schema.org/draft/2020-12/schema", "$defs": schemas["protocol"]["$defs"],
                  "$ref": "#/$defs/observation_record"}
    obs = load(ROOT / "examples/c14n/observation-v1.json")
    Draft202012Validator(obs_schema, format_checker=FormatChecker()).validate(obs["record"])
    semantics(obs["record"])
    assert hashlib.sha256(json.dumps(obs["record"], sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode("utf-8")).hexdigest() == obs["fingerprint_sha256"], "observation fingerprint mismatch"
    golden = load(ROOT / "examples/c14n/golden.json")
    for case in golden["cases"]:
        canon = json.dumps(json.loads(case["input"]), sort_keys=True, ensure_ascii=False, separators=(",", ":"))
        assert canon == case["canonical"] and hashlib.sha256(canon.encode("utf-8")).hexdigest() == case["sha256"], "c14n golden mismatch: " + case["name"]
    runs = {}
    for path in sorted((ROOT / "docs/evidence/runs").glob("*.json")):
        value = load(path)
        validators["run-record"].validate(value)
        assert value["run_id"] not in runs, "duplicate run id"
        hit = unredacted_home(value)
        assert hit is None, f"run record {path.name} contains an unredacted home path (use ~ or <user>): {hit[:120]}"
        runs[value["run_id"]] = value
    # Production rule packs: schema, registry-backed promotion and existing passed run evidence.
    prod_registry = {}
    for path in sorted((ROOT / "rules/registry").glob("*.json")):
        value = load(path)
        validators["tested-registry"].validate(value)
        assert not value["synthetic"], "production registry entries cannot be synthetic: " + path.name
        for ref in value["evidence_refs"]:
            assert ref in runs and runs[ref]["status"] == "passed", "registry evidence must be a passed run record: " + ref
        prod_registry[value["registry_id"]] = value
    prod_rules = 0
    for pack in ("windows", "macos"):
        for path in sorted((ROOT / "rules" / pack).glob("*.json")):
            value = load(path)
            validators["rule-v2"].validate(value)
            rule_v2_semantics(value, prod_registry)
            assert pack in value["platforms"], "rule pack directory and platforms disagree: " + path.name
            prod_rules += 1
    matrix = load(ROOT / "docs/evidence/device-matrix.json")
    device_claim_semantics(matrix, runs)
    negative = []
    def reject(name, source, mutate, validator="protocol"):
        bad = copy.deepcopy(source)
        mutate(bad)
        try:
            validators[validator].validate(bad)
            if validator == "rule-v2":
                rule_v2_semantics(bad, registry)
            else:
                semantics(bad)
        except Exception as exc:
            from jsonschema.exceptions import ValidationError
            if not isinstance(exc, (ValidationError, AssertionError, ValueError)):
                raise
            negative.append(name)
            return
        raise AssertionError("invalid example accepted: " + name)
    reject("unknown-field", examples["candidate"], lambda x: x.update(arbitrary_shell="echo no"))
    reject("numeric-byte-count", examples["candidate"], lambda x: x.update(logical_bytes=104857600))
    reject("u64-overflow", examples["candidate"], lambda x: x.update(logical_bytes=str(2**64)))
    reject("tampered-plan", examples["plan"], lambda x: x["body"].update(mode="delete"))
    reject("false-complete-scan", examples["scan-partial"], lambda x: x.update(status="completed"))
    reject("blocked-action", examples["candidate"], lambda x: x.update(risk="blocked"))
    reject("incorrect-freed-space", examples["operation-trash"], lambda x: x.update(observed_free_delta_bytes="104857600"))
    reject("encoding-mismatch", examples["candidate"], lambda x: x["path"].update(raw_encoding="unix_bytes_base64"))
    def reject_claim(name, mutate_matrix, extra_run=None):
        bad, pool = copy.deepcopy(matrix), dict(runs)
        if extra_run is not None:
            validators["run-record"].validate(extra_run)
            pool[extra_run["run_id"]] = extra_run
        mutate_matrix(bad)
        try:
            device_claim_semantics(bad, pool)
        except AssertionError:
            negative.append(name)
            return
        raise AssertionError("invalid device claim accepted: " + name)
    win_run = copy.deepcopy(runs["run_template"]) | {"run_id": "run_synthetic_win", "status": "passed",
        "actual": "Synthetic observed result", "terminal_state": "completed", "observer": "validator"}
    def first(m, **match):
        return next(c for c in m["claims"] if all(c[k] == v for k, v in match.items()))
    def claim_mac_with_windows(m):
        first(m, platform="macos").update(state="verified", run_ids=["run_synthetic_win"])
    reject_claim("scn07-mac-claim-from-windows-run", claim_mac_with_windows, win_run)
    reject_claim("scn07-verified-without-runs", lambda m: first(m, platform="windows", state="unverified").update(state="verified"))
    reject_claim("scn07-verified-from-unverified-run", lambda m: first(m, platform="windows", state="unverified").update(state="verified", run_ids=["run_template"]))
    reject("run-unverified-with-actual", runs["run_template"], lambda x: x.update(actual="made up"), "run-record")
    reject("run-passed-without-actual", runs["run_template"], lambda x: x.update(status="passed"), "run-record")
    for name, leaked in [("run-unredacted-windows-home", "C:\\Users\\alice\\AppData\\Local\\Temp\\fixture"), ("run-unredacted-macos-home", "/Users/alice/Library/Caches/x")]:
        assert unredacted_home(dict(runs["run_template"], command=["cleanmanager", "scan", "--root", leaked])) is not None, "unredacted home accepted: " + name
        negative.append(name)
    assert unredacted_home(["~\\AppData\\Local", "C:\\Users\\<user>\\x", "C:\\Users\\Public\\x", "/Users/Shared/x", "~/Library"]) is None, "redacted paths rejected"
    reject("unbacked-verified-rule", load(rules[0]), lambda x: x.update(status="verified"), "rule")
    def reject_plan(name, source, mutate):
        """Mutate a plan/v2 example and re-sign it, so rejection proves the intended invariant, not a stale digest."""
        def change(x):
            mutate(x["body"])
            x["plan_digest"] = hashlib.sha256(json.dumps(x["body"], sort_keys=True, ensure_ascii=False,
                separators=(",", ":"), allow_nan=False).encode("utf-8")).hexdigest()
        reject(name, source, change)
    action_types = {i["action_type"] for k, v in examples.items() if k.startswith("plan-v2-") for i in v["body"]["items"]}
    assert action_types == set(r["$ref"].rsplit("/action_", 1)[1] for r in schemas["protocol"]["$defs"]["plan_action"]["oneOf"]), "plan/v2 golden vectors must cover every action type"
    manual, maint, uninstall = examples["plan-v2-manual-selection"], examples["plan-v2-maintenance"], examples["plan-v2-uninstall"]
    reject_plan("v2-manual-policy-matchable", manual, lambda b: b.update(policy_matchable=True))
    reject_plan("v2-manual-quarantine", manual, lambda b: b["items"][0].update(action_type="fs_quarantine"))
    reject_plan("v2-manual-delete-outside-trash", manual, lambda b: b["items"][0].update(action_type="fs_delete"))
    reject_plan("v2-cleanup-targets-platform-trash", examples["plan-v2-cleanup"], lambda b: b["items"][2].update(target_location="platform_trash"))
    reject_plan("v2-maintenance-with-file-delete", maint, lambda b: b["items"].append(copy.deepcopy(examples["plan-v2-cleanup"]["body"]["items"][2]) | {"action_id": "a99"}))
    reject_plan("v2-maintenance-policy-matchable", maint, lambda b: b.update(policy_matchable=True))
    reject_plan("v2-uninstall-forced-quit", uninstall, lambda b: b["items"][0].update(force="always"))
    reject_plan("v2-registry-without-backup", uninstall, lambda b: b["items"][4].update(backup_required=False))
    reject_plan("v2-hklm-without-admin", uninstall, lambda b: b["items"][4].update(hive="HKLM"))
    reject_plan("v2-adapter-shell-string", uninstall, lambda b: b["items"][1].update(command_line="cmd /c del"))
    # Parity 12.3/7.3/10.x: one process per end plan; unidentified processes carry the AI entry; nothing preselected
    # in old downloads; "up to date" needs a channel; unknown card sizes explain themselves.
    reject_plan("v2-process-two-ends", examples["plan-v2-process"], lambda b: b["items"].append(copy.deepcopy(b["items"][0]) | {"action_id": "a02"}))
    reject_plan("v2-process-policy-matchable", examples["plan-v2-process"], lambda b: b.update(policy_matchable=True))
    reject("process-unidentified-without-ai-entry", examples["process-list"], lambda x: x["items"][0].update(ai_diagnosis=None))
    reject("old-download-preselected", examples["old-downloads-review"], lambda x: x["items"][0].update(selected=True))
    reject("update-latest-without-channel", examples["app-update-list"], lambda x: x["items"][1].update(state="up_to_date"))
    reject("card-unknown-size-without-note", examples["insight-overview"], lambda x: x["cards"][1].update(size_note=None))
    reject("scan-status-terminal-state", examples["scan-status-running"], lambda x: x.update(state="completed"))
    reject("scan-interrupted-claims-verified", examples["scan-interrupted"], lambda x: x.update(completeness="verified_scope"))
    reject("cancel-receipt-claims-stopped", examples["scan-cancel-receipt"], lambda x: x.update(note="Scan stopped."))
    reject("entries-page-over-size", examples["entries-page"], lambda x: x.update(page_size=2))
    reject("entries-page-501", examples["entries-page"], lambda x: x.update(page_size=501))
    reject("entries-page-duplicate", examples["entries-page"], lambda x: x["items"].append(copy.deepcopy(x["items"][0])) or x.update(page_size=4))
    reject("entries-unknown-as-zero", examples["entries-page"], lambda x: x["items"][2].update(logical_bytes="0"))
    reject("entries-page-without-revision", examples["entries-page"], lambda x: x.pop("revision"))
    restore = examples["recovery-plan-restore"]
    reject_plan("recovery-overwrite-on-conflict", restore, lambda b: b["items"][0].update(on_conflict="overwrite"))
    reject_plan("recovery-restore-without-target", restore, lambda b: b["items"][0]["target"].update(path=None))
    reject_plan("recovery-purge-with-restore-target", restore, lambda b: b.update(recovery_kind="purge"))
    reject("authorization-without-source", examples["authorization-policy"], lambda x: x.update(source=None))
    reject("authorization-required-with-source", examples["authorization-required"], lambda x: x.update(source="interactive_review"))
    reject("authorization-quota-exceeded", examples["authorization-policy"], lambda x: x["quota"].update(used_bytes="2147483648", reserved_unknown_bytes="1"))
    reject("authorization-client-approved-flag", examples["authorization-required"], lambda x: x.update(approved=True))
    reject("operation-status-terminal", examples["operation-status-running"], lambda x: x.update(state="completed"))
    reject("operation-cancel-claims-rollback", examples["operation-cancel-receipt"], lambda x: x.update(note="Rolled back."))
    reject("reconciliation-reported-complete", examples["operation-reconciliation"], lambda x: x.update(status="completed"))
    reject("reconciliation-reported-partial", examples["operation-reconciliation"], lambda x: x.update(status="partial"))
    reject("same-volume-move-reported-freed", examples["operation-trash"], lambda x: x.update(estimated_freed_bytes="104857600"))
    reject("quarantine-reminder-without-flag", examples["quarantine-list"], lambda x: x["items"][0].pop("reminder_due"))
    reject("quarantine-overwrite-action", examples["quarantine-list"], lambda x: x["items"][0].update(available_actions=["overwrite"]))
    history = examples["operation-list"]
    reject("history-delete-offers-undo", history, lambda x: x["items"][2].update(action="delete", counts={"succeeded": 1, "skipped": 0,
        "failed": 0, "not_started": 0, "unknown": 0}, recovery={"availability": "in_app_restore", "guidance": "Undo"}))
    reject("history-trash-in-app-restore", history, lambda x: x["items"][1]["recovery"].update(availability="in_app_restore"))
    reject("history-unknown-claims-recovery", history, lambda x: x["items"][2]["recovery"].update(availability="unavailable"))
    reject("v2-tampered-plan", uninstall, lambda x: x["body"]["items"][2]["target"].update(logical_bytes="1"))
    req, prev = examples["request-clean-preview"], examples["preview-clean"]
    reject("cli-noninteractive-review-only", req, lambda x: x["params"].update(include_tiers=["default_selected", "review_only"]))
    reject("cli-page-size-501", req, lambda x: x["params"].update(page_size=501))
    reject("cli-unknown-command", req, lambda x: x.update(command="shell.exec"))
    reject("cli-params-for-other-command", req, lambda x: x["params"].update(entry_ids=["e1"]))
    reject("cli-protect-relative-path", req, lambda x: x.update(command="protect.add", params={"entry": {"match": "exact",
        "path": {"platform": "windows", "display": "..\\Windows", "raw_encoding": "windows_utf16le_base64",
                 "raw_base64": base64.b64encode("..\\Windows".encode("utf-16-le")).decode("ascii")},
        "glob": None, "rule_id": None, "task_id": None, "note": None}}))
    trav = "C:\\Users\\..\\Windows"
    reject("cli-protect-traversal", req, lambda x: x.update(command="protect.add", params={"entry": {"match": "exact",
        "path": {"platform": "windows", "display": trav, "raw_encoding": "windows_utf16le_base64",
                 "raw_base64": base64.b64encode(trav.encode("utf-16-le")).decode("ascii")},
        "glob": None, "rule_id": None, "task_id": None, "note": None}}))
    reject("preview-blocked-selected", prev, lambda x: x["items"][3].update(selected_by_default=True))
    reject("preview-network-default-selected", prev, lambda x: x["items"][1].update(tier="default_selected"))
    reject("preview-skipped-but-selected", prev, lambda x: x["items"][2].update(selected_by_default=True))
    reject("preview-summary-mismatch", prev, lambda x: x["summary"].update(selected_known_bytes="0"))
    reject("preview-not-dry-run", prev, lambda x: x.update(dry_run=False))
    reject("maintenance-all-clear-with-failure", examples["maintenance-report"], lambda x: (x["items"][1].update(outcome="failed"),
        x["counts"].update(skipped=0, failed=1), x.update(all_clear=True)))
    reject("status-unavailable-as-zero", examples["status-snapshot"], lambda x: x["metrics"][1].update(value=0))
    reject("status-unknown-dimension-scored", examples["status-snapshot"], lambda x: x["health"]["dimensions"][1].update(score=0))
    reject("insight-unknown-size-as-zero", examples["insight-list"], lambda x: x["items"][1].update(logical_bytes="0"))
    reject("protection-hard-deny-removable", examples["protection-list"], lambda x: x["items"][0].update(removable=True))
    reject("app-protected-uninstallable", examples["app-list"], lambda x: x["items"][1]["uninstall_capability"].update(state="available"))
    cache_v2 = rules_v2["example-browser-code-cache"]
    def enable(x, ref="example.browser.code-cache@0.1.0"):
        x.update(status="enabled", tested_registry_ref=ref, evidence_refs=["synthetic:schema-example"])
    reject("v2-script-resolver", cache_v2, lambda x: x.update(root_resolver="script"), "rule-v2")
    reject("v2-cross-root-regex", cache_v2, lambda x: x["match"].update(path_regex=".*"), "rule-v2")
    reject("v2-follow-links", cache_v2, lambda x: x.update(follow_links=True), "rule-v2")
    reject("v2-component-traversal", cache_v2, lambda x: x["match"].update(relative_components=[".."]), "rule-v2")
    reject("v2-recursive-glob", cache_v2, lambda x: x["match"].update(name_glob="**"), "rule-v2")
    reject("v2-glob-separator", cache_v2, lambda x: x.update(exclude=["../secret"]), "rule-v2")
    reject("v2-enabled-without-registry", cache_v2, lambda x: enable(x, None), "rule-v2")
    reject("v2-enabled-with-synthetic-registry", cache_v2, enable, "rule-v2")
    reject("v2-network-rebuild-default-selected", rules_v2["example-package-cache-adapter"], lambda x: x.update(max_tier="default_selected"), "rule-v2")
    reject("v2-adapter-without-ref", rules_v2["example-package-cache-adapter"], lambda x: x.update(adapter_ref=None), "rule-v2")
    reject("v2-adapter-with-fs-modes", rules_v2["example-package-cache-adapter"], lambda x: x.update(allowed_modes=["delete"]), "rule-v2")
    reject("v2-blocked-with-modes", cache_v2, lambda x: x.update(max_tier="blocked"), "rule-v2")
    reject("registry-without-negative-fixture", registry["example.browser.code-cache@0.1.0"], lambda x: x["fixtures"].update(negative=[]), "tested-registry")
    # Optional AI diagnosis contract (diag/1): separate schema, own examples and negative cases.
    import runpy
    diag_examples, diag_rejected = runpy.run_path(str(ROOT / "tools/diag_contracts.py"))["validate"](ROOT, Draft202012Validator, FormatChecker, load)
    negative.extend(diag_rejected)
    evidence = ROOT / "docs/evidence"
    bundle = load(evidence / "rea-wiztree-4.33.bundle.json")
    artifact = load(evidence / "wiztree-artifact-summary.json")
    records = {record["evidence_id"]: record for record in bundle["records"]}
    assert len(records) == len(bundle["records"]), "duplicate REA evidence IDs"
    for key in ("inventory_evidence_id", "inspection_evidence_id", "extraction_evidence_id", "pe_evidence_id"):
        assert artifact[key] in records, "summary references missing REA evidence"
    for unknown in bundle["unknowns"]:
        for key in ("supporting_evidence_ids", "contradicting_evidence_ids", "mutation_evidence_ids"):
            assert all(ref in records for ref in unknown[key]), "unknown references missing evidence"
    assert artifact["archive_sha256"] == artifact["manifest"]["root_sha256"], "artifact digest mismatch"
    signature = load(evidence / "wiztree-windows-metadata.json")
    binary = next(f for f in artifact["files"] if f["path"] == "WizTree64.exe")
    assert signature["artifact_sha256"] == binary["sha256"], "signature observation identity mismatch"
    original = load(evidence / "disk-cleaner-probe.json")
    reproduced = load(evidence / "disk-cleaner-reproduced.json")
    assert original["source_commit"] == reproduced["source_commit"] and original["cases"] == reproduced["cases"], "probe results changed"
    links = 0
    paths = [ROOT / "README.md", *sorted((ROOT / "docs").rglob("*.md")), *sorted((ROOT / "examples").rglob("*.md"))]
    markdown = []
    for path in paths:
        if not path.exists():
            raise AssertionError("missing documentation file: " + str(path))
        content = path.read_text(encoding="utf-8")
        markdown.append(content)
        assert content.count("```") % 2 == 0, "unclosed fence: " + str(path)
        assert "\ufffd" not in content, "replacement character: " + str(path)
        for target in re.findall(r"\[[^\]\n]*\]\(([^)\n]+)\)", content):
            problem = link_problem(path, target)
            assert problem is None, f"bad local link ({problem}): {path.relative_to(ROOT).as_posix()}: {target}"
            if not (target.startswith("#") or EXTERNAL_LINK.match(target.strip("<> "))):
                links += 1
    # The link rules reject the shapes that are dead outside the author's machine.
    doc = ROOT / "docs" / "README.md"
    for name, bad in [("link-windows-drive-path", "C:/Users/someone/.agent/session.md"), ("link-windows-backslash-path", "C:\\Users\\someone\\a.md"),
                      ("link-file-url", "file:///C:/Users/someone/a.md"), ("link-unc-path", "\\\\server\\share\\a.md"),
                      ("link-rooted-path", "/etc/hosts"), ("link-other-scheme", "vscode://file/a.md"),
                      ("link-outside-repo", "../../../outside.md"), ("link-ignored-folder", "../.research/acceptance-raw/x.json"),
                      ("link-missing-target", "no-such-file.md")]:
        assert link_problem(doc, bad) is not None, "bad link accepted: " + bad
        negative.append(name)
    assert link_problem(doc, "https://example.com/a") is None and link_problem(doc, "mailto:a@example.com") is None and link_problem(doc, "verification-plan.md#2") is None, "valid link rejected"
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    members = {tomllib.loads(m.read_text(encoding="utf-8"))["package"]["name"] for m in (ROOT / "crates").glob("*/Cargo.toml")}
    locked = {(pkg["name"], pkg["version"]) for pkg in lock["package"] if pkg["name"] not in members}
    dep_doc = (ROOT / "docs/dependencies.md").read_text(encoding="utf-8")
    documented_deps = set(re.findall(r"^\| `([A-Za-z0-9_-]+)` \| ([0-9][^ |]*) \|", dep_doc, re.M))
    assert documented_deps == locked, "docs/dependencies.md differs from Cargo.lock; rerun tools/dependency_inventory.py: " + str(sorted(documented_deps ^ locked))
    desktop_doc = (ROOT / "docs/dependencies-desktop.md").read_text(encoding="utf-8")
    desktop_lock = tomllib.loads((ROOT / "apps/desktop/src-tauri/Cargo.lock").read_text(encoding="utf-8"))
    desktop_locked = {(pkg["name"], pkg["version"]) for pkg in desktop_lock["package"] if "source" in pkg}
    desktop_documented = set(re.findall(r"^\| `([A-Za-z0-9_-]+)` \| ([0-9][^ |]*) \|", desktop_doc, re.M))
    assert desktop_documented == desktop_locked, "docs/dependencies-desktop.md differs from the desktop Cargo.lock; rerun tools/dependency_inventory.py: " + str(sorted(desktop_documented ^ desktop_locked))
    npm_lock = json.loads((ROOT / "apps/desktop/package-lock.json").read_text(encoding="utf-8"))
    npm_locked = {(k.rsplit("node_modules/", 1)[1], p["version"]) for k, p in npm_lock["packages"].items() if k}
    npm_documented = set(re.findall(r"^\| (@?[a-z0-9][^ |`]*) \| ([0-9][^ |]*) \|", desktop_doc, re.M))
    assert npm_documented == npm_locked, "docs/dependencies-desktop.md differs from package-lock.json; rerun tools/dependency_inventory.py: " + str(sorted(npm_documented ^ npm_locked))
    # MVP 14.3: release notices are generated from the same lock files and name their digests.
    notices_doc = (ROOT / "docs/third-party-notices.md").read_text(encoding="utf-8")
    for lock_path in ("Cargo.lock", "apps/desktop/src-tauri/Cargo.lock", "apps/desktop/package-lock.json"):
        digest = hashlib.sha256((ROOT / lock_path).read_bytes()).hexdigest()
        assert f"`{lock_path}`：`{digest}`" in notices_doc, f"docs/third-party-notices.md is stale for {lock_path}; rerun tools/dependency_inventory.py"
    noticed = set(re.findall(r"^\| `([A-Za-z0-9_-]+)` \| ([0-9][^ |]*) \|", notices_doc, re.M))
    assert noticed and noticed <= (locked | desktop_locked), "docs/third-party-notices.md names crates that are not locked: " + str(sorted(noticed - (locked | desktop_locked)))
    for manifest in [ROOT / "Cargo.toml", *(ROOT / "crates").glob("*/Cargo.toml"), ROOT / "apps/desktop/src-tauri/Cargo.toml", ROOT / "apps/desktop/package.json"]:
        assert ".research" not in manifest.read_text(encoding="utf-8"), "build must not reference research copies: " + str(manifest)
    protocol_doc = (ROOT / "docs/cli-protocol.md").read_text(encoding="utf-8")
    exits = load(ROOT / "schemas/v1/exit-codes.json")
    assert set(exits["errors"]) == set(schemas["protocol"]["$defs"]["error"]["properties"]["code"]["enum"]), "exit table and error enum differ"
    rows = {int(m.group(1)): m.group(2) for m in re.finditer(r"^\| (\d+) \| [^|]+ \| ([^|]+) \|$", protocol_doc, re.M)}
    for name, code in [*exits["errors"].items(), *exits["results"].items()]:
        assert name in rows.get(code, ""), f"exit table row {code} in docs/cli-protocol.md does not list {name}"
    documented = set(re.findall(r"^\| `([a-z_]+(?:\.[a-z_]+)+)` \|", protocol_doc, re.M))
    commands = set(schemas["protocol"]["$defs"]["cli_request"]["properties"]["command"]["enum"])
    assert documented == commands, "CLI command table and schema differ: " + str(sorted(documented ^ commands))
    record_kinds = {r["$ref"].rsplit("/", 1)[1] for r in schemas["protocol"]["oneOf"]}
    fixture_kinds = {v["kind"] for v in examples.values()}
    assert record_kinds <= fixture_kinds, "record kinds without a fixture: " + str(sorted(record_kinds - fixture_kinds))
    core_section = protocol_doc.split("## 命令面与副作用", 1)[1].split("## 工具命令组", 1)[0]
    core_rows = re.findall(r"^\| `([^`]+)` \| [^|]+ \| ([^|]+) \| [^|]+ \|$", core_section, re.M)
    assert len(core_rows) >= 16, "core command table is incomplete"
    for command, responses in core_rows:
        named = set(re.findall(r"`([a-z_]+)`", responses))
        assert named and named <= record_kinds, f"core command {command} names unknown response records: {sorted(named - record_kinds)}"
    test_text = (ROOT / "docs/verification-plan.md").read_text(encoding="utf-8")
    task_text = (ROOT / "docs/roadmap.md").read_text(encoding="utf-8")
    tests = re.findall(r"^\| (TC-[A-Z0-9]+-\d{3}) \|", test_text, re.M)
    tasks = re.findall(r"^\| (TASK-\d{3}) \|", task_text, re.M)
    assert len(tests) == len(set(tests)) and len(tasks) == len(set(tasks)), "duplicate planned test/task ID"
    referenced_tests = set(re.findall(r"\bTC-[A-Z0-9]+-\d{3}\b", "\n".join(markdown)))
    referenced_tasks = set(re.findall(r"\bTASK-\d{3}\b", "\n".join(markdown)))
    assert referenced_tests <= set(tests), "undefined test IDs: " + str(sorted(referenced_tests - set(tests)))
    assert referenced_tasks <= set(tasks), "undefined task IDs: " + str(sorted(referenced_tasks - set(tasks)))
    report = {"status": "passed", "scope": "Document contracts and synthetic examples only; no product or OS behavior tested.",
              "schemas": len(schemas), "protocol_examples": len(examples), "rule_examples": len(rules), "rule_v2_examples": len(rules_v2), "registry_examples": len(registry),
              "negative_cases_rejected": negative, "local_links_checked": links, "markdown_files_checked": len(paths),
              "rea_evidence_records_linked": len(records), "rea_unknowns_linked": len(bundle["unknowns"]),
              "reference_probe_cases_matched": len(reproduced["cases"]),
              "planned_test_ids_checked": len(tests), "planned_task_ids_checked": len(tasks),
              "cli_commands_documented": len(documented), "diagnosis_examples": diag_examples,
              "run_records": len(runs), "locked_crates_documented": len(locked), "production_rules": prod_rules, "production_registry_entries": len(prod_registry), "devices_observed": sum(d["actual"] is not None for d in matrix["devices"])}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
