"""Record real Windows read-only scan runs (MVP task 4.6) as run records.

Every run uses the real `cleanmanager` binary against fixtures prepared by `cm-fixtures` in a
scratch directory; fixtures and the out-of-scope sentinel are re-verified afterwards. `actual`
is filled only from observed output. The exFAT run mounts a disposable VHD and only happens with
--with-vhd (elevated shell required). Nothing outside the scratch directory is scanned or written,
except the temporary VHD mount.

Usage: python tools/record_windows_runs.py --scratch DIR [--with-vhd]
"""
from __future__ import annotations
import argparse
import hashlib
import json
import platform
import shutil
import string
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".research" / "python-deps"))
from jsonschema import Draft202012Validator, FormatChecker  # noqa: E402

DEVICE = "dev_win11_x64_primary"
PROTOCOL = Draft202012Validator(json.loads((ROOT / "schemas/v1/protocol.schema.json").read_text(encoding="utf-8")), format_checker=FormatChecker())
RUN_SCHEMA = Draft202012Validator(json.loads((ROOT / "schemas/v1/run-record.schema.json").read_text(encoding="utf-8")), format_checker=FormatChecker())


def utc():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", **kw)


def os_facts():
    release, version, _, _ = platform.win32_ver()
    build = int(version.split(".")[2]) if version.count(".") >= 2 else 0
    product = "Windows 11" if build >= 22000 else f"Windows {release}"
    return {"platform": "windows", "name": f"{product} {platform.win32_edition()}".strip(), "build": version or None,
            "architecture": "x64" if platform.machine().lower() in ("amd64", "x86_64") else "arm64"}


def bin_path(name):
    return ROOT / "target" / "debug" / f"{name}.exe"


def fixtures(*args):
    out = run([str(bin_path("cm-fixtures")), *args])
    if out.returncode != 0:
        raise SystemExit("cm-fixtures failed: " + out.stderr)
    return json.loads(out.stdout)


def scan(root: Path, store: Path):
    started = utc()
    out = run([str(bin_path("cleanmanager")), "scan", "--root", str(root), "--store", str(store), "--json"])
    record = json.loads(out.stdout)
    PROTOCOL.validate(record)
    return started, out.returncode, record


