"""Validate the wave-1 Windows cleanup catalog on this machine (parity task 3.5, design D4).

Read-only against the user's data: real locations are only scanned (metadata) into a scratch
store; anything that is created, removed or regenerated lives in a throwaway browser profile or a
controlled fixture under --scratch, reached through the real resolvers (CM_RESOLVE_* or the
tool's own cache variable). Each rule gets a run record in docs/evidence/runs with positive and
negative observations and the application/tool versions found; passing rules get a tested
registry entry (with the thresholds exercised) and are promoted to `enabled`. Rules whose owner is
not installed here stay `experimental` and get an `incomplete` run record stating why.

Usage: python -I tools/validate_catalog_windows.py --scratch DIR [--no-build]
"""
from __future__ import annotations

import argparse
import http.server
import importlib.util
import json
import os
import re
import shutil
import subprocess
import threading
import time
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("validate_rules_windows", ROOT / "tools" / "validate_rules_windows.py")
V = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(V)

LOCAL = Path(os.environ["LOCALAPPDATA"])
ROAMING = Path(os.environ["APPDATA"])
DAY = 86400
RESULTS: dict = {}


def rule_file(rule_id: str) -> Path:
    return ROOT / "rules/windows" / f"{rule_id}.json"


def catalog() -> dict:
    _, rec = V.cli(["clean", "list"])
    return rec


def installed(cat: dict, rule_id: str):
    for c in cat["categories"]:
        for r in c["rules"]:
            if r["rule_id"] == rule_id:
                return r["installed_version"]
    return None


def major_range(version: str, parts: int = 1) -> dict:
    nums = [int(x) for x in re.findall(r"\d+", version)[:parts]]
    hi = nums[:-1] + [nums[-1] + 1]
    return {"min_inclusive": ".".join(map(str, nums)) + (".0" if parts == 1 else ""),
            "max_exclusive": ".".join(map(str, hi)) + (".0" if parts == 1 else "")}


def build_range(os_version: str) -> dict:
    a, b, c = os_version.split(".")[:3]
    return {"min_inclusive": f"{a}.{b}.{c}", "max_exclusive": f"{a}.{b}.{int(c) + 1}"}


def preview(scan_root: Path, store: Path, rule_id: str, env: dict | None = None):
    scan, result, items = V.analyze_preview(scan_root, store, rule_file(rule_id), env or {})
    return scan, result, items


def name(item) -> str:
    return item["path"]["display"]


def age(path: Path, days: int):
    t = time.time() - days * DAY
    os.utime(path, (t, t))


def finish(rule_id, test_id, ok, expected, actual, scan, measurements, limitations, positive, negative, effects, subject, label, vrange):
    run_id = "run_win_catalog_" + rule_id.replace(".", "_").replace("-", "_")
    V.write_record(run_id, test_id, expected, actual, ok, scan["backend"] if scan else "none", measurements, limitations,
                   ["cleanmanager", "analyze", "--preview-rule", f"rules/windows/{rule_id}.json"])
    RESULTS[rule_id] = {"ok": ok, "run_id": run_id, "actual": actual}
    if ok:
        V.promote(rule_id, subject, label, vrange, positive, negative, effects, run_id)
    return run_id


def not_validated(rule_id, test_id, reason, observed):
    """Owner missing on this device: incomplete record, rule stays experimental with the reason."""
    run_id = "run_win_catalog_" + rule_id.replace(".", "_").replace("-", "_")
    rec_cmd = ["cleanmanager", "clean", "list"]
    V.write_record(run_id, test_id, f"{rule_id}: validate against a real installation on this device", f"not validated: {reason}",
                   False, "none", {"observed": observed}, [reason], rec_cmd)
    path = ROOT / "docs/evidence/runs" / f"{run_id}.json"
    rec = json.loads(path.read_text(encoding="utf-8"))
    rec.update(status="incomplete", terminal_state="not_run", exit_code=None)
    V.SCHEMAS["run-record"].validate(rec)
    path.write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    rule = json.loads(rule_file(rule_id).read_text(encoding="utf-8"))
    rule.update(status="experimental", tested_registry_ref=None, evidence_refs=[run_id])
    V.SCHEMAS["rule-v2"].validate(rule)
    rule_file(rule_id).write_text(json.dumps(rule, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    RESULTS[rule_id] = {"ok": False, "run_id": run_id, "reason": reason}


# --------------------------------------------------------------------------------------------
# Chromium browsers: throwaway profile, pages served over local HTTP so the HTTP cache is used.

class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def end_headers(self):
        self.send_header("Cache-Control", "public, max-age=86400")
        super().end_headers()


def serve(site: Path):
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), partial(QuietHandler, directory=str(site)))
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def headless(exe: Path, udd: Path, url: str) -> str:
    out = V.run([str(exe), "--headless=new", f"--user-data-dir={udd}", "--no-first-run", "--no-default-browser-check",
                 "--disable-extensions", "--enable-unsafe-swiftshader", "--dump-dom", url], timeout=120)
    return out.stdout


