"""Validate the first Windows rule batch on real applications (MVP task 7.4).

Browser rules use the real installed browser with a throwaway --user-data-dir (never the user's
profile): a headless run creates the profile and its Code Cache, the experimental rule is
previewed against a real scan of that profile, the cache is removed and the browser relaunched to
prove it regenerates and the profile still works. The temp rule is checked against a controlled
TEMP fixture through the real resolver. Passing rules get a run record, a non-synthetic tested
registry entry and are promoted to `enabled`; failing rules stay experimental.

Usage: python tools/validate_rules_windows.py --scratch DIR
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".research" / "python-deps"))
from jsonschema import Draft202012Validator, FormatChecker  # noqa: E402

CLI = ROOT / "target" / "debug" / "cleanmanager.exe"
SCHEMAS = {n: Draft202012Validator(json.loads((ROOT / f"schemas/v1/{n}.schema.json").read_text(encoding="utf-8")), format_checker=FormatChecker())
           for n in ("protocol", "run-record", "tested-registry", "rule-v2")}
BROWSERS = [
    ("microsoft.edge", "Microsoft Edge", [Path(os.environ.get("ProgramFiles(x86)", "")) / "Microsoft/Edge/Application/msedge.exe",
                                          Path(os.environ.get("ProgramFiles", "")) / "Microsoft/Edge/Application/msedge.exe"],
     ("Microsoft", "Edge", "User Data")),
    ("google.chrome", "Google Chrome", [Path(os.environ.get("ProgramFiles", "")) / "Google/Chrome/Application/chrome.exe",
                                        Path(os.environ.get("ProgramFiles(x86)", "")) / "Google/Chrome/Application/chrome.exe",
                                        Path(os.environ.get("LOCALAPPDATA", "")) / "Google/Chrome/Application/chrome.exe"],
     ("Google", "Chrome", "User Data")),
]


def utc():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def run(cmd, env=None, timeout=180):
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", env=env, timeout=timeout)


def cli(args, env=None):
    out = run([str(CLI), *args, "--json"], env=env)
    rec = json.loads(out.stdout)
    SCHEMAS["protocol"].validate(rec)
    return out.returncode, rec


def file_version(exe: Path) -> str:
    ps = f"(Get-Item '{exe}').VersionInfo.ProductVersion"
    return run(["powershell", "-NoProfile", "-Command", ps]).stdout.strip()


def os_facts():
    _, version, _, _ = platform.win32_ver()
    build = int(version.split(".")[2])
    return {"platform": "windows", "name": ("Windows 11 " if build >= 22000 else "Windows ") + platform.win32_edition(),
            "build": version, "architecture": "x64" if platform.machine().lower() in ("amd64", "x86_64") else "arm64"}


def headless(exe: Path, udd: Path, page: Path) -> str:
    out = run([str(exe), "--headless=new", f"--user-data-dir={udd}", "--no-first-run", "--no-default-browser-check",
               "--disable-gpu", "--disable-extensions", "--dump-dom", page.as_uri()], timeout=120)
    return out.stdout


def analyze_preview(scan_root: Path, store: Path, rule_file: Path, env_overrides: dict):
    code, scan = cli(["scan", "--root", str(scan_root), "--store", str(store)])
    env = {**os.environ, **env_overrides}
    code2, result = cli(["analyze", "--scan", scan["scan_id"], "--store", str(store), "--preview-rule", str(rule_file)], env)
    _, page = cli(["candidates", "list", "--set", result["candidate_set_id"], "--store", str(store), "--limit", "500"])
    return scan, result, page["items"]


def write_record(run_id, test_id, expected, actual, ok, backend, measurements, limitations, command):
    commit = run(["git", "rev-parse", "HEAD"]).stdout.strip() or None
    rec = {
        "schema_version": "1.0", "test_id": test_id, "run_id": run_id, "recorded_at": utc(), "code_commit": commit,
        "binary_sha256": hashlib.sha256(CLI.read_bytes()).hexdigest(), "device_id": "dev_win11_x64_primary", "os": os_facts(),
        "filesystem": {"type": "NTFS", "volume_id": None},
        "permission_context": {"elevated": True, "signing_identity": None, "grants": ["SeBackupPrivilege/SeRestorePrivilege disabled by cleanmanager"]},
        "scope_digest": None, "fixture_manifest_sha256": None, "backend": backend, "config": {}, "command": command, "seed": None,
        "expected": expected, "actual": actual, "terminal_state": "completed" if ok else "failed", "exit_code": 0 if ok else 1,
        "measurements": measurements, "observer": "tools/validate_rules_windows.py",
        "limitations": ["Working tree may contain uncommitted changes; code_commit is the base commit."] + limitations,
        "status": "passed" if ok else "failed",
    }
    SCHEMAS["run-record"].validate(rec)
    (ROOT / "docs/evidence/runs" / f"{run_id}.json").write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def promote(rule_id, subject, name, version_range, positive, negative, effects, run_id):
    rule_path = ROOT / "rules/windows" / f"{rule_id}.json"
    rule = json.loads(rule_path.read_text(encoding="utf-8"))
    # The registry records the thresholds the run exercised (CAT-07): changing one needs a new rule version.
    reg = {"schema_version": "1.0", "registry_id": f"{rule_id}@1.0.0", "rule_id": rule_id, "rule_version": "1.0.0", "synthetic": False,
           "subject": subject | {"name": name}, "version_range": version_range,
           "platforms": [{"platform": "windows", "os_versions": [os_facts()["build"]], "architectures": [os_facts()["architecture"]]}],
           "tested_at": datetime.now(timezone.utc).strftime("%Y-%m-%d"), "result": "passed",
           "fixtures": {"positive": positive, "negative": negative}, "known_effects": effects, "evidence_refs": [run_id],
           "thresholds": rule["thresholds"]}
    SCHEMAS["tested-registry"].validate(reg)
    (ROOT / "rules/registry").mkdir(exist_ok=True)
    (ROOT / "rules/registry" / f"{rule_id}@1.0.0.json").write_text(json.dumps(reg, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    rule.update(status="enabled", tested_registry_ref=f"{rule_id}@1.0.0", evidence_refs=[run_id])
    SCHEMAS["rule-v2"].validate(rule)
    rule_path.write_text(json.dumps(rule, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def validate_browser(owner, name, candidates, parts, scratch: Path, results: dict):
    exe = next((p for p in candidates if p.exists()), None)
    if exe is None:
        results[owner] = "skipped: not installed"
        return
    version = file_version(exe)
    local = scratch / f"{owner}-local"
    udd = local.joinpath(*parts)
    page = scratch / "page.html"
    page.write_text("<html><body><script>" + "function f(n){let s=0;for(let i=0;i<n;i++){s+=i%7}return s}" * 50
                    + "document.body.setAttribute('data-ok', String(f(100000)));</script><p id=marker>cm-ok</p></body></html>", encoding="utf-8")
    dom1 = headless(exe, udd, page)
    headless(exe, udd, page)  # second load lets the code cache populate
    cache = udd / "Default" / "Code Cache"
    created = cache.is_dir()
    rule_file = ROOT / "rules/windows" / f"{owner}.code-cache.json"
    scan, result, items = analyze_preview(udd, scratch / "store", rule_file, {"CM_RESOLVE_LOCALAPPDATA": str(local)})
    paths = [i["path"]["display"] for i in items]
    matched = [i for i in items if i["path"]["display"].endswith("Default\\Code Cache")]
    positive = created and len(matched) == 1 and matched[0]["risk"] == "low" and "experimental_preview" in matched[0]["reason"]
    negative = len(items) == 1 and not any(p.endswith(("\\Cache", "\\GPUCache", "\\Service Worker", "\\Local Storage")) for p in paths)
    shutil.rmtree(cache)
    dom2 = headless(exe, udd, page)
    headless(exe, udd, page)
    regenerated = cache.is_dir() and "cm-ok" in dom2 and "cm-ok" in dom1
    ok = positive and negative and regenerated
    run_id = f"run_win_rule_{owner.replace('.', '_')}_code_cache"
    write_record(run_id, "ANA-01",
        f"{name} {version}: headless throwaway profile creates Default\\Code Cache; preview matches only that directory (risk low); after removal the cache is recreated and the page still renders",
        f"code_cache_created={created} candidates={len(items)} matched={len(matched)} risk={matched[0]['risk'] if matched else None} regenerated_after_removal={regenerated} page_rendered={'cm-ok' in dom2}",
        ok, scan["backend"], {"app_version": version, "candidate_count": len(items), "code_cache_bytes": matched[0]["logical_bytes"] if matched else None},
        ["Throwaway --user-data-dir; the user's real profile was not scanned or modified.", "Running-browser skip is an execution-time probe (owner_process), not covered here."],
        ["cleanmanager", "analyze", "--preview-rule", f"rules/windows/{owner}.code-cache.json"])
    results[owner] = {"version": version, "ok": ok, "created": created, "matched": len(matched), "candidates": len(items), "regenerated": regenerated}
    if ok:
        major = version.split(".")[0]
        promote(f"{owner}.code-cache", {"kind": "app", "id": owner}, name, {"min_inclusive": f"{major}.0", "max_exclusive": f"{int(major) + 1}.0"},
                ["Headless throwaway profile: Default\\Code Cache created and matched as the only candidate"],
                ["Cache, GPUCache, Service Worker and Local Storage are not matched", "Code Cache recreated after removal; page still renders"],
                ["Pages run script slightly slower until the cache is rebuilt.", "Skipped at execution while the browser is running."], run_id)


def validate_temp(scratch: Path, results: dict):
    temp = scratch / "temp-fixture"
    (temp / "sub").mkdir(parents=True)
    old = time.time() - 30 * 86400
    for n in ("old.tmp", "cache.db"):
        (temp / n).write_bytes(b"x" * 10)
        os.utime(temp / n, (old, old))
    (temp / "new.tmp").write_bytes(b"y")
    (temp / "sub" / "nested-old.tmp").write_bytes(b"z")
    os.utime(temp / "sub" / "nested-old.tmp", (old, old))
    rule_file = ROOT / "rules/windows/windows.user-temp.stale-files.json"
    scan, result, items = analyze_preview(temp, scratch / "store", rule_file, {"CM_RESOLVE_TEMP": str(temp)})
    by = {i["path"]["display"].rsplit("\\", 1)[-1]: i for i in items}
    ok = (set(by) == {"old.tmp", "cache.db"} and by["old.tmp"]["risk"] == "medium" and by["cache.db"]["risk"] == "blocked"
          and by["cache.db"]["allowed_modes"] == [])
    run_id = "run_win_rule_user_temp_stale"
    write_record(run_id, "ANA-02",
        "Only top-level files older than 7 days are candidates; old.tmp is review-only, an old database is blocked, new and nested files are not candidates",
        f"candidates={sorted(by)} old.tmp.risk={by.get('old.tmp', {}).get('risk')} cache.db.risk={by.get('cache.db', {}).get('risk')}",
        ok, scan["backend"], {"candidate_count": len(items)},
        ["Controlled TEMP fixture through the real resolver override; the user's real TEMP was not scanned."],
        ["cleanmanager", "analyze", "--preview-rule", "rules/windows/windows.user-temp.stale-files.json"])
    results["windows.user-temp"] = {"ok": ok, "candidates": sorted(by)}
    if ok:
        promote("windows.user-temp.stale-files", {"kind": "system", "id": "windows"}, "Windows user temporary folder",
                {"min_inclusive": "10.0.22000", "max_exclusive": None},
                ["30-day-old top-level file is a review-only candidate"],
                ["File modified within 7 days is not a candidate", "Old *.db file is blocked as a database", "Nested files are not matched (max_depth 0)"],
                ["Files are owned by whichever program wrote them; review before removing.", "Files in use are skipped at execution."], run_id)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", type=Path, required=True)
    args = ap.parse_args()
    subprocess.run(["cargo", "build", "-q", "-p", "cm-cli"], cwd=ROOT, check=True)
    scratch = args.scratch.resolve()
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    results = {}
    for owner, name, exes, parts in BROWSERS:
        validate_browser(owner, name, exes, parts, scratch, results)
    validate_temp(scratch, results)
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