def write_run(test_id, run_id, expected, actual, status, record, exit_code, fs, extra_limits, measurements, fixture_sha, command):
    commit = run(["git", "rev-parse", "HEAD"], cwd=ROOT).stdout.strip() or None
    dirty = bool(run(["git", "status", "--porcelain"], cwd=ROOT).stdout.strip())
    rec = {
        "schema_version": "1.0", "test_id": test_id, "run_id": run_id, "recorded_at": utc(),
        "code_commit": commit, "binary_sha256": sha256_file(bin_path("cleanmanager")), "device_id": DEVICE,
        "os": os_facts(), "filesystem": {"type": fs, "volume_id": None},
        "permission_context": {"elevated": True, "signing_identity": None,
                               "grants": ["SeBackupPrivilege/SeRestorePrivilege disabled by cleanmanager before scanning"]},
        "scope_digest": hashlib.sha256(record["scope_id"].encode()).hexdigest(), "fixture_manifest_sha256": fixture_sha,
        "backend": record["backend"], "config": {"batch_entries": 4096}, "command": command, "seed": 20261014,
        "expected": expected, "actual": actual, "terminal_state": record["status"], "exit_code": exit_code,
        "measurements": measurements, "observer": "tools/record_windows_runs.py",
        "limitations": (["Working tree had uncommitted changes; code_commit is the base commit."] if dirty else []) + extra_limits,
        "status": status,
    }
    RUN_SCHEMA.validate(rec)
    path = ROOT / "docs/evidence/runs" / f"{run_id}.json"
    path.write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("recorded", path.relative_to(ROOT))
    return run_id


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", type=Path, required=True)
    ap.add_argument("--with-vhd", action="store_true")
    args = ap.parse_args()
    subprocess.run(["cargo", "build", "-q", "-p", "cm-cli", "-p", "cm-fixtures"], cwd=ROOT, check=True)
    scratch = args.scratch.resolve()
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    store, sentinel = scratch / "store", scratch / "sentinel"
    fixtures("sentinel", "--path", str(sentinel))
    runs = []

    # 1. tiny fixture: names, hard link, links to the sentinel
    tiny = scratch / "tiny"
    m = fixtures("create", "--profile", "tiny", "--seed", "20261014", "--root", str(tiny), "--sentinel", str(sentinel))
    started, code, rec = scan(tiny, store)
    fixtures("verify", "--root", str(tiny), "--sentinel", str(sentinel))
    expected_entries = 1 + m["counts"]["directories"] + m["counts"]["files"] + m["counts"]["hardlinks"] + m["counts"]["sentinel_links"] + 1
    ok = rec["status"] == "completed" and rec["entries_seen"] == expected_entries and code == 0
    runs.append(write_run("SCN-01", "run_win_dir_tiny", f"completed, {expected_entries} entries (root + fixture incl. 2 untraversed links + marker), exit 0, fixture and sentinel unchanged",
        f"status={rec['status']} completeness={rec['completeness']} entries_seen={rec['entries_seen']} logical_bytes={rec['logical_bytes']} exit={code}; fixture and sentinel verified unchanged",
        "passed" if ok else "failed", rec, code, "NTFS", [], {"entries_seen": rec["entries_seen"]}, hashlib.sha256(json.dumps(m, sort_keys=True).encode()).hexdigest(),
        ["cleanmanager", "scan", "--root", "<scratch>/tiny", "--store", "<scratch>/store", "--json"]))

    # 2. P0-10K
    p10k = scratch / "p0-10k"
    m = fixtures("create", "--profile", "p0-10k", "--seed", "20261014", "--root", str(p10k), "--sentinel", str(sentinel))
    t0 = datetime.now(timezone.utc)
    started, code, rec = scan(p10k, store)
    wall = (datetime.now(timezone.utc) - t0).total_seconds()
    fixtures("verify", "--root", str(p10k), "--sentinel", str(sentinel))
    marker = (p10k / ".cm-fixture-root.json").stat().st_size
    expected_bytes = int(m["logical_bytes"]) + marker
    ok = rec["status"] == "completed" and int(rec["logical_bytes"]) == expected_bytes and rec["unknown_size_count"] == 0
    runs.append(write_run("TC-P0-001", "run_win_dir_p0_10k", f"completed; logical_bytes equals manifest {m['logical_bytes']} + marker {marker} = {expected_bytes}; no unknown sizes",
        f"status={rec['status']} entries_seen={rec['entries_seen']} logical_bytes={rec['logical_bytes']} unknown={rec['unknown_size_count']} exit={code}",
        "passed" if ok else "failed", rec, code, "NTFS", ["Single warm run; not a benchmark (see G0 task 6.2)."],
        {"wall_seconds_including_cli_start_and_index_commit": round(wall, 3), "entries_seen": rec["entries_seen"]},
        hashlib.sha256(json.dumps(m, sort_keys=True).encode()).hexdigest(),
        ["cleanmanager", "scan", "--root", "<scratch>/p0-10k", "--store", "<scratch>/store", "--json"]))

    # 3. denied subtree → partial, typed reason, known results kept
    denied = scratch / "denied"
    (denied / "locked").mkdir(parents=True)
    (denied / "locked" / "inside.txt").write_text("x")
    (denied / "visible.txt").write_text("yy")
    run(["icacls", str(denied / "locked"), "/deny", "*S-1-1-0:(RX)"])
    try:
        started, code, rec = scan(denied, store)
    finally:
        run(["icacls", str(denied / "locked"), "/remove:d", "*S-1-1-0"])
    ok = rec["status"] == "partial" and rec["skipped_ranges"] == 1 and code == 5 and any(i["code"] == "permission_required" for i in rec["issues"])
    runs.append(write_run("SCN-03", "run_win_dir_denied", "partial, 1 skipped range with permission_required, exit 5, visible file kept",
        f"status={rec['status']} skipped_ranges={rec['skipped_ranges']} issues={[i['code'] for i in rec['issues']]} logical_bytes={rec['logical_bytes']} exit={code}",
        "passed" if ok else "failed", rec, code, "NTFS", ["Deny ACE for Everyone; the admin token's backup privilege was disabled by the CLI."], {}, None,
        ["cleanmanager", "scan", "--root", "<scratch>/denied", "--store", "<scratch>/store", "--json"]))

    # 4. exFAT (non-NTFS) on a disposable VHD
    if args.with_vhd:
        letter = next(c for c in reversed(string.ascii_uppercase[12:]) if not Path(f"{c}:\\").exists())
        vhd = scratch / "exfat.vhdx"
        dp = scratch / "exfat.dp"
        dp.write_text(f'create vdisk file="{vhd}" maximum=64 type=expandable\nselect vdisk file="{vhd}"\nattach vdisk\ncreate partition primary\nformat fs=exfat quick label=CMTEST\nassign letter={letter}\n')
        assert run(["diskpart", "/s", str(dp)]).returncode == 0
        try:
            data = Path(f"{letter}:\\data")
            (data / "sub").mkdir(parents=True)
            (data / "sub" / "中文.txt").write_bytes(b"\x01" * 3000)
            (data / "a.bin").write_bytes(b"\x02" * 5000)
            started, code, rec = scan(data, store)
            ok = rec["status"] == "completed" and rec["logical_bytes"] == "8000" and code == 0
            runs.append(write_run("TC-WIN-001", "run_win_dir_exfat", "completed on exFAT via FileIdBothDirectoryInfo fallback; logical_bytes 8000; identities weak",
                f"status={rec['status']} entries_seen={rec['entries_seen']} logical_bytes={rec['logical_bytes']} exit={code}",
                "passed" if ok else "failed", rec, code, "exFAT", ["64 MiB disposable VHD; no SMB volume tested."], {}, None,
                ["cleanmanager", "scan", "--root", f"{letter}:\\data", "--store", "<scratch>/store", "--json"]))
        finally:
            run(["powershell", "-NoProfile", "-Command", f'Dismount-DiskImage -ImagePath "{vhd}" | Out-Null'])
    print(json.dumps({"runs": runs}))


if __name__ == "__main__":
    main()