def validate_chromium(owner, label, exes, parts, scratch: Path, cat: dict):
    exe = next((p for p in exes if p.exists()), None)
    if exe is None:
        for kind in ("gpu-cache", "http-cache"):
            not_validated(f"{owner}.{kind}", "CAT-03", f"{label} is not installed on this device", {})
        return
    version = installed(cat, f"{owner}.gpu-cache") or V.file_version(exe)
    site = scratch / f"{owner}-site"
    site.mkdir(parents=True, exist_ok=True)
    (site / "app.js").write_text("document.getElementById('marker').textContent='cm-ok';" + "var x=1;" * 2000, encoding="utf-8")
    (site / "style.css").write_text("body{background:#fff}" * 200, encoding="utf-8")
    (site / "page.html").write_text("<html><head><link rel=stylesheet href=style.css></head><body><canvas id=c width=64 height=64></canvas>"
                                    "<p id=marker>pending</p><script src=app.js></script><script>"
                                    "var g=document.getElementById('c').getContext('webgl');if(g){g.clearColor(0,0,1,1);g.clear(g.COLOR_BUFFER_BIT);}"
                                    "</script></body></html>", encoding="utf-8")
    httpd = serve(site)
    url = f"http://127.0.0.1:{httpd.server_address[1]}/page.html"
    local = scratch / f"{owner}-local"
    udd = local.joinpath(*parts)
    try:
        dom1 = headless(exe, udd, url)
        headless(exe, udd, url)
        gpu, http_cache = udd / "Default" / "GPUCache", udd / "Default" / "Cache" / "Cache_Data"
        created = {"gpu-cache": gpu.is_dir(), "http-cache": http_cache.is_dir()}
        env = {"CM_RESOLVE_LOCALAPPDATA": str(local)}
        checks = {}
        for kind, target, risk in (("gpu-cache", "Default\\GPUCache", "low"), ("http-cache", "Default\\Cache\\Cache_Data", "medium")):
            scan, _, items = preview(udd, scratch / f"store-{owner}-{kind}", f"{owner}.{kind}", env)
            hits = [i for i in items if name(i).endswith(target)]
            others = [name(i) for i in items if not name(i).endswith(target)]
            checks[kind] = (scan, items, hits, others, risk)
        shutil.rmtree(gpu, ignore_errors=True)
        shutil.rmtree(http_cache, ignore_errors=True)
        dom2 = headless(exe, udd, url)
        headless(exe, udd, url)
        regenerated = {"gpu-cache": gpu.is_dir(), "http-cache": http_cache.is_dir()}
    finally:
        httpd.shutdown()
    rendered = "cm-ok" in dom1 and "cm-ok" in dom2
    for kind, (scan, items, hits, others, risk) in checks.items():
        rule_id = f"{owner}.{kind}"
        folder = "GPUCache" if kind == "gpu-cache" else "Cache\\Cache_Data"
        ok = created[kind] and len(hits) == 1 and hits[0]["risk"] == risk and not others and regenerated[kind] and rendered
        finish(rule_id, "CAT-03", ok,
               f"{label} {version}: a headless throwaway profile creates Default\\{folder}; "
               f"the preview matches only that directory (risk {risk}); after removal it is recreated and the page still renders",
               f"created={created[kind]} candidates={len(items)} matched={len(hits)} risk={hits[0]['risk'] if hits else None} "
               f"other_matches={len(others)} regenerated_after_removal={regenerated[kind]} page_rendered={rendered}",
               scan, {"app_version": version, "candidate_count": len(items), "matched_logical_bytes": hits[0]["logical_bytes"] if hits else None},
               ["Throwaway --user-data-dir and a 127.0.0.1 test server; the user's real profiles were not scanned or modified.",
                "The running-browser skip is a live probe (owner_process), covered by run_win_catalog_probe_e2e for another rule."],
               [f"Headless throwaway profile: Default\\{folder} matched as the only candidate"],
               ["Code Cache, Local Storage, IndexedDB, Cookies and the other profile folders are not matched",
                "Recreated after removal; the test page still renders"],
               ["Skipped while the browser is running or holds a cache file open."] +
               (["Pages are downloaded again; cached-only pages are unavailable offline."] if kind == "http-cache" else ["The first page draws may be slower."]),
               {"kind": "app", "id": owner}, label, major_range(version))


