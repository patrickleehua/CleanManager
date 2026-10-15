"""Run the real `cleanmanager` binary on a throwaway fixture and validate every stdout record (MVP 12.1/12.5).

Checks per command: stdout is UTF-8 without BOM or ANSI escapes, one compact JSON record per line, each record
validates against schemas/v1/protocol.schema.json (strict, plus the semantic invariants of validate_docs.py),
and the exit code matches the record (0/5/... per exit-codes.json). Writes only under a new temporary directory.
Usage: python -I tools/validate_cli_outputs.py [--bin target/debug/cleanmanager.exe] [--output docs/evidence/cli-output-validation.json]
"""
from __future__ import annotations
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("validate_docs", ROOT / "tools" / "validate_docs.py")
vd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vd)  # adds .research/python-deps to sys.path when present
from jsonschema import Draft202012Validator, FormatChecker  # noqa: E402

# Records outside the schemas/v1 first-version protocol (documented as later commands), never silently accepted.
OUTSIDE_V1 = {"policy_list", "policy"}


def main() -> int:
    ap = argparse.ArgumentParser()
    exe = "cleanmanager.exe" if os.name == "nt" else "cleanmanager"
    ap.add_argument("--bin", type=Path, default=ROOT / "target" / "debug" / exe)
    ap.add_argument("--output", type=Path)
    args = ap.parse_args()
    schema = vd.load(ROOT / "schemas/v1/protocol.schema.json")
    exits = vd.load(ROOT / "schemas/v1/exit-codes.json")
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    work = Path(tempfile.mkdtemp(prefix="cm-cli-outputs-"))
    fx, store = work / "fixture", work / "store"
    for i in range(3):
        (fx / f"dir{i}").mkdir(parents=True)
        for j in range(4):
            (fx / f"dir{i}" / f"file{j}.bin").write_bytes(b"x" * (100 * (i + 1) + j))
    (fx / "中文 名称.txt").write_text("unicode name", encoding="utf-8")
    env = {**os.environ, "CM_STORE_DIR": str(store), "CLEANMANAGER_LANG": "en"}
    results, failures, state = [], [], {}

    def run(name, argv, expect_exit=None, stream=False):
        p = subprocess.run([str(args.bin), *argv], capture_output=True, env=env, timeout=600)
        out = p.stdout
        problems = []
        if out.startswith(b"\xef\xbb\xbf"):
            problems.append("BOM on stdout")
        if b"\x1b" in out:
            problems.append("ANSI escape on stdout")
        try:
            text = out.decode("utf-8")
        except UnicodeDecodeError:
            problems.append("stdout is not UTF-8")
            text = ""
        lines = [line for line in text.split("\n") if line]
        if not stream and len(lines) != 1:
            problems.append(f"{len(lines)} stdout lines, expected exactly one record")
        records = []
        for line in lines:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError as e:
                problems.append(f"not JSON: {e}")
                continue
            records.append(rec)
            if rec.get("kind") in OUTSIDE_V1:
                continue
            errs = sorted(validator.iter_errors(rec), key=lambda e: list(e.path))
            problems.extend(f"{rec.get('kind')}: {'/'.join(map(str, e.path))}: {e.message[:200]}" for e in errs[:3])
            if not errs:
                try:
                    vd.semantics(rec)
                except AssertionError as e:
                    problems.append(f"{rec.get('kind')}: semantic: {e}")
        last = records[-1] if records else {}
        if last.get("kind") == "error":
            want = exits["errors"].get(last.get("code"))
            if want is not None and p.returncode != want:
                problems.append(f"exit {p.returncode} but error {last.get('code')} maps to {want}")
        if expect_exit is not None and p.returncode not in (expect_exit if isinstance(expect_exit, tuple) else (expect_exit,)):
            problems.append(f"exit {p.returncode}, expected {expect_exit}")
        results.append({"name": name, "argv": ["cleanmanager", *argv], "exit": p.returncode, "kinds": [r.get("kind") for r in records],
                        "valid": not problems, "problems": problems})
        if problems:
            failures.append(name)
        return last, records

    run("capabilities", ["capabilities"], 0)
    scan, _ = run("scan", ["scan", "--root", str(fx), "--non-interactive"], 0)
    sid = scan.get("scan_id", "s_missing")
    run("scan jsonl", ["scan", "--root", str(fx), "--format", "jsonl"], 0, stream=True)
    run("scan show", ["scan", "show", sid], 0)
    run("scan wait", ["scan", "wait", sid, "--timeout", "5"], 0)
    run("scan cancel (terminal)", ["scan", "cancel", sid], 0)
    page, _ = run("entries list", ["entries", "list", "--scan", sid, "--limit", "2"], 0)
    if page.get("next_cursor"):
        run("entries list page 2", ["entries", "list", "--scan", sid, "--limit", "2", "--cursor", page["next_cursor"]], 0)
        run("entries list foreign cursor", ["entries", "list", "--scan", sid, "--limit", "3", "--cursor", page["next_cursor"]], 6)
    run("entries list files by name", ["entries", "list", "--scan", sid, "--kind", "file", "--sort", "name_asc", "--limit", "50"], 0)
    run("entries list limit 501", ["entries", "list", "--scan", sid, "--limit", "501"], 2)
    analysis, _ = run("analyze", ["analyze", "--scan", sid], (0, 5))
    if analysis.get("candidate_set_id"):
        run("candidates list", ["candidates", "list", "--set", analysis["candidate_set_id"], "--limit", "10"], 0)
    run("clean list", ["clean", "list"], 0)
    run("clean dry-run", ["clean", "--scan", sid, "--dry-run", "--non-interactive"], 0)
    run("operations list", ["operations", "list", "--limit", "5"], 0)
    run("recovery list", ["recovery", "list"], 0)
    run("protect list", ["protect", "list"], 0)
    run("protect check", ["protect", "check", "--path", str(fx)], 0)
    run("purge roots", ["purge", "roots"], 0)
    run("installer locations", ["installer", "locations"], 0)
    run("doctor", ["doctor"], 0)
    run("doctor export preview", ["doctor", "--export", str(work / "doctor.json")], 0)
    run("completion print", ["completion", "print", "powershell", "--json"], 0)
    run("completion install preview", ["completion", "install", "bash", "--profile", str(work / "bashrc"), "--json"], 0)
    run("status", ["status"], 0)
    run("insights", ["insights"], 0)
    run("startup list", ["startup", "list"], (0, 5))
    run("apps list", ["apps", "list", "--limit", "5"], 0)
    run("unknown command", ["frobnicate"], 2)
    run("self-approval flag", ["plan", "apply", "p_x", "--yes", "1"], 2)
    run("short option", ["plan", "apply", "p_x", "-y"], 2)
    run("missing plan", ["plan", "show", "p_missing"], None)
    assert not (work / "doctor.json").exists() and not (work / "bashrc").exists(), "a preview wrote a file"
    report = {"status": "passed" if not failures else "failed",
              "scope": "Real cleanmanager runs on a temporary synthetic fixture on this machine; records validated against schemas/v1. Not a cleanup test.",
              "binary": str(args.bin.relative_to(ROOT)) if args.bin.is_relative_to(ROOT) else args.bin.name,
              "platform": f"{platform.system()} {platform.release()} {platform.machine()}",
              "outside_v1_schema": sorted(OUTSIDE_V1), "commands": len(results), "failures": failures, "results": results}
    shutil.rmtree(work, ignore_errors=True)
    text = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    print(json.dumps({k: report[k] for k in ("status", "commands", "failures")}, indent=2))
    for r in results:
        if r["problems"]:
            print(r["name"], r["problems"], file=sys.stderr)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
