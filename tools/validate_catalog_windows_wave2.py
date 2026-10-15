"""Validate catalog wave 2 on this Windows machine (parity task 3.8, design D4).

Wave 2 covers communication, media/games, design, cloud and office, virtualization, emulator
runtimes, package-manager caches and administrator-only system logs. Same acceptance as wave 1
(task 3.5): an enabled rule has a tested-registry entry plus a run record with positive and
negative observations on this device; anything that cannot be observed here stays experimental.

Strictly read-only against the user's data. Real locations are only *scanned* (metadata into a
scratch store) and previewed with `analyze --preview-rule`; nothing real is deleted, moved,
trashed or planned, and no owner-tool cleanup command is run (only the adapters' read-only
previews that `clean list` performs). Positive age-threshold cases and all lookalike negatives use
controlled fixtures under --scratch, reached through the real resolvers via CM_RESOLVE_* overrides.

The administrator-only rules are also exercised end to end with `clean --dry-run` twice on a
fixture: once with this (elevated) process and once under a restricted token
(`runas /trustlevel:0x20000`, i.e. elevation refused); the second run must skip the rows.

Usage: python -I tools/validate_catalog_windows_wave2.py --scratch DIR [--cli PATH]
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("validate_catalog_windows", ROOT / "tools" / "validate_catalog_windows.py")
C = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(C)
V = C.V  # shared helpers (CLI runner, schemas, promote)

OBSERVER = "tools/validate_catalog_windows_wave2.py"
LOCAL = Path(os.environ["LOCALAPPDATA"])
ROAMING = Path(os.environ["APPDATA"])
PROGRAM_DATA = Path(os.environ.get("ProgramData", r"C:\ProgramData"))
HOME = Path.home()
DAY = 86400
RESULTS: dict = {}
NOW = time.time()


# --------------------------------------------------------------------------------------------
# Records

def record(run_id, test_id, expected, actual, ok, backend, measurements, limitations, command, elevated=True, status=None):
    commit = V.run(["git", "rev-parse", "HEAD"]).stdout.strip() or None
    rec = {
        "schema_version": "1.0", "test_id": test_id, "run_id": run_id, "recorded_at": V.utc(), "code_commit": commit,
        "binary_sha256": hashlib.sha256(V.CLI.read_bytes()).hexdigest(), "device_id": "dev_win11_x64_primary", "os": V.os_facts(),
        "filesystem": {"type": "NTFS", "volume_id": None},
        "permission_context": {"elevated": elevated, "signing_identity": None,
                               "grants": ["SeBackupPrivilege/SeRestorePrivilege disabled by cleanmanager"] if elevated
                               else ["restricted token (runas /trustlevel:0x20000): Administrators deny-only"]},
        "scope_digest": None, "fixture_manifest_sha256": None, "backend": backend, "config": {}, "command": command, "seed": None,
        "expected": expected, "actual": actual,
        "terminal_state": "completed" if ok else ("not_run" if status == "incomplete" else "failed"),
        "exit_code": 0 if ok else (None if status == "incomplete" else 1),
        "measurements": measurements, "observer": OBSERVER,
        "limitations": ["Working tree may contain uncommitted changes; code_commit is the base commit."] + limitations,
        "status": status or ("passed" if ok else "failed"),
    }
    V.SCHEMAS["run-record"].validate(rec)
    (ROOT / "docs/evidence/runs" / f"{run_id}.json").write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def run_id_of(rule_id: str) -> str:
    return "run_win_catalog_" + rule_id.replace(".", "_").replace("-", "_")


def rule_path(rule_id: str) -> Path:
    return ROOT / "rules/windows" / f"{rule_id}.json"


def load_rule(rule_id: str) -> dict:
    return json.loads(rule_path(rule_id).read_text(encoding="utf-8"))


def keep_experimental(rule_id, test_id, reason, observed):
    """Owner missing or not observable here: incomplete record; the rule stays experimental."""
    rid = run_id_of(rule_id)
    record(rid, test_id, f"{rule_id}: validate against a real installation on this device", f"not validated: {reason}", False, "none",
           {"observed": observed}, [reason], ["cleanmanager", "clean", "list"], status="incomplete")
    rule = load_rule(rule_id)
    rule.update(status="experimental", tested_registry_ref=None, evidence_refs=[rid])
    V.SCHEMAS["rule-v2"].validate(rule)
    rule_path(rule_id).write_text(json.dumps(rule, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    RESULTS[rule_id] = {"ok": False, "status": "experimental", "run_id": rid, "reason": reason}


# --------------------------------------------------------------------------------------------
# Independent oracle: what a rule should match under a base, computed with plain os calls.

def oracle(base: Path, rule: dict, now: float = NOW) -> list[str]:
    m = rule["match"]
    start = base.joinpath(*m["relative_components"])
    if not start.exists() or start.is_symlink():
        return []
    want_dir = {"directory": True, "file": False}.get(m["object_type"])

    def type_ok(p: Path) -> bool:
        return want_dir is None or p.is_dir() == want_dir

    def old_enough(p: Path) -> bool:
        days = rule["thresholds"]["min_age_days"]
        return days is None or now - p.stat().st_mtime >= days * DAY

    if m["name_glob"] is None and not m["extensions"]:
        hits = [start] if type_ok(start) else []
    else:
        hits, stack = [], [(start, 0)]
        while stack:
            d, depth = stack.pop()
            for c in d.iterdir():
                if c.is_symlink():
                    continue
                name = c.name.lower()
                glob_ok = m["name_glob"] is None or fnmatch.fnmatchcase(name, m["name_glob"].lower())
                ext_ok = not m["extensions"] or ("." in name and name.rsplit(".", 1)[1] in m["extensions"])
                if glob_ok and ext_ok and type_ok(c):
                    hits.append(c)
                elif c.is_dir() and depth < m["max_depth"]:
                    stack.append((c, depth + 1))
    return sorted(str(p.relative_to(base)).lower() for p in hits if old_enough(p))


def got_rel(items, base: Path) -> list[str]:
    b = str(base).lower().rstrip("\\") + "\\"
    out = []
    for i in items:
        d = i["path"]["display"].lower()
        out.append(d[len(b):] if d.startswith(b) else "!outside:" + d)
    return sorted(out)


def age(p: Path, days: float):
    t = NOW - days * DAY
    os.utime(p, (t, t))


def build(base: Path, spec: dict[str, float | None]):
    """spec: relative path -> age in days (None = directory with default time). Paths ending in
    '/' are directories. Ages are applied deepest first so directory times stick."""
    for rel in spec:
        p = base / rel.rstrip("/")
        if rel.endswith("/"):
            p.mkdir(parents=True, exist_ok=True)
        else:
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b"x" * 64)
    for rel in sorted(spec, key=lambda r: -r.count("/")):
        if spec[rel] is not None:
            age(base / rel.rstrip("/"), spec[rel])


def preview(scan_root: Path, store: Path, rule_id: str, env: dict | None = None):
    return V.analyze_preview(scan_root, store, rule_path(rule_id), env or {})


# --------------------------------------------------------------------------------------------
# One rule: real read-only observation (when present) + controlled lookalike fixture.

def check(rule_id, test_id, *, scratch, label, subject, version, vrange, real_base, fixture_base, fixture_env, fixture_spec,
          fixture_expected, positive, negative, effects, limitations, real_must_match=True, real_note="", fixture_root=None):
    rule = load_rule(rule_id)
    if version is None:
        return keep_experimental(rule_id, test_id, f"{label}: no installed version could be determined on this device", {})
    measurements = {"owner_version": version}
    real_ok, real_desc = True, "real location absent"
    scan = None
    if real_base is not None and real_base.exists():
        expect = oracle(real_base, rule)
        scan, _, items = preview(real_base, scratch / f"store-real-{rule_id}", rule_id)
        got = got_rel(items, real_base)
        risks = sorted({i["risk"] for i in items})
        real_ok = got == expect and all(r == "medium" for r in risks) and (bool(expect) or not real_must_match)
        real_desc = f"real: candidates={len(items)} expected_by_oracle={len(expect)} match={got == expect} risks={risks}"
        measurements.update(real_candidates=got, real_expected=expect,
                            real_matched_logical_bytes=str(sum(int(i["logical_bytes"] or 0) for i in items)))
    elif real_must_match:
        real_ok, real_desc = False, "real location absent although the owner is installed"
    build(fixture_root or fixture_base, fixture_spec)
    fx_scan, _, fx_items = preview(fixture_base, scratch / f"store-fx-{rule_id}", rule_id, fixture_env)
    fx_got = got_rel(fx_items, fixture_base)
    fx_oracle = oracle(fixture_base, rule)
    want = sorted(x.lower() for x in fixture_expected)
    fx_ok = fx_got == want == fx_oracle and all(i["risk"] == "medium" for i in fx_items)
    measurements.update(fixture_candidates=fx_got, fixture_expected=want)
    ok = real_ok and fx_ok
    rid = run_id_of(rule_id)
    thresholds = rule["thresholds"]
    th = f" (min age {thresholds['min_age_days']} days, modified time)" if thresholds["min_age_days"] else ""
    record(rid, test_id,
           f"{label} {version}: the rule matches exactly the oracle set in the real location (read-only scan){th}, every row review-only; "
           f"in a lookalike fixture through the real resolver only {want} match",
           f"{real_desc}; fixture: candidates={fx_got} expected={want} oracle_agrees={fx_oracle == want}{real_note}",
           ok, (scan or fx_scan)["backend"], measurements,
           ["Real location scanned read-only into a scratch store; nothing was removed, so regeneration was not exercised and the rule stays review_only.",
            "Fixture reached through CM_RESOLVE_* overrides; the resolver_override issue marks those results as fixture-based."] + limitations,
           ["cleanmanager", "analyze", "--preview-rule", f"rules/windows/{rule_id}.json"])
    RESULTS[rule_id] = {"ok": ok, "status": "enabled" if ok else "experimental", "run_id": rid, "real": real_desc, "fixture": fx_got}
    if ok:
        V.promote(rule_id, subject, label, vrange, positive, negative, effects, rid)
        # promote() writes the shared observer-less registry; nothing else to adjust.
    return ok


def rng(version: str, parts: int = 2) -> dict:
    nums = [int(x) for x in re.findall(r"\d+", version)[:parts]]
    hi = nums[:-1] + [nums[-1] + 1]
    return {"min_inclusive": ".".join(map(str, nums)), "max_exclusive": ".".join(map(str, hi))}


def build_range(osv: str) -> dict:
    a, b, c = osv.split(".")[:3]
    return {"min_inclusive": f"{a}.{b}.{c}", "max_exclusive": f"{a}.{b}.{int(c) + 1}"}


def tool_version(cmd: list[str], pattern=r"\d+\.\d+(?:\.\d+)*") -> str | None:
    try:
        out = V.run(cmd, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    m = re.search(pattern, (out.stdout or "") + (out.stderr or ""))
    return m.group(0) if out.returncode == 0 and m else None


# --------------------------------------------------------------------------------------------

def validate_installed(scratch: Path, cat: dict):
    inst = lambda rid: C.installed(cat, rid)  # noqa: E731
    roaming, local, pd = scratch / "roaming", scratch / "local", scratch / "programdata"
    env_r = {"CM_RESOLVE_APPDATA": str(roaming)}
    env_l = {"CM_RESOLVE_LOCALAPPDATA": str(local)}
    env_p = {"CM_RESOLVE_PROGRAMDATA": str(pd)}
    osv = inst("windows.wer.machine-report-archive")

    # Feishu (Lark) -----------------------------------------------------------------------
    v = inst("bytedance.feishu.code-cache")
    lark, lark_fx = ROAMING / "LarkShell", roaming / "LarkShell"
    lark_spec = {"CodeCache/14.3.127.23/1/x.bin": None, "Default/CodeCache/": None, "GrShaderCache/data_0": None, "ShaderCache/index": None,
                 "ShaderCacheOld/": None, "OldShaderCache": None, "GraphiteDawnCache/data_0": None, "persistent_storage.db": None,
                 "sdk_storage/log/apollo-0-2026-01-01.alaudalog": 30, "sdk_storage/log/apollo-0-recent.alaudalog": 0,
                 "sdk_storage/log/apollo_2026.0101.log": 30, "sdk_storage/log/tea_records.log": 30,
                 "sdk_storage/log/netlog/netmain-0-2026-01-01.alaudalog": 30, "sdk_storage/log/xlog/deep/old-0.alaudalog": 30}
    common = dict(scratch=scratch, label="Feishu", subject={"kind": "app", "id": "bytedance.feishu"}, version=v, vrange=rng(v) if v else None,
                  fixture_env=env_r, fixture_spec=lark_spec, fixture_root=lark_fx)
    check("bytedance.feishu.code-cache", "CAT-03", real_base=lark, fixture_base=lark_fx, fixture_expected=["CodeCache"],
          positive=["Real %APPDATA%\\LarkShell\\CodeCache matched as the only candidate"],
          negative=["A nested Default\\CodeCache, the shader caches, profile state and databases are not matched"],
          effects=["Feishu recompiles its scripts on the next start; skipped while Feishu runs or holds a file open."], limitations=[], **common)
    check("bytedance.feishu.shader-cache", "CAT-03", real_base=lark, fixture_base=lark_fx, fixture_expected=["GrShaderCache", "ShaderCache"],
          positive=["Real LarkShell\\GrShaderCache and LarkShell\\ShaderCache matched"],
          negative=["ShaderCacheOld, a file named OldShaderCache, GraphiteDawnCache and CodeCache are not matched"],
          effects=["Rebuilt as Feishu windows are drawn; skipped while Feishu runs."], limitations=[], **common)
    check("bytedance.feishu.logs", "CAT-07", real_base=lark / "sdk_storage" / "log", fixture_base=lark_fx / "sdk_storage" / "log",
          fixture_expected=["apollo-0-2026-01-01.alaudalog", "netlog\\netmain-0-2026-01-01.alaudalog"], real_must_match=False,
          positive=["30-day-old *.alaudalog files directly in sdk_storage\\log and one folder below are review-only candidates"],
          negative=["A log written today is not a candidate", "*.log analytics/record files are not matched", "Logs two folders deep are not matched",
                    "Real logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Past Feishu diagnostic logs are no longer available for support."], limitations=[],
          real_note=" (real logs are all younger than 7 days on this device)", **common)

    # WeChat (Weixin 4.x) -----------------------------------------------------------------
    v = inst("tencent.wechat.logs")
    xw, xw_fx = ROAMING / "Tencent" / "xwechat", roaming / "Tencent" / "xwechat"
    xw_spec = {"log/mm_20260101.xlog": 30, "log/mm_recent.xlog": 0, "log/mm.mmap3": 30, "log/radium/host_20260101.xlog": 30,
               "log/radium/deep/x_20260101.xlog": 30, "radium/web/profiles/game/Code Cache/js/a": None,
               "radium/web/profiles/multitab/Code Cache/wasm/b": None, "radium/web/profiles/game/Cache/Cache_Data/c": None,
               "radium/web/profiles/game/Local Storage/leveldb/d": None, "radium/web/profiles/game/sub/Code Cache/e": None}
    common = dict(scratch=scratch, label="WeChat (Weixin)", subject={"kind": "app", "id": "tencent.wechat"}, version=v, vrange=rng(v) if v else None,
                  fixture_env=env_r, fixture_spec=xw_spec, fixture_root=xw_fx)
    check("tencent.wechat.logs", "CAT-07", real_base=xw / "log", fixture_base=xw_fx / "log",
          fixture_expected=["mm_20260101.xlog", "radium\\host_20260101.xlog"], real_must_match=False,
          positive=["30-day-old *.xlog files in xwechat\\log and log\\radium are review-only candidates"],
          negative=["A log written today, the mm.mmap3 log buffer and logs two folders deep are not matched",
                    "Real logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Past WeChat diagnostic logs are no longer available."], limitations=["Chat data (Documents\\xwechat_files) is outside every resolved root."],
          real_note=" (real logs are all younger than 7 days on this device)", **common)
    check("tencent.wechat.web-code-cache", "CAT-03", real_base=xw / "radium" / "web" / "profiles", fixture_base=xw_fx / "radium" / "web" / "profiles",
          fixture_expected=["game\\Code Cache", "multitab\\Code Cache"],
          positive=["Real radium\\web\\profiles\\<profile>\\Code Cache folders matched"],
          negative=["HTTP Cache, Local Storage and a Code Cache two folders deep are not matched"],
          effects=["Mini-program and web view scripts are recompiled; skipped while WeChat runs."], limitations=[], **common)

    # UU Remote (NetEase GameViewer) ------------------------------------------------------
    v = inst("netease.uu-remote.code-cache")
    uu, uu_fx = LOCAL / "GameViewer" / "webviewcache" / "EBWebView", local / "GameViewer" / "webviewcache" / "EBWebView"
    uu_spec = {"Default/Code Cache/js/a": None, "Default/Cache/Cache_Data/b": None, "Default/Cache/No_Vary_Search/c": None,
               "Default/Local Storage/leveldb/d": None, "Default/GPUCache/e": None, "Profile 1/Code Cache/f": None}
    common = dict(scratch=scratch, label="UU Remote (NetEase GameViewer)", subject={"kind": "app", "id": "netease.uu-remote"}, version=v,
                  vrange=rng(v) if v else None, real_base=uu, fixture_base=uu_fx, fixture_env=env_l, fixture_spec=uu_spec, limitations=[])
    check("netease.uu-remote.code-cache", "CAT-03", fixture_expected=["Default\\Code Cache"],
          positive=["Real EBWebView\\Default\\Code Cache matched as the only candidate"],
          negative=["HTTP cache, Local Storage, GPUCache and another profile's Code Cache are not matched"],
          effects=["Recompiled when the UU Remote interface runs; skipped while GameViewer runs."], **common)
    check("netease.uu-remote.http-cache", "CAT-03", fixture_expected=["Default\\Cache\\Cache_Data"],
          positive=["Real EBWebView\\Default\\Cache\\Cache_Data matched as the only candidate"],
          negative=["No_Vary_Search, Code Cache, Local Storage and other profiles are not matched"],
          effects=["Interface resources are downloaded again; skipped while GameViewer runs."], **common)

    # Docker Desktop ----------------------------------------------------------------------
    v = inst("docker.desktop.code-cache")
    dd, dd_fx = ROAMING / "Docker Desktop", roaming / "Docker Desktop"
    dd_spec = {"Code Cache/js/a": None, "GPUCache/data_0": None, "Cache/Cache_Data/b": None, "Cache/No_Vary_Search/c": None,
               "Local Storage/leveldb/d": None, "Default/GPUCache/e": None, "settings.json": None}
    common = dict(scratch=scratch, label="Docker Desktop", subject={"kind": "app", "id": "docker.desktop"}, version=v, vrange=rng(v) if v else None,
                  real_base=dd, fixture_base=dd_fx, fixture_env=env_r, fixture_spec=dd_spec,
                  limitations=["Only the dashboard (Electron) caches are matched; container data lives in Docker's own disk image."])
    for rid, comp, eff in (("docker.desktop.code-cache", "Code Cache", "Recompiled when the dashboard runs."),
                           ("docker.desktop.gpu-cache", "GPUCache", "Rebuilt when the dashboard is drawn."),
                           ("docker.desktop.http-cache", "Cache\\Cache_Data", "Dashboard resources are downloaded again.")):
        check(rid, "CAT-03", fixture_expected=[comp], positive=[f"Real %APPDATA%\\Docker Desktop\\{comp} matched as the only candidate"],
              negative=["The other cache folders, Local Storage, a nested Default\\GPUCache and settings are not matched"],
              effects=[eff + " Skipped while Docker Desktop runs."], **common)
    dl, dl_fx = LOCAL / "Docker" / "log", local / "Docker" / "log"
    check("docker.desktop.logs", "CAT-07", scratch=scratch, label="Docker Desktop", subject={"kind": "app", "id": "docker.desktop"}, version=v,
          vrange=rng(v) if v else None, real_base=dl, fixture_base=dl_fx, fixture_env=env_l, real_must_match=False,
          fixture_spec={"host/com.docker.backend.old.log": 30, "host/new.log": 0, "host/electron-audit-log.json": 30, "vm/init.log": 30,
                        "host/deep/x.log": 30},
          fixture_expected=["host\\com.docker.backend.old.log", "vm\\init.log"],
          positive=["30-day-old *.log files in Docker\\log\\host and \\vm are review-only candidates"],
          negative=["Logs written today, audit JSON files and logs two folders deep are not matched",
                    "Real logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Past Docker Desktop logs are no longer available for diagnostics."], limitations=[],
          real_note=" (real logs are all younger than 7 days on this device)")

    # OneDrive ----------------------------------------------------------------------------
    v = inst("microsoft.onedrive.logs")
    od, od_fx = LOCAL / "Microsoft" / "OneDrive", local / "Microsoft" / "OneDrive"
    od_spec = {"logs/Common/FileCoAuth-2026-01-01.odl": 30, "logs/Common/FileCoAuth-recent.odl": 0, "logs/Common/general.keystore": 30,
               "logs/ListSync/Business1/microsoftNucleusTelemetryCache.otc": 30, "logs/ListSync/Business1/Nucleus-2026-01-01.odlgz": 30,
               "logs/Personal/CldFlt0-2026-01-01.etlgz": 30, "logs/Personal/FeedbackHub/SubmissionPayload.json": 30,
               "logs/a/b/c/d/deep.odl": 30, "setup/logs/Install_2026-01-01.log": 30, "setup/logs/Update_recent.log": 0,
               "setup/logs/Install-2026-01-01.aodl": 30, "setup/logs/sub/x.log": 30, "setup/logs/notes.txt": 30, "settings/Personal/global.ini": 30}
    common = dict(scratch=scratch, label="Microsoft OneDrive", subject={"kind": "app", "id": "microsoft.onedrive"}, version=v,
                  vrange=rng(v) if v else None, real_base=od, fixture_base=od_fx, fixture_env=env_l, fixture_spec=od_spec, real_must_match=False,
                  real_note=" (real logs are all younger than 7 days on this device)",
                  limitations=["OneDrive runs continuously; the rows use the open-file probe so only logs it no longer holds are eligible."])
    check("microsoft.onedrive.logs", "CAT-07",
          fixture_expected=["logs\\Common\\FileCoAuth-2026-01-01.odl", "logs\\ListSync\\Business1\\Nucleus-2026-01-01.odlgz",
                            "logs\\Personal\\CldFlt0-2026-01-01.etlgz"],
          positive=["30-day-old .odl/.odlgz/.etlgz logs under OneDrive\\logs are review-only candidates"],
          negative=["Log key stores (general.keystore), telemetry caches (.otc), feedback payloads, logs from today and logs nested too deep are not matched",
                    "Real logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Past sync logs are no longer available to OneDrive support."], **common)
    check("microsoft.onedrive.setup-logs", "CAT-07", fixture_expected=["setup\\logs\\Install_2026-01-01.log", "setup\\logs\\Install-2026-01-01.aodl"],
          positive=["30-day-old installer/updater logs directly in OneDrive\\setup\\logs are review-only candidates"],
          negative=["Logs from today, text notes, logs in subfolders and settings are not matched",
                    "Real setup logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Update troubleshooting history is lost."], **common)

    # winget ------------------------------------------------------------------------------
    wv = tool_version(["winget", "--version"])
    wg = LOCAL / "Packages" / "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe" / "LocalState"
    wg_fx = local / "Packages" / "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe" / "LocalState"
    check("microsoft.winget.logs", "CAT-07", scratch=scratch, label="Windows Package Manager (winget)", subject={"kind": "app", "id": "microsoft.winget"},
          version=wv, vrange=rng(wv) if wv else None, real_base=wg, fixture_base=wg_fx, fixture_env=env_l, real_must_match=False,
          fixture_spec={"DiagOutputDir/WinGet-2026-01-01.log": 30, "DiagOutputDir/WinGet-recent.log": 0, "DiagOutputDir/old.txt": 30,
                        "settings.json": 30, "DiagOutputDir/sub/x.log": 30},
          fixture_expected=["DiagOutputDir\\WinGet-2026-01-01.log"],
          positive=["30-day-old winget/App Installer logs in DiagOutputDir are review-only candidates"],
          negative=["Logs from today, non-log files, nested logs and winget settings are not matched",
                    "Real logs on this device are newer than 7 days and were correctly not candidates"],
          effects=["Past winget logs are no longer available."],
          limitations=["winget is an MSIX package without an uninstall record, so the live registry verdict is version_untested (review_only)."],
          real_note=" (real logs are all younger than 7 days on this device)")

    # Package managers --------------------------------------------------------------------
    pv = tool_version([shutil.which("pnpm"), "--version"]) if shutil.which("pnpm") else None
    check("pnpm.metadata-cache", "CAT-03", scratch=scratch, label="pnpm", subject={"kind": "tool", "id": "pnpm"}, version=pv,
          vrange=rng(pv) if pv else None, real_base=LOCAL / "pnpm-cache", fixture_base=local / "pnpm-cache", fixture_env=env_l,
          fixture_spec={"metadata-v1.3/registry.npmjs.org/a.json": None, "metadata-full-v1.3/registry.npmjs.org/b.json": None, "v12/": None,
                        "lockfile-verified.jsonl": None, "store/metadata-x/": None},
          fixture_expected=["metadata-full-v1.3", "metadata-v1.3"],
          positive=["Real pnpm-cache\\metadata-v1.3 matched"],
          negative=["Other cache folders, the lockfile-verified journal and nested metadata folders are not matched; the package store is elsewhere"],
          effects=["Metadata is fetched again on the next install; offline installs need the network."],
          limitations=["pnpm is not installed by an installer here (user node_modules), so no owner-tool adapter and the live verdict is version_untested; "
                       "a dependency cache is review_only regardless."])
    pm = tool_version(["py", "help"], r"(?<=manager )\d+\.\d+")
    check("python.install-manager.downloads", "CAT-03", scratch=scratch, label="Python install manager", subject={"kind": "tool", "id": "python.install-manager"},
          version=pm, vrange=rng(pm) if pm else None, real_base=LOCAL / "Python", fixture_base=local / "Python", fixture_env=env_l,
          fixture_spec={"_cache/pythoncore-3.15-64-3.15.0.zip": None, "_cache/last_welcome.txt": None, "_cache/sub/x.zip": None,
                        "pythoncore-3.15-64/python.exe": None, "bin/python3.exe": None},
          fixture_expected=["_cache\\pythoncore-3.15-64-3.15.0.zip"],
          positive=["Real Python\\_cache runtime download (*.zip) matched"],
          negative=["Installed runtimes (pythoncore-*), bin shims, non-zip files and nested archives are not matched"],
          effects=["Reinstalling the same runtime downloads it again."],
          limitations=["The install manager is an MSIX package without an uninstall record; live verdict version_untested (review_only)."])

    # GPU driver shader cache -------------------------------------------------------------
    gv = None
    out = V.run(["powershell", "-NoProfile", "-Command",
                 "(Get-CimInstance Win32_VideoController | Where-Object Name -like 'AMD*' | Select-Object -First 1).DriverVersion"])
    if out.returncode == 0 and re.fullmatch(r"\d+(\.\d+)+", out.stdout.strip()):
        gv = out.stdout.strip()
    check("amd.shader-cache", "CAT-03", scratch=scratch, label="AMD Radeon graphics driver", subject={"kind": "app", "id": "amd.radeon-driver"},
          version=gv, vrange=rng(gv) if gv else None, real_base=LOCAL / "AMD", fixture_base=local / "AMD", fixture_env=env_l,
          fixture_spec={"DxCache/a": None, "VkCache/b": None, "CN/settings.xml": None, "RadeonSoftware/c": None, "LogCache": None},
          fixture_expected=["DxCache", "VkCache"],
          positive=["Real AMD DxCache, DxcCache, DX9Cache, OglCache and VkCache matched"],
          negative=["AMD Software settings (CN), RadeonSoftware and a file named like a cache are not matched"],
          effects=["Shaders are rebuilt when applications next run; skipped while a cache file is held open."],
          limitations=["Driver version read from Win32_VideoController for the record; the live registry verdict is version_untested (review_only)."])

    # Administrator-only system logs ------------------------------------------------------
    wer, wer_fx = PROGRAM_DATA / "Microsoft" / "Windows" / "WER", pd / "Microsoft" / "Windows" / "WER"
    spec = {}
    for comp in ("ReportArchive", "ReportQueue"):
        spec.update({f"{comp}/AppCrash_old_1/Report.wer": 30, f"{comp}/AppCrash_old_1/": 30, f"{comp}/AppCrash_new_2/Report.wer": 0,
                     f"{comp}/AppCrash_new_2/": 0, f"{comp}/loose-old.txt": 30})
    spec["Temp/old/"] = 30
    for rid, comp in (("windows.wer.machine-report-archive", "ReportArchive"), ("windows.wer.machine-report-queue", "ReportQueue")):
        check(rid, "CAT-07", scratch=scratch, label="Windows Error Reporting (machine-wide)", subject={"kind": "system", "id": "windows.wer.machine"},
              version=osv, vrange=build_range(osv) if osv else None, real_base=wer, fixture_base=wer_fx, fixture_env=env_p, fixture_spec=spec,
              fixture_expected=[f"{comp}\\AppCrash_old_1"], real_must_match=False,
              positive=[f"30-day-old report folder in ProgramData WER {comp} is a review-only candidate"],
              negative=["A report folder from today, a loose file and the WER Temp folder are not matched",
                        f"Real {comp} reports on this device are newer than 7 days and were correctly not candidates",
                        "Without administrator rights the row is skipped (elevated_token probe; run_win_catalog_admin_skip_e2e)"],
              effects=["Error history is lost; queued reports are not sent. Administrator only: skipped when elevation is refused."],
              limitations=["The real machine-wide WER store was scanned read-only from an elevated terminal."],
              real_note=" (real reports are all younger than 7 days on this device)")


# --------------------------------------------------------------------------------------------
# Elevation refused end to end (CAT-06 + requires_admin): clean --dry-run elevated vs restricted.

def admin_skip_e2e(scratch: Path):
    rid = "run_win_catalog_admin_skip_e2e"
    pd = scratch / "e2e-programdata"
    wer = pd / "Microsoft" / "Windows" / "WER"
    build(wer, {"ReportArchive/AppCrash_old_9/Report.wer": 30, "ReportArchive/AppCrash_old_9/": 30})
    env = {**os.environ, "CM_RESOLVE_PROGRAMDATA": str(pd)}
    item = lambda rec: next((i for i in rec.get("items", []) if i["path"]["display"].endswith("AppCrash_old_9")), None)  # noqa: E731
    probe = lambda i: next((p["result"] for p in i["probes"] if p["probe"] == "elevated_token"), None) if i else None  # noqa: E731
    # Elevated (this process).
    store = scratch / "store-e2e-admin"
    _, scan = V.cli(["scan", "--root", str(wer), "--store", str(store)])
    _, high = V.cli(["clean", "--scan", scan["scan_id"], "--store", str(store), "--dry-run", "--non-interactive"], env)
    # Restricted token: Administrators deny-only, i.e. elevation refused. runas returns at once;
    # the batch file writes its JSON outputs and a done marker.
    low = scratch / "e2e-low"
    low.mkdir(parents=True, exist_ok=True)
    bat = low / "run.cmd"
    lines = ["@echo off", f'set "CM_RESOLVE_PROGRAMDATA={pd}"',
             f'"{V.CLI}" scan --root "{wer}" --store "{low / "store"}" --json > "{low / "scan.json"}"',
             f'for /f "usebackq delims=" %%i in (`powershell -NoProfile -Command "(Get-Content -Raw \'{low / "scan.json"}\' | ConvertFrom-Json).scan_id"`) do set SID=%%i',
             f'"{V.CLI}" clean --scan %SID% --store "{low / "store"}" --dry-run --non-interactive --json > "{low / "dry.json"}"',
             f'"{V.CLI}" clean --scan %SID% --store "{low / "store"}" --mode trash --candidates %CID% --json > "{low / "plan.json"}"',
             f'echo done> "{low / "done.txt"}"']
    # The plan attempt needs the candidate id from the dry run: run the batch in two phases.
    bat.write_text("\r\n".join(lines[:5] + [lines[-1]]) + "\r\n", encoding="ascii")
    subprocess.run(["runas.exe", "/trustlevel:0x20000", f'C:\\Windows\\System32\\cmd.exe /c "{bat}"'], check=True)
    deadline = time.time() + 180
    while not (low / "done.txt").exists() and time.time() < deadline:
        time.sleep(1)
    dry = json.loads((low / "dry.json").read_text(encoding="utf-8-sig"))
    lo = item(dry)
    plan = {}
    if lo is not None:
        (low / "done.txt").unlink()
        bat.write_text("\r\n".join([lines[0], lines[1], f'set "SID={json.loads((low / "scan.json").read_text(encoding="utf-8-sig"))["scan_id"]}"',
                                    f'set "CID={lo["object_id"]}"', lines[5], lines[6]]) + "\r\n", encoding="ascii")
        subprocess.run(["runas.exe", "/trustlevel:0x20000", f'C:\\Windows\\System32\\cmd.exe /c "{bat}"'], check=True)
        deadline = time.time() + 180
        while not (low / "done.txt").exists() and time.time() < deadline:
            time.sleep(1)
        plan = json.loads((low / "plan.json").read_text(encoding="utf-8-sig"))
    hi = item(high)
    ok = (hi is not None and probe(hi) == "not_in_use" and hi["skip_reason"] is None and hi["tier"] == "review_only" and not hi["selected_by_default"]
          and lo is not None and probe(lo) == "unknown" and lo["skip_reason"] == "probe_unknown" and not lo["selected_by_default"]
          and plan.get("kind") == "error" and plan.get("code") == "policy_denied")
    record(rid, "CAT-06",
           "windows.wer.machine-report-archive (requires_admin): with administrator rights the 30-day-old fixture report is a review-only row "
           "(elevated_token not_in_use); under a restricted token (elevation refused) the same row is skipped (elevated_token unknown, "
           "skip probe_unknown) and an explicit plan request for it is refused (policy_denied)",
           f"elevated: probe={probe(hi)} skip={hi and hi['skip_reason']} tier={hi and hi['tier']} selected={hi and hi['selected_by_default']}; "
           f"restricted: probe={probe(lo)} skip={lo and lo['skip_reason']} selected={lo and lo['selected_by_default']}; "
           f"restricted_plan={plan.get('kind')}:{plan.get('code')}",
           ok, scan["backend"], {"restricted_tier_reason": (lo or {}).get("tier_reason"), "elevated_tier_reason": (hi or {}).get("tier_reason"), "restricted_plan_message": plan.get("message")},
           ["Controlled fixture through CM_RESOLVE_PROGRAMDATA; the real ProgramData WER store was not modified.",
            "The restricted run used runas /trustlevel:0x20000 (Administrators deny-only), which reports TokenIsElevated=1 but has no "
            "administrator rights; the probe therefore checks Administrators membership as well as elevation."],
           ["cleanmanager", "clean", "--scan", "ID", "--dry-run", "--non-interactive"], elevated=True)
    RESULTS["admin_skip_e2e"] = {"ok": ok}


def real_dry_run(scratch: Path):
    """`clean --dry-run` (read-only preview, nothing planned) over the real wave-2 locations: the
    live probes answer for every row, owner_process follows the real process list, and nothing
    from wave 2 is preselected."""
    rid = "run_win_catalog_wave2_real_dry_run"
    roots = [ROAMING / "LarkShell", ROAMING / "Tencent" / "xwechat" / "radium", ROAMING / "Docker Desktop",
             LOCAL / "GameViewer" / "webviewcache", LOCAL / "AMD", LOCAL / "pnpm-cache", LOCAL / "Python"]
    procs = {line.split('","')[0].strip('"').lower() for line in V.run(["tasklist", "/fo", "csv", "/nh"]).stdout.splitlines() if line.strip()}
    wave2 = {p.stem for p in (ROOT / "rules/windows").glob("*.json")} - {
        "google.chrome.code-cache", "google.chrome.gpu-cache", "google.chrome.http-cache", "microsoft.edge.code-cache", "microsoft.edge.gpu-cache",
        "microsoft.edge.http-cache", "microsoft.vscode.cached-data", "microsoft.vscode.code-cache", "microsoft.vscode.gpu-cache"}
    rows, problems = [], []
    for i, root in enumerate(r for r in roots if r.exists()):
        store = scratch / f"store-dry-{i}"
        _, scan = V.cli(["scan", "--root", str(root), "--store", str(store)])
        _, prev = V.cli(["clean", "--scan", scan["scan_id"], "--store", str(store), "--dry-run", "--non-interactive"])
        for it in prev.get("items", []):
            if it["rule_id"] not in wave2:
                continue
            rule = load_rule(it["rule_id"])
            probes = {p["probe"]: p["result"] for p in it["probes"]}
            owner_exes = {"bytedance.feishu": ["feishu.exe", "lark.exe"], "tencent.wechat": ["weixin.exe", "wechat.exe", "wechatappex.exe"],
                          "netease.uu-remote": ["gameviewer.exe"], "python.install-manager": ["py.exe", "pymanager.exe", "pyw.exe"],
                          "docker.desktop": ["docker desktop.exe", "com.docker.backend.exe", "com.docker.build.exe"]}.get(rule["owner"]["id"])
            if set(probes) != set(rule["probes"]):
                problems.append(f"{it['rule_id']}: probes {sorted(probes)} != declared {rule['probes']}")
            if "owner_process" in probes and owner_exes is not None:
                running = any(e in procs for e in owner_exes)
                if probes["owner_process"] != ("in_use" if running else "not_in_use"):
                    problems.append(f"{it['rule_id']}: owner_process={probes['owner_process']} but running={running}")
            if it["selected_by_default"] or it["tier"] != "review_only":
                problems.append(f"{it['rule_id']}: tier={it['tier']} selected={it['selected_by_default']}")
            skipped = any(v != "not_in_use" for v in probes.values())
            if (it["skip_reason"] is not None) != skipped:
                problems.append(f"{it['rule_id']}: skip_reason={it['skip_reason']} with probes {probes}")
            rows.append({"rule_id": it["rule_id"], "path": it["path"]["display"], "tier": it["tier"], "skip_reason": it["skip_reason"],
                         "probes": probes, "registry_status": it["registry_status"]})
    ok = bool(rows) and not problems
    record(rid, "CAT-06",
           "clean --dry-run over the real Feishu, WeChat web view, Docker Desktop, UU Remote, AMD shader, pnpm and Python install-manager "
           "locations: every wave-2 row is review-only and never preselected, every declared probe is answered, owner_process agrees with "
           "the live process list, and any in_use/unknown probe skips the row",
           f"rows={len(rows)} skipped={sum(1 for r in rows if r['skip_reason'])} problems={problems}",
           ok, "win.directory", {"rows": rows}, ["Preview only: no plan was created and nothing was changed."],
           ["cleanmanager", "clean", "--scan", "ID", "--dry-run", "--non-interactive"])
    RESULTS["wave2_real_dry_run"] = {"ok": ok, "rows": len(rows), "problems": problems}


def validate_not_installed(cat: dict):
    adapters = {a["adapter_id"]: a for a in cat["adapters"]}
    docker_wsl = LOCAL / "Docker" / "wsl"
    keep_experimental("docker.desktop.data-disk", "CAT-09",
                      "Docker Desktop is installed but its engine has never created a data disk here (no Local AppData\\Docker\\wsl)",
                      {"docker_wsl_present": docker_wsl.exists()})
    a = adapters.get("docker.system-prune", {})
    keep_experimental("docker.engine.unused-data", "CAT-05",
                      "docker.system-prune adapter unavailable: Docker Desktop is installed per user (Local AppData\\Programs, user-writable), "
                      "not in Program Files, and there is no engine data; the cleanup command was never run and the disk is never deleted directly",
                      {"adapter_status": a.get("status"), "reason": a.get("reason"), "docker_wsl_present": docker_wsl.exists()})
    absent = [
        ("microsoft.teams.code-cache", "Microsoft Teams", LOCAL / "Packages" / "MSTeams_8wekyb3d8bbwe"),
        ("slack.code-cache", "Slack", ROAMING / "Slack"),
        ("discord.code-cache", "Discord", ROAMING / "discord"),
        ("valve.steam.web-code-cache", "Steam", LOCAL / "Steam"),
        ("epicgames.launcher.webcache", "Epic Games Launcher", LOCAL / "EpicGamesLauncher"),
        ("adobe.media-cache", "Adobe Premiere Pro / After Effects (Creative Cloud)", ROAMING / "Adobe" / "Common"),
        ("nvidia.shader-cache", "an NVIDIA graphics driver", LOCAL / "NVIDIA"),
        ("google.android-emulator.avd-images", "the Android Emulator (no virtual devices)", HOME / ".android" / "avd"),
    ]
    for rid, label, path in absent:
        keep_experimental(rid, "CAT-03", f"{label} is not installed on this device", {"location_present": path.exists()})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", type=Path, required=True)
    ap.add_argument("--cli", type=Path, default=ROOT / "target" / "debug" / "cleanmanager.exe")
    args = ap.parse_args()
    V.CLI = args.cli.resolve()
    scratch = args.scratch.resolve()
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    cat = C.catalog()
    validate_installed(scratch, cat)
    validate_not_installed(cat)
    admin_skip_e2e(scratch)
    real_dry_run(scratch)
    print(json.dumps(RESULTS, indent=2, ensure_ascii=False))
    print("validated at", datetime.now(timezone.utc).isoformat())


if __name__ == "__main__":
    main()