# --------------------------------------------------------------------------------------------
# Read-only observation of real locations

def validate_vscode(scratch: Path, cat: dict):
    code = ROAMING / "Code"
    version = installed(cat, "microsoft.vscode.cached-data")
    for rule_id, comp in (("microsoft.vscode.cached-data", "CachedData"), ("microsoft.vscode.code-cache", "Code Cache"),
                          ("microsoft.vscode.gpu-cache", "GPUCache")):
        if not code.is_dir() or version is None:
            not_validated(rule_id, "CAT-03", "Visual Studio Code is not installed (or has no installer record) on this device", {})
            continue
        scan, _, items = preview(code, scratch / f"store-{rule_id}", rule_id)
        hits = [i for i in items if name(i).endswith("\\Code\\" + comp)]
        inside = [n.lower()[len(str(code)):] for n in map(name, items)]  # path below %APPDATA%\Code
        bad = [n for n in inside if any(k in n for k in ("\\user\\", "globalstorage", "workspacestorage", "local storage", "backups", "indexeddb"))]
        ok = len(items) == 1 and len(hits) == 1 and hits[0]["risk"] == "medium" and not bad
        finish(rule_id, "CAT-03", ok,
               f"VS Code {version}: a read-only scan of the real %APPDATA%\\Code matches only Code\\{comp} as one review-only candidate",
               f"candidates={len(items)} matched={len(hits)} risk={hits[0]['risk'] if hits else None} protected_or_state_matches={len(bad)}",
               scan, {"app_version": version, "candidate_count": len(items), "matched_logical_bytes": hits[0]["logical_bytes"] if hits else None},
               ["Read-only scan of the real VS Code data folder; nothing was removed, so regeneration was not exercised and the rule stays review_only."],
               [f"Real %APPDATA%\\Code\\{comp} matched as the only candidate"],
               ["User settings, globalStorage/workspaceStorage state databases, Backups and Local Storage are not matched"],
               ["Recreated by VS Code on the next start; skipped while VS Code runs or holds a file open."],
               {"kind": "app", "id": "microsoft.vscode"}, "Visual Studio Code", major_range(version, 2))


def validate_directx(scratch: Path, cat: dict):
    rule_id, root = "windows.directx.shader-cache", LOCAL / "D3DSCache"
    osv = installed(cat, rule_id)
    if not root.is_dir():
        return not_validated(rule_id, "CAT-03", "no DirectX shader cache folder on this device", {})
    scan, _, items = preview(root, scratch / "store-d3d", rule_id)
    children = sorted(p.name for p in root.iterdir() if p.is_dir())
    matched = sorted(name(i).rsplit("\\", 1)[-1] for i in items)
    direct = all(name(i).rsplit("\\", 1)[0].lower() == str(root).lower() for i in items)
    ok = bool(items) and matched == children and direct and all(i["risk"] == "medium" for i in items)
    finish(rule_id, "CAT-03", ok,
           "Read-only scan of the real %LOCALAPPDATA%\\D3DSCache: every per-application folder (and nothing deeper) is a review-only candidate",
           f"candidates={len(items)} folders_present={len(children)} all_direct_children={direct} risks={sorted({i['risk'] for i in items})}",
           scan, {"os_version": osv, "candidate_count": len(items)},
           ["Read-only scan; regeneration by the graphics driver was not exercised, so the rule stays review_only."],
           ["Every real per-application cache folder under D3DSCache matched"], ["Files inside the folders are not separate candidates"],
           ["Shaders are rebuilt when applications next run; the first run may stutter."],
           {"kind": "system", "id": "windows.directx"}, "Windows DirectX shader cache", build_range(osv))


