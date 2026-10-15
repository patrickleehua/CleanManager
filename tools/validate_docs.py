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
            if "started_at" in x and "finished_at" in x:
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
    elif kind == "progress" and value["total_units"] is not None:
        assert value["completed_units"] <= value["total_units"], "progress beyond total"
    if "root_resolver" in value:
        assert all(c not in (".", "..") and not any(v in c for v in ("/", "\\", "\x00", ":")) for c in value["relative_components"]), "invalid relative component"
        if value["status"] in ("verified", "enabled"):
            assert value["evidence_refs"], "verified rules require evidence"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps({"status": "validation_in_progress", "scope": "Documentation only"}) + "\n", encoding="utf-8")
    schemas = {name: load(ROOT / "schemas" / "v1" / (name + ".schema.json")) for name in ("protocol", "rule")}
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
    negative = []
    def reject(name, source, mutate, validator="protocol"):
        bad = copy.deepcopy(source)
        mutate(bad)
        try:
            validators[validator].validate(bad)
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
    reject("unbacked-verified-rule", load(rules[0]), lambda x: x.update(status="verified"), "rule")
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
        for target in re.findall(r"(?<!!)\[[^\]\n]*\]\(([^)\n]+)\)", content):
            if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("#"):
                continue
            target = target.split("#", 1)[0].strip("<>")
            assert (path.parent / target).resolve().exists(), f"broken local link: {path}: {target}"
            links += 1
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
              "schemas": len(schemas), "protocol_examples": len(examples), "rule_examples": len(rules),
              "negative_cases_rejected": negative, "local_links_checked": links, "markdown_files_checked": len(paths),
              "rea_evidence_records_linked": len(records), "rea_unknowns_linked": len(bundle["unknowns"]),
              "reference_probe_cases_matched": len(reproduced["cases"]),
              "planned_test_ids_checked": len(tests), "planned_task_ids_checked": len(tasks)}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