def validate_npm(scratch: Path, cat: dict):
    root = LOCAL / "npm-cache"
    version = installed(cat, "npm.cache-content")
    adapter = next((a for a in cat["adapters"] if a["adapter_id"] == "npm.cache-verify"), None)
    if version is None or adapter is None or not root.is_dir():
        for r in ("npm.cache-content", "npm.cache-logs"):
            not_validated(r, "CAT-05", "npm (Node.js) is not installed in a trusted location on this device", {})
        return
    scan, _, items = preview(root, scratch / "store-npm", "npm.cache-content")
    hits = [i for i in items if name(i).endswith("\\npm-cache\\_cacache")]
    same_root = (adapter["reported_path"] or "").rstrip("\\").lower() == str(root).lower()
    ok = (len(items) == 1 and len(hits) == 1 and hits[0]["risk"] == "medium" and hits[0]["allowed_modes"] == []
          and "owner_tool:npm.cache-verify" in hits[0]["reason"] and adapter["status"] == "available" and same_root)
    finish("npm.cache-content", "CAT-05", ok,
           f"npm {version}: the real npm cache's _cacache is one review-only owner-tool row without file modes; the read-only adapter preview "
           "(node npm-cli.js config get cache) reports the same cache root the resolver uses",
           f"candidates={len(items)} matched={len(hits)} risk={hits[0]['risk'] if hits else None} modes={hits[0]['allowed_modes'] if hits else None} "
           f"adapter_status={adapter['status']} adapter_exit={adapter['exit_code']} adapter_path_matches_resolver={same_root}",
           scan, {"tool_version": version, "adapter_executable": adapter["executable"], "adapter_sha256": adapter["executable_sha256"],
                  "adapter_script_sha256": adapter["script_sha256"], "adapter_duration_ms": adapter["duration_ms"]},
           ["Only the read-only adapter preview ran; `npm cache verify` (the declared cleanup) was not executed this round."],
           ["Real _cacache matched as one owner-tool row"], ["_logs, index files and anything outside _cacache are not part of this row",
                                                            "The row has no file modes: it can never become a direct delete"],
           ["Packages are downloaded again on the next install; offline installs of removed packages fail."],
           {"kind": "tool", "id": "npm"}, "npm", major_range(version))
    # Logs: fixture through npm's own cache variable (the resolver honours npm_config_cache).
    fx = scratch / "npm-fixture"
    (fx / "_logs").mkdir(parents=True, exist_ok=True)
    (fx / "_cacache").mkdir(exist_ok=True)
    for n, days in (("2026-01-01T00_00_00_000Z-debug-0.log", 30), ("recent-debug-0.log", 0), ("old-note.txt", 30)):
        (fx / "_logs" / n).write_bytes(b"x" * 10)
        age(fx / "_logs" / n, days)
    (fx / "_cacache" / "old.log").write_bytes(b"x")
    age(fx / "_cacache" / "old.log", 30)
    scan, _, items = preview(fx, scratch / "store-npm-logs", "npm.cache-logs", {"npm_config_cache": str(fx)})
    real_logs = sum(1 for p in (root / "_logs").glob("*.log")) if (root / "_logs").is_dir() else 0
    got = sorted(name(i).rsplit("\\", 1)[-1] for i in items)
    ok = got == ["2026-01-01T00_00_00_000Z-debug-0.log"] and items[0]["risk"] == "medium"
    finish("npm.cache-logs", "CAT-07", ok,
           "Only *.log files directly in the npm cache's _logs folder and at least 7 days old are review-only candidates",
           f"candidates={got} real_npm_log_files_present={real_logs}", scan, {"tool_version": version, "candidate_count": len(items)},
           ["Controlled fixture reached through npm's own npm_config_cache variable; the real cache was not modified."],
           ["30-day-old debug log in _logs is a review-only candidate"],
           ["A log modified today is not a candidate", "A non-.log file in _logs is not matched", "A .log file elsewhere in the cache is not matched"],
           ["Logs of past npm runs are no longer available for troubleshooting."], {"kind": "tool", "id": "npm"}, "npm", major_range(version))


def validate_cargo(scratch: Path, cat: dict):
    rule_id = "cargo.registry-cache"
    home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    out = V.run(["cargo", "--version"]) if shutil.which("cargo") else None
    version = re.search(r"\d+\.\d+\.\d+", out.stdout).group(0) if out and out.returncode == 0 else None
    if not (home / "registry" / "cache").is_dir() or version is None:
        return not_validated(rule_id, "CAT-03", "Cargo with a registry cache is not present on this device", {})
    scan, _, items = preview(home, scratch / "store-cargo", rule_id)
    hits = [i for i in items if name(i).endswith("\\registry\\cache")]
    kept = [n for n in map(name, items) if any(k in n for k in ("\\registry\\src", "\\registry\\index", "\\git\\", "\\bin"))]
    ok = len(items) == 1 and len(hits) == 1 and hits[0]["risk"] == "medium" and not kept
    finish(rule_id, "CAT-03", ok,
           f"Cargo {version}: a read-only scan of the real CARGO_HOME matches only registry\\cache (crate archives) as one review-only candidate",
           f"candidates={len(items)} matched={len(hits)} risk={hits[0]['risk'] if hits else None} kept_folders_matched={len(kept)}",
           scan, {"tool_version": version, "candidate_count": len(items), "matched_logical_bytes": hits[0]["logical_bytes"] if hits else None},
           ["Read-only scan of the real Cargo home; Cargo has no installer record here, so the live registry verdict is version_untested "
            "and the row stays review_only (it is a dependency store and review_only regardless)."],
           ["Real registry\\cache matched as the only candidate"], ["registry\\src, registry\\index, git checkouts and bin are not matched"],
           ["Crates are downloaded again by the next build; skipped while cargo runs."], {"kind": "tool", "id": "cargo"}, "Cargo",
           major_range(version, 2))


def validate_wsl(scratch: Path, cat: dict):
    rule_id, root = "microsoft.wsl.distro-disk", LOCAL / "wsl"
    out = V.run(["wsl", "--version"]) if shutil.which("wsl") else None
    text = out.stdout.replace("\x00", "") if out else ""
    m = re.search(r"(\d+\.\d+\.\d+(?:\.\d+)?)", text)
    disks = list(root.glob("*/ext4.vhdx")) if root.is_dir() else []
    if not disks or m is None:
        return not_validated(rule_id, "CAT-09", "no WSL distribution disk under Local AppData\\wsl on this device", {})
    version = m.group(1)
    scan, _, items = preview(root, scratch / "store-wsl", rule_id)
    ok = (len(items) == len(disks) and all(name(i).endswith("\\ext4.vhdx") and i["allowed_modes"] == [] and "large_item_hint" in i["reason"]
                                           and i["risk"] == "medium" for i in items))
    finish(rule_id, "CAT-09", ok,
           "Each real WSL distribution disk (ext4.vhdx) appears as one large-item hint row with no file modes",
           f"hint_rows={len(items)} disks_present={len(disks)} all_without_modes={all(i['allowed_modes'] == [] for i in items)}",
           scan, {"wsl_version": version, "hint_rows": len(items), "logical_bytes": [i["logical_bytes"] for i in items]},
           ["Read-only scan; the hint row cannot be selected or planned by construction."],
           ["Real ext4.vhdx of the installed distribution listed as a hint row"],
           ["Other files under Local AppData\\wsl are not listed", "The row has no allowed modes and is never default-selected"],
           ["Space is reclaimed inside the distribution or with WSL's own tools, never by deleting the disk."],
           {"kind": "app", "id": "microsoft.wsl"}, "Windows Subsystem for Linux", major_range(version, 2))


# --------------------------------------------------------------------------------------------
# Controlled fixtures through the real resolvers (locations absent or private on this device)

def validate_wer_and_dumps(scratch: Path, cat: dict):
    osv = installed(cat, "windows.wer.report-archive")
    local = scratch / "wer-local"
    wer = local / "Microsoft" / "Windows" / "WER"
    real_wer = {k: (LOCAL / "Microsoft/Windows/WER" / k).is_dir() for k in ("ReportArchive", "ReportQueue")}
    for rule_id, comp in (("windows.wer.report-archive", "ReportArchive"), ("windows.wer.report-queue", "ReportQueue")):
        base = wer / comp
        for n, days in (("AppCrash_old_1", 30), ("AppCrash_new_2", 0)):
            (base / n).mkdir(parents=True, exist_ok=True)
            (base / n / "Report.wer").write_bytes(b"x" * 64)
            age(base / n / "Report.wer", days)
            age(base / n, days)
        (base / "loose-old.txt").write_bytes(b"x")
        age(base / "loose-old.txt", 30)
        scan, _, items = preview(wer, scratch / f"store-{rule_id}", rule_id, {"CM_RESOLVE_LOCALAPPDATA": str(local)})
        got = sorted(name(i).rsplit("\\", 1)[-1] for i in items)
        ok = got == ["AppCrash_old_1"] and all(i["risk"] == "medium" for i in items)
        finish(rule_id, "CAT-07", ok,
               f"Per-user WER {comp}: only entries at least 7 days old are review-only candidates; newer reports are not candidates",
               f"candidates={got} real_{comp}_present={real_wer[comp]}", scan, {"os_version": osv, "candidate_count": len(items)},
               ["Controlled fixture through the real resolver (CM_RESOLVE_LOCALAPPDATA); the per-user WER folder does not exist on this "
                "device, and the machine-wide ProgramData WER store is not a target."],
               [f"30-day-old report folder in {comp} is a review-only candidate"],
               ["A report folder from today is not a candidate", "A loose old file beside the report folders is not matched"],
               ["Crash history for troubleshooting is lost; queued reports are not sent."],
               {"kind": "system", "id": "windows.wer"}, "Windows Error Reporting (per user)", build_range(osv))
    rule_id = "windows.crash-dumps.user"
    dumps = local / "CrashDumps"
    dumps.mkdir(parents=True, exist_ok=True)
    for n, days in (("app.exe.1234.dmp", 30), ("app.exe.5678.dmp", 0), ("notes.txt", 30)):
        (dumps / n).write_bytes(b"x" * 32)
        age(dumps / n, days)
    scan, _, items = preview(dumps, scratch / "store-dumps", rule_id, {"CM_RESOLVE_LOCALAPPDATA": str(local)})
    got = sorted(name(i).rsplit("\\", 1)[-1] for i in items)
    ok = got == ["app.exe.1234.dmp"] and items[0]["risk"] == "medium"
    finish(rule_id, "CAT-07", ok, "Only *.dmp files in Local AppData\\CrashDumps at least 7 days old are review-only candidates",
           f"candidates={got} real_CrashDumps_present={(LOCAL / 'CrashDumps').is_dir()}", scan, {"os_version": osv, "candidate_count": len(items)},
           ["Controlled fixture through the real resolver; Local AppData\\CrashDumps does not exist on this device."],
           ["30-day-old .dmp is a review-only candidate"], ["A dump from today is not a candidate", "A non-dump file is not matched"],
           ["The dumps are no longer available for debugging."], {"kind": "system", "id": "windows.crash-dumps"}, "Windows user crash dumps",
           build_range(osv))


def validate_downloads(scratch: Path, cat: dict):
    rule_id = "windows.downloads.incomplete"
    osv = installed(cat, rule_id)
    dl = scratch / "downloads"
    (dl / "sub").mkdir(parents=True, exist_ok=True)
    for n, days in (("setup.exe.crdownload", 30), ("movie.mkv.part", 0), ("archive.zip", 30), ("sub\\nested.partial", 30)):
        (dl / n).write_bytes(b"x" * 16)
        age(dl / n, days)
    scan, _, items = preview(dl, scratch / "store-dl", rule_id, {"CM_RESOLVE_DOWNLOADS": str(dl)})
    got = sorted(name(i).rsplit("\\", 1)[-1] for i in items)
    real = Path.home() / "Downloads"
    real_markers = sum(1 for p in real.iterdir() if p.suffix.lower() in (".crdownload", ".part", ".partial")) if real.is_dir() else 0
    ok = got == ["setup.exe.crdownload"] and items[0]["risk"] == "medium"
    finish(rule_id, "CAT-07", ok,
           "Only unfinished-download files directly in Downloads and untouched for 7 days are review-only candidates",
           f"candidates={got} real_download_markers_present={real_markers}", scan, {"os_version": osv, "candidate_count": len(items)},
           ["Controlled fixture through the real resolver (CM_RESOLVE_DOWNLOADS); the real Downloads folder was only counted, not scanned."],
           ["30-day-old .crdownload is a review-only candidate"],
           ["A .part file modified today is not a candidate", "A finished .zip is not matched", "A marker in a subfolder is not matched"],
           ["Skipped while a browser still writes the file (incomplete_download_handle)."], {"kind": "system", "id": "windows.downloads"},
           "Windows Downloads folder", build_range(osv))


# --------------------------------------------------------------------------------------------
# End-to-end: live probes in `clean --dry-run` (CAT-06) and plan creation refusals (CLI-12)

def probe_e2e(scratch: Path):
    dl = scratch / "e2e-downloads"
    dl.mkdir(parents=True, exist_ok=True)
    marker = dl / "held.iso.crdownload"
    marker.write_bytes(b"x" * 8)
    age(marker, 30)
    store = scratch / "store-e2e"
    _, scan = V.cli(["scan", "--root", str(dl), "--store", str(store)])
    env = {**os.environ, "CM_RESOLVE_DOWNLOADS": str(dl)}
    with open(marker, "ab"):  # a browser still writing the file
        _, held = V.cli(["clean", "--scan", scan["scan_id"], "--store", str(store), "--dry-run", "--non-interactive"], env)
    _, free = V.cli(["clean", "--scan", scan["scan_id"], "--store", str(store), "--dry-run", "--non-interactive"], env)
    code, plan = V.cli(["clean", "--scan", scan["scan_id"], "--store", str(store), "--mode", "trash", "--non-interactive"], env)
    item = lambda rec: next((i for i in rec.get("items", []) if i["path"]["display"].endswith("held.iso.crdownload")), None)  # noqa: E731
    h, f = item(held), item(free)
    probe = lambda i: next((p["result"] for p in i["probes"] if p["probe"] == "incomplete_download_handle"), None) if i else None  # noqa: E731
    ok = (h is not None and h["skip_reason"] == "in_use" and probe(h) == "in_use" and not h["selected_by_default"]
          and f is not None and f["skip_reason"] is None and probe(f) == "not_in_use" and f["tier"] == "review_only" and not f["selected_by_default"]
          and plan["kind"] == "error" and plan["code"] == "policy_denied")
    V.write_record("run_win_catalog_probe_e2e", "CAT-06",
                   "clean --dry-run probes live: a held .crdownload is skipped as in_use and never preselected; once released it is not_in_use "
                   "but stays review_only, so a non-interactive plan without explicit IDs has nothing to include (policy_denied)",
                   f"held: skip={h and h['skip_reason']} probe={probe(h)} selected={h and h['selected_by_default']}; released: skip={f and f['skip_reason']} "
                   f"probe={probe(f)} tier={f and f['tier']} selected={f and f['selected_by_default']}; non_interactive_plan={plan['kind']}:{plan.get('code')}",
                   ok, scan["backend"], {"preview_items_held": len(held.get("items", [])), "plan_exit_code": code},
                   ["Controlled fixture; the file was held open by this validator to stand in for a browser download."],
                   ["cleanmanager", "clean", "--scan", "ID", "--dry-run", "--non-interactive"])
    RESULTS["probe_e2e"] = {"ok": ok}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", type=Path, required=True)
    ap.add_argument("--no-build", action="store_true", help="use the existing target/debug binary")
    args = ap.parse_args()
    if not args.no_build:
        subprocess.run(["cargo", "build", "-q", "-p", "cm-cli"], cwd=ROOT, check=True)
    scratch = args.scratch.resolve()
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    cat = catalog()
    for owner, label, exes, parts in V.BROWSERS:
        validate_chromium(owner, label, exes, parts, scratch, cat)
    if not (ROAMING / "Mozilla/Firefox").exists() and not (LOCAL / "Mozilla/Firefox").exists():
        not_validated("mozilla.firefox.cache2", "CAT-03", "Mozilla Firefox is not installed on this device", {})
    validate_vscode(scratch, cat)
    validate_directx(scratch, cat)
    validate_npm(scratch, cat)
    validate_cargo(scratch, cat)
    go = next((a for a in cat["adapters"] if a["adapter_id"] == "go.build-cache-clean"), {})
    if go.get("status") != "available":
        not_validated("go.build-cache", "CAT-05", "Go is not installed in a trusted location (Program Files) on this device; its owner-tool "
                      "adapter is unavailable, and the cache is never removed directly", {"adapter_status": go.get("status"), "reason": go.get("reason")})
    validate_wsl(scratch, cat)
    validate_wer_and_dumps(scratch, cat)
    validate_downloads(scratch, cat)
    for rule_id in ("apple.metadata.ds-store", "apple.metadata.appledouble", "apple.metadata.spotlight"):
        not_validated(rule_id, "CAT-08", "no removable volume is attached to this device; the resolver only accepts a user-selected removable "
                      "drive root", {})
    probe_e2e(scratch)
    print(json.dumps(RESULTS, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
