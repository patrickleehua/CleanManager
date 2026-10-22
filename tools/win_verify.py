"""Repeatable Windows CLI acceptance for CleanManager (docs/verification-plan.md, "repeatable acceptance").

Productized from the one-off runner of the 2026-10-22 Windows acceptance
(docs/acceptance/windows-2026-10-22-cases.md). It runs the CLI cases that need neither the desktop
window nor a destructive action, against a synthetic fixture it creates under %TEMP% and its own
CM_STORE_DIR, and checks every result against expectations written here, independently of the
product's own output.

    python -I tools/win_verify.py [--cli PATH] [--strict] [--out DIR] [--keep-fixture]

Without --cli it builds the release CLI first (cargo build --release --locked -p cm-cli, honouring
CARGO_TARGET_DIR). Nothing outside the fixture and the run folder is written: no plan is approved,
nothing is moved, deleted or restored, and the fixture's SHA-256 manifest is compared before and after.

Results: target/win-verify/<stamp>/summary.md, results.json and run-record.json are redacted with the
Doctor rules (home -> ~, user -> <user>, host -> <host>); raw/ keeps the unredacted stdout/stderr of
every command for local debugging and must not be published.

Case status: passed, failed, known_pending (a case whose expectation is a fix landing now; on an
older binary it shows here instead of silently passing or failing the run), inconclusive (the run
could not decide, e.g. a scan finished before the cancel arrived) or not_run (a prerequisite failed).
Exit code: 0 all passed; 1 any failed; 3 no failure but something pending, inconclusive or not run;
2 usage or setup error. --strict treats known_pending as failed.

Standard library only, plus the project-local jsonschema (.research/python-deps, the same setup as
tools/validate_docs.py) for validating every JSON record against schemas/v1/protocol.schema.json.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True

# ---------------------------------------------------------------- fixture shape (independent expectations)
INPUT_FILES = [  # relative path, size, filler byte
    ("a.txt", 1024, b"a"),
    ("big.bin", 16384, b"b"),
    ("中文 空格.txt", 2048, b"c"),
    ("nested/second.log", 8192, b"d"),
    ("nested/zero.txt", 0, b"e"),
    (".ssh/id_ed25519", 256, b"f"),  # fake credential: filler bytes only, never a real key
]
INPUT_DIRS = [".ssh", "empty", "nested"]
INPUT_ENTRIES = 1 + len(INPUT_DIRS) + len(INPUT_FILES)  # the root counts as an entry
INPUT_BYTES = sum(size for _, size, _ in INPUT_FILES)
TREE_DIRS, TREE_FILES_PER_DIR = 400, 100  # 400 directories, 40,000 files
WIDE_FILES = 20000  # one directory
TREE_ENTRIES = 1 + TREE_DIRS * (TREE_FILES_PER_DIR + 1)  # root + directories + files
WIDE_ENTRIES = 1 + WIDE_FILES
PLAN_TTL_SECONDS = 900

# Cases whose expectation is the intended behaviour after fixes landing on 2026-10-22. On a binary
# without the fix they report known_pending (see module docstring), never a silent pass.
PENDING = {
    "WV-06": "BUG-WIN-01: a regular-file root fails with not_a_directory before any row is written",
    "WV-07": "review finding 2026-10-22: a junction root fails with root_is_link instead of a false complete scan",
    "WV-08": "contract change 2026-10-22: a failed scan reports logical_bytes and allocated_bytes as null, never \"0\"",
    "WV-12": "review finding 2026-10-22: a cancel inside one large directory stops at the next batch boundary (cancelled, exit 130)",
}

CASES = [
    ("WV-01", "help and doctor", "WIN-20", "help --format text exits 0 with readable text; doctor exits 0 with kind=doctor_report, redacted=true, and its stdout contains no home path, user name or host name"),
    ("WV-02", "experimental features default off", "WIN-02", "fresh store: every experimental capability disabled; no capability reports available; status is refused with unsupported / exit 7"),
    ("WV-03", "fixture scan", "WIN-04", f"scan of the fixture: completed / verified_scope / exit 0, {INPUT_ENTRIES} entries, logical_bytes \"{INPUT_BYTES}\", no issues; scan show returns the same record"),
    ("WV-04", "entries paging, filters and limits", "WIN-10", f"limit 2 pages cover all {INPUT_ENTRIES} entries exactly once; every file's logical_bytes equals the fixture manifest; byte fields are decimal strings or null; name filter 中文 returns exactly the 2048-byte file; an absent name returns 0 items; limit 0 and 501 are invalid_argument / exit 2"),
    ("WV-05", "missing root", "WIN-09", "scan of a missing directory: failed / exit 8, one root_unavailable issue, entries_seen 0, nothing indexed"),
    ("WV-06", "regular-file root", "WIN-09", "scan of a 1,024-byte file: failed / exit 8, one not_a_directory issue, entries_seen 0; entries list for that scan is empty (never a directory row)"),
    ("WV-07", "junction root", "TC-WIN-001", "scan of a junction: failed / exit 8, one root_is_link issue, entries_seen 0, nothing behind the link indexed"),
    ("WV-08", "failed scans have unknown sizes", "WIN-09", "every failed scan result above (missing, file and junction roots) has logical_bytes null and allocated_bytes null"),
    ("WV-09", "protection refuses a credential", "WIN-11", "fake .ssh/id_ed25519: inspect lists a protection reason, offers no move_to_trash and refuses with policy_denied; planning it is policy_denied / exit 3; protect check decides denied"),
    ("WV-10", "frozen plan needs approval", "WIN-08", f"manual trash plan for big.bin: one fs_trash item, plan_kind manual_selection, authorization_status required, policy_matchable false, expires_at - created_at = {PLAN_TTL_SECONDS} s; plan show returns the same body and digest; non-interactive apply is needs_approval / exit 3; operation history stays empty"),
    ("WV-11", "cancel a multi-directory scan", "WIN-19", f"detached scan of {TREE_DIRS} directories / {TREE_DIRS * TREE_FILES_PER_DIR} files, cancelled at once: accepted receipt, then cancelled / partial_scope / exit 130 (never completed) with 0 < entries_seen <= {TREE_ENTRIES}, retained rows queryable"),
    ("WV-12", "cancel inside one large directory", "WIN-19", f"detached scan of one directory with {WIDE_FILES} files, cancelled at once: accepted receipt, then cancelled / partial_scope / exit 130 (never completed) with 0 < entries_seen <= {WIDE_ENTRIES}, retained rows queryable; whether it stopped before the listing ended is recorded, not required (it depends on timing)"),
    ("WV-13", "experimental switch on and off", "WIN-21", "experimental --state on enables every gated capability with gate_passed false and still no capability is available; --state off disables them and status is unsupported / exit 7 again"),
    ("WV-14", "fixture integrity and no execution", "WIN-22", "input, sentinel and junction-target SHA-256 manifests and the cancel fixtures' file counts are identical before and after; operation history and quarantine are empty; no plan was approved"),
    ("WV-15", "protocol schema", "WIN-20", "every JSON stdout record parses without duplicate keys, validates against schemas/v1/protocol.schema.json and satisfies the validator's semantic invariants"),
]


# ---------------------------------------------------------------- redaction (mirrors crates/cm-cli/src/doctor_cmd.rs)
def short_path(p: str) -> str:
    try:
        buf = ctypes.create_unicode_buffer(1024)
        n = ctypes.windll.kernel32.GetShortPathNameW(p, buf, 1024)
        return buf.value if 0 < n < 1024 else p
    except (AttributeError, OSError):
        return p


class Redactor:
    def __init__(self):
        pairs = []
        home = os.environ.get("USERPROFILE") or os.environ.get("HOME") or ""
        if len(home) > 3:
            for h in {home.rstrip("\\/"), short_path(home).rstrip("\\/")}:
                pairs += [(h, "~"), (h.replace("\\", "/"), "~"), (h.replace("\\", "\\\\"), "~")]  # plain, slashed, escaped
        for var, mask in (("USERNAME", "<user>"), ("USER", "<user>"), ("COMPUTERNAME", "<host>"), ("HOSTNAME", "<host>")):
            v = os.environ.get(var, "")
            if len(v) >= 2:
                alias = "".join(c for c in v if not c.isspace() and c != ".")[:6]
                if len(v) > 8 and len(alias) == 6:
                    pairs += [(f"{alias}~{n}", mask) for n in range(1, 10)]
                pairs.append((v, mask))
        pairs += [(str(ROOT), "<repo>"), (str(ROOT).replace("\\", "/"), "<repo>"), (str(ROOT).replace("\\", "\\\\"), "<repo>")]
        self.pairs = sorted(set(pairs), key=lambda p: -len(p[0]))

    def text(self, s: str) -> str:
        for needle, mask in self.pairs:
            s = re.sub(re.escape(needle), lambda _m, m=mask: m, s, flags=re.I)
        return s

    def value(self, v):
        if isinstance(v, str):
            return self.text(v)
        if isinstance(v, list):
            return [self.value(x) for x in v]
        if isinstance(v, dict):  # raw path bytes cannot be redacted piecewise: keep only the display
            return {k: self.value(x) for k, x in v.items() if k != "raw_base64"}
        return v

    def leaks(self, s: str) -> list[str]:
        out = []
        for var in ("USERNAME", "COMPUTERNAME"):
            v = os.environ.get(var, "")
            if len(v) >= 2 and v.lower() in s.lower():
                out.append(var)
        home = os.environ.get("USERPROFILE", "")
        if len(home) > 3 and home.lower() in s.lower():
            out.append("USERPROFILE")
        return out


RED = Redactor()


# ---------------------------------------------------------------- helpers
def strict_json(text: str):
    def unique(pairs):
        out = {}
        for k, v in pairs:
            if k in out:
                raise ValueError("duplicate JSON key: " + k)
            out[k] = v
        return out
    return json.loads(text, object_pairs_hook=unique)


def manifest(root: Path) -> dict:
    files, dirs = {}, []
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root).as_posix()
        if p.is_dir():
            dirs.append(rel)
        elif p.is_file():
            data = p.read_bytes()
            files[rel] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    return {"files": files, "directories": dirs, "logical_bytes": sum(f["bytes"] for f in files.values())}


def count_files(root: Path) -> int:
    return sum(len(f) for _, _, f in os.walk(root))


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def canon_sha(v) -> str:
    return hashlib.sha256(json.dumps(v, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode("utf-8")).hexdigest()


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def make_junction(link: Path, target: Path) -> str | None:
    """Create a directory junction (no elevation needed). Returns an error text, or None."""
    try:
        import _winapi
        _winapi.CreateJunction(str(target), str(link))
        return None
    except (ImportError, AttributeError, OSError) as e:
        first = str(e)
    p = subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], capture_output=True, text=True)
    return None if p.returncode == 0 else f"{first}; mklink /J: {p.stdout.strip()} {p.stderr.strip()}"


def is_junction(p: Path) -> bool:
    fn = getattr(os.path, "isjunction", None)
    if fn:
        return fn(p)
    try:
        return bool(os.lstat(p).st_file_attributes & 0x400) and not p.is_symlink()
    except OSError:
        return False


# ---------------------------------------------------------------- environment facts
def os_facts() -> dict:
    build = platform.version()
    try:
        major = int(build.split(".")[2])
    except (IndexError, ValueError):
        major = 0
    edition = platform.win32_edition() if hasattr(platform, "win32_edition") else ""
    name = ("Windows 11" if major >= 22000 else "Windows 10" if major else "Windows") + (f" {edition}" if edition else "")
    arch = None
    try:
        k = ctypes.windll.kernel32
        pm, nm = ctypes.c_ushort(), ctypes.c_ushort()
        if k.IsWow64Process2(k.GetCurrentProcess(), ctypes.byref(pm), ctypes.byref(nm)):
            arch = {0x8664: "x64", 0xAA64: "arm64"}.get(nm.value)
    except (AttributeError, OSError):
        pass
    if arch is None:
        arch = {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(platform.machine().lower())
    return {"platform": "windows", "name": name, "build": build, "architecture": arch}


def filesystem_of(path: Path) -> str | None:
    try:
        root = os.path.splitdrive(str(path))[0] + "\\"
        fs = ctypes.create_unicode_buffer(64)
        ok = ctypes.windll.kernel32.GetVolumeInformationW(root, None, 0, None, None, None, fs, 64)
        return fs.value if ok else None
    except (AttributeError, OSError):
        return None


def elevated() -> bool | None:
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return None


def git(*args) -> str | None:
    try:
        p = subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True, text=True, timeout=30)
        return p.stdout.strip() if p.returncode == 0 else None
    except (OSError, subprocess.SubprocessError):
        return None


# ---------------------------------------------------------------- the run
class Run:
    def __init__(self, cli: Path, out: Path, timeout: int):
        self.cli, self.out, self.timeout = cli, out, timeout
        self.raw = out / "raw"
        self.raw.mkdir(parents=True, exist_ok=True)
        self.store = out / "store"
        self.env = dict(os.environ, CM_STORE_DIR=str(self.store))
        self.env.pop("CM_TEST_SCAN_SLOW_MS", None)
        self.seq = 0
        self.records = []  # (tag, record) for schema validation
        self.commands = []
        self.current = None  # case id the next commands belong to
        self.case_of = {}

    def cli_run(self, tag, *args, json_out=True, timeout=None):
        self.seq += 1
        argv = [str(self.cli), *map(str, args)] + (["--json", "--non-interactive"] if json_out else [])
        t0 = time.monotonic()
        try:
            p = subprocess.run(argv, env=self.env, cwd=self.out, capture_output=True, encoding="utf-8", errors="replace", timeout=timeout or self.timeout)
            res = {"tag": tag, "argv": argv, "exit_code": p.returncode, "seconds": round(time.monotonic() - t0, 3), "stdout": p.stdout, "stderr": p.stderr}
        except subprocess.TimeoutExpired as e:
            res = {"tag": tag, "argv": argv, "exit_code": None, "timeout": True, "seconds": round(time.monotonic() - t0, 3),
                   "stdout": e.stdout if isinstance(e.stdout, str) else "", "stderr": e.stderr if isinstance(e.stderr, str) else ""}
        res["record"] = None
        self.case_of[tag] = self.current
        if json_out and res["stdout"].strip():
            try:
                res["record"] = strict_json(res["stdout"])
                self.records.append((tag, res["record"]))
            except ValueError as e:
                res["parse_error"] = str(e)
        (self.raw / f"{self.seq:02d}-{tag}.json").write_text(json.dumps(res, ensure_ascii=False, indent=2), encoding="utf-8")
        self.commands.append(["cleanmanager", *[RED.text(str(a)) for a in argv[1:]]])
        return res


class Case:
    def __init__(self, cid, title, maps_to, expected):
        self.id, self.title, self.maps_to, self.expected = cid, title, maps_to, expected
        self.problems, self.observed, self.status, self.note = [], {}, None, None

    def need(self, cond, text):
        if not cond:
            self.problems.append(text)
        return bool(cond)

    def finish(self, strict):
        if self.status in ("not_run", "inconclusive"):
            return
        if not self.problems:
            self.status = "passed"
        elif self.id in PENDING and not strict:
            self.status = "known_pending"
        else:
            self.status = "failed"


def rec_of(res):
    return res["record"] if isinstance(res.get("record"), dict) else {}


def check_scan_refusal(case, res, code, entries_res):
    r = rec_of(res)
    issues = [i.get("code") for i in r.get("issues", [])] if r else []
    case.observed.update({"exit_code": res["exit_code"], "status": r.get("status"), "issues": issues, "entries_seen": r.get("entries_seen"),
                          "logical_bytes": r.get("logical_bytes"), "allocated_bytes": r.get("allocated_bytes")})
    case.need(r.get("kind") == "scan_result", f"expected a scan_result record, got {r.get('kind') or res.get('parse_error') or 'no JSON'}")
    case.need(r.get("status") == "failed", f"status {r.get('status')!r}, expected 'failed'")
    case.need(res["exit_code"] == 8, f"exit {res['exit_code']}, expected 8")
    case.need(issues == [code], f"issues {issues}, expected [{code!r}]")
    case.need(r.get("entries_seen") == 0, f"entries_seen {r.get('entries_seen')}, expected 0")
    if entries_res is not None:
        items = rec_of(entries_res).get("items")
        case.observed["indexed_entries"] = None if items is None else len(items)
        case.observed["indexed_kinds"] = [i.get("kind") for i in items or []]
        case.need(items == [], f"entries list returned {case.observed['indexed_kinds']}, expected no rows")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--cli", type=Path, help="CLI binary to test (default: build the release CLI)")
    ap.add_argument("--out", type=Path, help="run folder (default: target/win-verify/<stamp>)")
    ap.add_argument("--strict", action="store_true", help="treat known_pending cases as failed")
    ap.add_argument("--keep-fixture", action="store_true", help="leave the %%TEMP%% fixture in place")
    ap.add_argument("--timeout", type=int, default=180, help="per-command timeout in seconds (default 180)")
    ap.add_argument("--device-id", help="run-record device_id (default dev_win11_x64_primary / dev_win11_arm64)")
    args = ap.parse_args(argv)
    sys.stdout.reconfigure(encoding="utf-8")
    if os.name != "nt":
        print("win_verify.py runs on Windows only (use tools/mac_verify.sh on macOS)", file=sys.stderr)
        return 2

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out = (args.out or ROOT / "target" / "win-verify" / stamp).resolve()
    out.mkdir(parents=True, exist_ok=True)
    started = utc_now()

    # ---- binary
    build_note = None
    if args.cli:
        cli = args.cli.resolve()
    else:
        target_dir = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
        if not target_dir.is_absolute():
            target_dir = ROOT / target_dir
        print("building: cargo build --release --locked -p cm-cli", flush=True)
        p = subprocess.run(["cargo", "build", "--release", "--locked", "-p", "cm-cli"], cwd=ROOT)
        if p.returncode != 0:
            print("CLI build failed", file=sys.stderr)
            return 2
        cli = target_dir / "release" / "cleanmanager.exe"
        build_note = "cargo build --release --locked -p cm-cli"
    if not cli.is_file():
        print(f"CLI not found: {cli}", file=sys.stderr)
        return 2

    # ---- schema tools (same setup as tools/validate_docs.py)
    validator = semantics = None
    schema_error = None
    sys.path.insert(0, str(ROOT / "tools"))
    try:
        import validate_docs as vd  # inserts .research/python-deps and imports jsonschema
        validator = vd.Draft202012Validator(vd.load(ROOT / "schemas/v1/protocol.schema.json"), format_checker=vd.FormatChecker())
        semantics = vd.semantics
    except ImportError as e:
        schema_error = f"{e} (install with: python -m pip install --target .research/python-deps -r requirements-docs.txt)"

    # ---- fixture under %TEMP%
    tmp = Path(os.path.realpath(tempfile.gettempdir()))
    fx = tmp / f"cm-win-verify-{stamp}-{uuid.uuid4().hex[:6]}"
    inp, sentinel, jtarget, junction = fx / "input", fx / "sentinel", fx / "junction-target", fx / "junction-root"
    tree, wide = fx / "cancel-tree", fx / "cancel-wide"
    print(f"fixture: {RED.text(str(fx))}", flush=True)
    for d in INPUT_DIRS:
        (inp / d).mkdir(parents=True, exist_ok=True)
    for rel, size, byte in INPUT_FILES:
        (inp / rel).parent.mkdir(parents=True, exist_ok=True)
        (inp / rel).write_bytes(byte * size)
    sentinel.mkdir(parents=True)
    (sentinel / "do-not-touch.txt").write_text("outside every scan scope: must stay unchanged", encoding="utf-8")
    jtarget.mkdir()
    (jtarget / "behind-the-link.txt").write_bytes(b"j" * 512)
    junction_error = make_junction(junction, jtarget)
    for i in range(TREE_DIRS):
        d = tree / f"d{i:03d}"
        d.mkdir(parents=True)
        for j in range(TREE_FILES_PER_DIR):
            (d / f"f{j:03d}.bin").touch()
    wide.mkdir()
    for j in range(WIDE_FILES):
        (wide / f"w{j:05d}.bin").touch()
    before = {"input": manifest(inp), "sentinel": manifest(sentinel), "junction_target": manifest(jtarget),
              "tree_files": count_files(tree), "wide_files": count_files(wide)}
    assert before["input"]["logical_bytes"] == INPUT_BYTES

    run = Run(cli, out, args.timeout)
    cases = {cid: Case(cid, t, m, e) for cid, t, m, e in CASES}

    def C(cid):  # commands issued from here on belong to this case (for WV-15 attribution)
        run.current = cid
        return cases[cid]
    aborted = None

    try:
        # WV-01 help and doctor
        c = C("WV-01")
        h = run.cli_run("help", "help", "--format", "text", json_out=False)
        c.observed["help_exit_code"] = h["exit_code"]
        c.need(h["exit_code"] == 0 and len(h["stdout"].strip()) > 100, "help --format text did not print help text with exit 0")
        c.need(not h["stdout"].lstrip().startswith("{"), "help --format text printed JSON")
        d = run.cli_run("doctor", "doctor")
        dr = rec_of(d)
        leaks = RED.leaks(d["stdout"])
        c.observed.update({"doctor_exit_code": d["exit_code"], "doctor_redacted": dr.get("redacted"), "doctor_leaks": leaks,
                           "doctor_checks": {x.get("check"): x.get("status") for x in dr.get("checks", [])}})
        c.need(d["exit_code"] == 0 and dr.get("kind") == "doctor_report", "doctor did not return a doctor_report with exit 0")
        c.need(dr.get("redacted") is True, "doctor report is not marked redacted")
        c.need(not leaks, f"doctor stdout contains {leaks}")

        # WV-02 default off
        c = C("WV-02")
        e = run.cli_run("experimental-default", "experimental")
        feats = rec_of(e).get("capabilities", [])
        caps = run.cli_run("capabilities-default", "capabilities")
        states = [x.get("state") for x in rec_of(caps).get("capabilities", [])]
        st = run.cli_run("status-gate-default", "status")
        c.observed.update({"experimental_enabled": [x.get("capability") for x in feats if x.get("enabled")], "capability_states": {s: states.count(s) for s in set(states)},
                           "status": {"exit_code": st["exit_code"], "code": rec_of(st).get("code")}})
        c.need(feats and not any(x.get("enabled") for x in feats), "experimental capabilities are not all disabled in a fresh store")
        c.need(states and "available" not in states, "a capability reports available (or capabilities returned nothing)")
        c.need(st["exit_code"] == 7 and rec_of(st).get("code") == "unsupported", "status was not refused with unsupported / exit 7")

        # WV-03 fixture scan
        c = C("WV-03")
        s = run.cli_run("scan-fixture", "scan", "--root", inp)
        sr = rec_of(s)
        scan_id = sr.get("scan_id")
        c.observed.update({k: sr.get(k) for k in ("status", "completeness", "entries_seen", "logical_bytes", "skipped_ranges", "unknown_size_count")} | {"exit_code": s["exit_code"], "issues": [i.get("code") for i in sr.get("issues", [])]})
        c.need(s["exit_code"] == 0 and sr.get("status") == "completed" and sr.get("completeness") == "verified_scope", "fixture scan is not completed / verified_scope / exit 0")
        c.need(sr.get("entries_seen") == INPUT_ENTRIES, f"entries_seen {sr.get('entries_seen')}, expected {INPUT_ENTRIES}")
        c.need(sr.get("logical_bytes") == str(INPUT_BYTES), f"logical_bytes {sr.get('logical_bytes')!r}, expected '{INPUT_BYTES}'")
        c.need(sr.get("issues") == [] and sr.get("skipped_ranges") == 0, "fixture scan has issues or skipped ranges")
        if scan_id:
            sh = run.cli_run("scan-show", "scan", "show", scan_id)
            same = {k: v for k, v in rec_of(sh).items() if k != "request_id"} == {k: v for k, v in sr.items() if k != "request_id"}
            c.observed["scan_show_same_record"] = same
            c.need(same, "scan show differs from the scan result")

        # WV-04 entries
        c = C("WV-04")
        ids = {}
        if not scan_id:
            c.status, c.note = "not_run", "the fixture scan returned no scan_id"
        else:
            full = run.cli_run("entries-all", "entries", "list", "--scan", scan_id, "--limit", "500", "--sort", "name_asc")
            items = rec_of(full).get("items") or []
            for it in items:
                disp = Path(it.get("path", {}).get("display", ""))
                try:
                    rel = disp.relative_to(inp).as_posix()
                except ValueError:
                    rel = "."
                ids[rel] = it
            page_ids, cursor, pages = [], None, 0
            while pages < 20:
                pages += 1
                pargs = ["entries", "list", "--scan", scan_id, "--limit", "2", "--sort", "name_asc"] + (["--cursor", cursor] if cursor else [])
                pg = rec_of(run.cli_run(f"entries-page-{pages}", *pargs))
                page_ids += [x.get("entry_id") for x in pg.get("items", [])]
                c.need(len(pg.get("items", [])) <= 2, f"page {pages} holds more than 2 items")
                cursor = pg.get("next_cursor")
                if not cursor:
                    break
            wrong_sizes = {rel: (ids.get(rel) or {}).get("logical_bytes") for rel, size, _ in INPUT_FILES if (ids.get(rel) or {}).get("logical_bytes") != str(size)}
            bad_bytes = [x.get("entry_id") for x in items for k in ("logical_bytes", "allocated_bytes") if not (x.get(k) is None or (isinstance(x.get(k), str) and x[k].isdigit()))]
            zh = rec_of(run.cli_run("entries-name-zh", "entries", "list", "--scan", scan_id, "--name", "中文")).get("items")
            none = rec_of(run.cli_run("entries-name-absent", "entries", "list", "--scan", scan_id, "--name", "no-such-name-win-verify")).get("items")
            l0 = run.cli_run("entries-limit-0", "entries", "list", "--scan", scan_id, "--limit", "0")
            l501 = run.cli_run("entries-limit-501", "entries", "list", "--scan", scan_id, "--limit", "501")
            c.observed.update({"pages": pages, "page_entry_ids": page_ids, "total": len(items), "wrong_file_sizes": wrong_sizes, "bad_byte_fields": bad_bytes,
                               "chinese_filter_items": None if zh is None else len(zh), "absent_filter_items": None if none is None else len(none),
                               "limit_0": {"exit_code": l0["exit_code"], "code": rec_of(l0).get("code")}, "limit_501": {"exit_code": l501["exit_code"], "code": rec_of(l501).get("code")}})
            c.need(len(items) == INPUT_ENTRIES, f"entries list returned {len(items)} rows, expected {INPUT_ENTRIES}")
            c.need(len(page_ids) == len(set(page_ids)) == INPUT_ENTRIES and set(page_ids) == {x.get("entry_id") for x in items}, "paging duplicated or missed entries")
            c.need(not wrong_sizes, f"file sizes differ from the fixture manifest: {wrong_sizes}")
            c.need(not bad_bytes, "byte fields that are neither decimal strings nor null")
            c.need(zh is not None and len(zh) == 1 and zh[0].get("logical_bytes") == "2048", "name filter 中文 did not return exactly the 2048-byte file")
            c.need(none == [], "an absent name returned rows")
            for name, r in (("limit 0", l0), ("limit 501", l501)):
                c.need(r["exit_code"] == 2 and rec_of(r).get("code") == "invalid_argument", f"{name} was not invalid_argument / exit 2")

        # WV-05..WV-08 refused roots
        failed_results = []
        c = C("WV-05")
        m = run.cli_run("scan-missing-root", "scan", "--root", inp / "missing-directory")
        check_scan_refusal(c, m, "root_unavailable", run.cli_run("entries-missing-root", "entries", "list", "--scan", rec_of(m)["scan_id"]) if rec_of(m).get("scan_id") else None)
        failed_results.append(("missing", m))
        c = C("WV-06")
        f = run.cli_run("scan-file-root", "scan", "--root", inp / "a.txt")
        check_scan_refusal(c, f, "not_a_directory", run.cli_run("entries-file-root", "entries", "list", "--scan", rec_of(f)["scan_id"]) if rec_of(f).get("scan_id") else None)
        failed_results.append(("file", f))
        c = C("WV-07")
        if junction_error or not is_junction(junction):
            c.status, c.note = "not_run", f"could not create the junction fixture: {junction_error}"
        else:
            j = run.cli_run("scan-junction-root", "scan", "--root", junction)
            check_scan_refusal(c, j, "root_is_link", run.cli_run("entries-junction-root", "entries", "list", "--scan", rec_of(j)["scan_id"]) if rec_of(j).get("scan_id") else None)
            failed_results.append(("junction", j))
        c = C("WV-08")
        sizes = {}
        for name, r in failed_results:
            rr = rec_of(r)
            sizes[name] = {"status": rr.get("status"), "logical_bytes": rr.get("logical_bytes"), "allocated_bytes": rr.get("allocated_bytes")}
            if rr.get("status") == "failed":
                c.need(rr.get("logical_bytes") is None and rr.get("allocated_bytes") is None, f"{name} root: failed scan reports logical_bytes={rr.get('logical_bytes')!r}, allocated_bytes={rr.get('allocated_bytes')!r}")
        c.observed["failed_scan_sizes"] = sizes
        if not any(v["status"] == "failed" for v in sizes.values()):
            c.status, c.note = "not_run", "no refused root produced a failed scan to check"

        # WV-13 (first half) + WV-09 + WV-10 need the gated tools
        run.current = "WV-13"
        on = run.cli_run("experimental-on", "experimental", "--state", "on")
        c = C("WV-09")
        sec = ids.get(".ssh/id_ed25519")
        if not scan_id or not sec:
            c.status, c.note = "not_run", "no entry id for .ssh/id_ed25519 (fixture scan or entries list failed)"
        else:
            ins = rec_of(run.cli_run("inspect-credential", "trash", "inspect", "--scan", scan_id, "--entry", sec["entry_id"]))
            pl = run.cli_run("plan-credential", "trash", "--scan", scan_id, "--entry", sec["entry_id"])
            pc = rec_of(run.cli_run("protect-check-credential", "protect", "check", "--path", inp / ".ssh" / "id_ed25519"))
            c.observed.update({"inspect": {"kind": ins.get("kind"), "available_actions": ins.get("available_actions"), "protected_reasons": [x.get("reason") for x in ins.get("protected") or []],
                                           "refusal": (ins.get("refusal") or {}).get("code")},
                               "plan": {"exit_code": pl["exit_code"], "code": rec_of(pl).get("code")}, "protect_check": {"decision": pc.get("decision"), "layer": pc.get("layer")}})
            c.need(ins.get("kind") == "trash_inspection" and ins.get("protected"), "inspect lists no protection reason")
            c.need("move_to_trash" not in (ins.get("available_actions") or []), "inspect offers move_to_trash for a credential")
            c.need((ins.get("refusal") or {}).get("code") == "policy_denied", "inspect does not refuse with policy_denied")
            c.need(pl["exit_code"] == 3 and rec_of(pl).get("code") == "policy_denied", "planning the credential was not policy_denied / exit 3")
            c.need(pc.get("decision") == "denied", "protect check did not decide denied")
        c = C("WV-10")
        big = ids.get("big.bin")
        if not scan_id or not big:
            c.status, c.note = "not_run", "no entry id for big.bin"
        else:
            pr = run.cli_run("plan-big-bin", "trash", "--scan", scan_id, "--entry", big["entry_id"])
            plan = rec_of(pr)
            body = plan.get("body") or {}
            if plan.get("kind") != "plan":
                c.need(False, f"trash plan returned {plan.get('kind')} {plan.get('code')} / exit {pr['exit_code']}")
            else:
                ttl = (datetime.fromisoformat(body["expires_at"].replace("Z", "+00:00")) - datetime.fromisoformat(body["created_at"].replace("Z", "+00:00"))).total_seconds()
                show = rec_of(run.cli_run("plan-show", "plan", "show", body["plan_id"]))
                ap_ = run.cli_run("plan-apply-unapproved", "plan", "apply", body["plan_id"])
                ops = rec_of(run.cli_run("operations-after-apply", "operations", "list")).get("items")
                c.observed.update({"plan_kind": body.get("plan_kind"), "action_types": [i.get("action_type") for i in body.get("items", [])], "authorization_status": plan.get("authorization_status"),
                                   "policy_matchable": body.get("policy_matchable"), "ttl_seconds": ttl, "show_same": show.get("body") == body and show.get("plan_digest") == plan.get("plan_digest"),
                                   "apply": {"exit_code": ap_["exit_code"], "code": rec_of(ap_).get("code")}, "operations": None if ops is None else len(ops)})
                c.need(c.observed["action_types"] == ["fs_trash"] and body.get("plan_kind") == "manual_selection", "plan is not one manual_selection fs_trash item")
                c.need(plan.get("authorization_status") == "required" and body.get("policy_matchable") is False, "plan does not require approval or is policy-matchable")
                c.need(ttl == PLAN_TTL_SECONDS, f"plan TTL {ttl} s, expected {PLAN_TTL_SECONDS}")
                c.need(c.observed["show_same"], "plan show returned a different body or digest")
                c.need(ap_["exit_code"] == 3 and rec_of(ap_).get("code") == "needs_approval", "unapproved apply was not needs_approval / exit 3")
                c.need(ops == [], "operation history is not empty after the refused apply")

        # WV-11 / WV-12 cancellation
        for cid, root, total in (("WV-11", tree, TREE_ENTRIES), ("WV-12", wide, WIDE_ENTRIES)):
            c = C(cid)
            dt = run.cli_run(f"{cid.lower()}-detach", "scan", "--root", root, "--detach")
            sid = rec_of(dt).get("scan_id")
            if not sid:
                c.need(False, f"scan --detach returned no scan_id (exit {dt['exit_code']})")
                continue
            rc = rec_of(run.cli_run(f"{cid.lower()}-cancel", "scan", "cancel", sid))
            wt = run.cli_run(f"{cid.lower()}-wait", "scan", "wait", sid, "--timeout", "120", timeout=150)
            wr = rec_of(wt)
            kept = rec_of(run.cli_run(f"{cid.lower()}-retained", "entries", "list", "--scan", sid, "--limit", "2")).get("items")
            c.observed.update({"receipt": {"accepted": rc.get("accepted"), "state_at_receipt": rc.get("state_at_receipt")},
                               "terminal": {"exit_code": wt["exit_code"], "kind": wr.get("kind"), "status": wr.get("status"), "completeness": wr.get("completeness"), "entries_seen": wr.get("entries_seen")},
                               "retained_page_items": None if kept is None else len(kept), "total_entries": total,
                               "stopped_before_listing_finished": isinstance(wr.get("entries_seen"), int) and wr["entries_seen"] < total})
            if rc.get("accepted") is False:
                c.status, c.note = "inconclusive", f"the scan was already terminal ({rc.get('state_at_receipt')}) when the cancel arrived; rerun on a slower disk or larger fixture"
                continue
            c.need(rc.get("kind") == "scan_cancel_receipt" and rc.get("accepted") is True, "cancel was not accepted")
            c.need(wr.get("status") == "cancelled" and wt["exit_code"] == 130, f"terminal {wr.get('status')} / exit {wt['exit_code']}, expected cancelled / 130")
            c.need(wr.get("completeness") == "partial_scope", "cancelled scan does not report partial_scope")
            c.need(isinstance(wr.get("entries_seen"), int) and 0 < wr["entries_seen"] <= total, f"entries_seen {wr.get('entries_seen')} outside (0, {total}]")
            c.need(bool(kept), "no retained rows are queryable after the cancel")

        # WV-13 second half
        c = C("WV-13")
        on_feats = rec_of(on).get("capabilities", [])
        caps_on = [x.get("state") for x in rec_of(run.cli_run("capabilities-on", "capabilities")).get("capabilities", [])]
        off = rec_of(run.cli_run("experimental-off", "experimental", "--state", "off")).get("capabilities", [])
        st2 = run.cli_run("status-gate-off", "status")
        c.observed.update({"on_enabled": sum(bool(x.get("enabled")) for x in on_feats), "on_total": len(on_feats), "gate_passed_any": any(x.get("gate_passed") for x in on_feats),
                           "capability_states_on": {s: caps_on.count(s) for s in set(caps_on)}, "off_enabled": sum(bool(x.get("enabled")) for x in off),
                           "status_after_off": {"exit_code": st2["exit_code"], "code": rec_of(st2).get("code")}})
        c.need(on_feats and all(x.get("enabled") for x in on_feats), "experimental --state on did not enable every gated capability")
        c.need(not any(x.get("gate_passed") for x in on_feats), "a gated capability claims its release gate passed")
        c.need(caps_on and "available" not in caps_on, "a capability reports available with experimental features on")
        c.need(off and not any(x.get("enabled") for x in off), "experimental --state off left a capability enabled")
        c.need(st2["exit_code"] == 7 and rec_of(st2).get("code") == "unsupported", "status after --state off was not unsupported / exit 7")

        # WV-14 integrity and no execution
        c = C("WV-14")
        ops = rec_of(run.cli_run("operations-final", "operations", "list")).get("items")
        q = rec_of(run.cli_run("recovery-final", "recovery", "list")).get("items")
        after = {"input": manifest(inp), "sentinel": manifest(sentinel), "junction_target": manifest(jtarget), "tree_files": count_files(tree), "wide_files": count_files(wide)}
        c.observed.update({"input_sha256": canon_sha(before["input"]), "unchanged": {k: before[k] == after[k] for k in before}, "operations": None if ops is None else len(ops),
                           "quarantine": None if q is None else len(q), "approvals_requested": 0})
        for k in before:
            c.need(before[k] == after[k], f"{k} changed during the run")
        c.need(ops == [], "operation history is not empty")
        c.need(q == [], "quarantine is not empty")
        c.need(not any(a[1:3] == ["plan", "approve"] or a[1:3] == ["plan", "review"] for a in run.commands), "a plan approval command was run")
    except Exception:  # keep what was observed; the report says the run aborted
        import traceback
        aborted = RED.text(traceback.format_exc())
        print(aborted, file=sys.stderr)
    finally:
        # ---- WV-15 schema over everything collected
        c = C("WV-15")
        bad = []
        if validator is None:
            c.status, c.note = "not_run", "jsonschema unavailable: " + str(schema_error)
        else:
            from jsonschema.exceptions import best_match
            for tag, r in run.records:
                err = best_match(validator.iter_errors(r))
                errs = [f"{err.json_path}: {err.message}"] if err is not None else []
                if not errs:
                    try:
                        semantics(r)
                    except Exception as ex:  # semantic invariant from validate_docs
                        errs = [f"semantic invariant: {type(ex).__name__}: {ex}"]
                if errs:
                    failed_scan_bytes = r.get("kind") == "scan_result" and r.get("status") == "failed" and (r.get("logical_bytes") is not None or r.get("allocated_bytes") is not None)
                    bad.append({"record": tag, "case": run.case_of.get(tag), "failed_scan_with_bytes": failed_scan_bytes, "errors": [RED.text(x)[:300] for x in errs]})
            c.observed.update({"records_checked": len(run.records), "invalid": bad})
            c.need(run.records, "no JSON records were collected")
        for cs in cases.values():
            if cs.status is None and not cs.problems and not cs.observed:
                cs.status, cs.note = "not_run", cs.note or ("the run aborted before this case" if aborted else "the run stopped before this case")
        # ---- cleanup
        if not args.keep_fixture:
            try:
                if is_junction(junction):
                    os.rmdir(junction)  # removes the link only, never its target
                shutil.rmtree(fx)
            except OSError as e:
                print(f"fixture cleanup failed: {e}", file=sys.stderr)

    for cs in cases.values():
        if cs.id != "WV-15":
            cs.finish(args.strict)
    # WV-15: a record that is invalid only because of a fix still pending (its case is known_pending,
    # or it is a failed scan still reporting byte counts while WV-08 is pending) is attributed to that
    # case instead of failing the schema check twice.
    c = cases["WV-15"]
    pending_now = {cs.id for cs in cases.values() if cs.status == "known_pending"}
    own, attributed = [], []
    for b in c.observed.get("invalid", []):
        explained = b["case"] in pending_now or (b["failed_scan_with_bytes"] and "WV-08" in pending_now)
        (attributed if explained else own).append(b["record"])
    if own:
        c.need(False, f"{len(own)} record(s) failed schema or semantic validation: {', '.join(own)}")
    if attributed:
        c.observed["attributed_to_pending"] = attributed
        if args.strict:
            c.need(False, f"{len(attributed)} record(s) invalid because of pending fixes: {', '.join(attributed)}")
        else:
            c.note = f"{len(attributed)} record(s) invalid only because of known-pending fixes: {', '.join(attributed)}"
    c.finish(args.strict)
    return report(args, run, cases, out, cli, started, stamp, fx, build_note, before, aborted)


def report(args, run, cases, out, cli, started, stamp, fx, build_note, before, aborted) -> int:
    counts = {}
    for cs in cases.values():
        counts[cs.status] = counts.get(cs.status, 0) + 1
    if counts.get("failed") or aborted:
        code = 1
    elif any(counts.get(s) for s in ("known_pending", "inconclusive", "not_run")):
        code = 3
    else:
        code = 0
    osf = os_facts()
    head = git("rev-parse", "HEAD")
    dirty = git("status", "--porcelain", "--untracked-files=no")
    binary_sha = sha256_file(cli)
    elev = elevated()
    device = args.device_id or ("dev_win11_arm64" if osf["architecture"] == "arm64" else "dev_win11_x64_primary")
    results = {
        "tool": "tools/win_verify.py", "started_at": started, "finished_at": utc_now(), "exit_code": code, "strict": args.strict,
        "code_commit": head, "working_tree_modified": bool(dirty), "binary_sha256": binary_sha, "binary": RED.text(str(cli)), "built_with": build_note,
        "os": osf, "elevated": elev, "fixture": RED.text(str(fx)), "fixture_kept": args.keep_fixture,
        "fixture_manifest_sha256": canon_sha(before["input"]), "counts": counts, "aborted": aborted,
        "cases": [{"id": cs.id, "title": cs.title, "maps_to": cs.maps_to, "expected": cs.expected, "pending_fix": PENDING.get(cs.id),
                   "status": cs.status, "problems": [RED.text(p) for p in cs.problems], "note": cs.note and RED.text(cs.note),
                   "observed": RED.value(cs.observed)} for cs in cases.values()],
        "commands": run.commands,
    }
    (out / "results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")

    status = {0: "passed", 1: "failed", 3: "incomplete"}[code]
    limitations = [
        "Hosted on the machine that ran tools/win_verify.py; CLI only: desktop window, Narrator/keyboard journeys, real trash/quarantine moves, restore, uninstall, startup, maintenance, signing and other devices are not covered.",
        f"Synthetic fixture under %TEMP% ({INPUT_ENTRIES} entries, {INPUT_BYTES} bytes, fake .ssh credential, junction, {TREE_DIRS}x{TREE_FILES_PER_DIR} tree, {WIDE_FILES}-file directory); own CM_STORE_DIR.",
        "Working tree had uncommitted changes when the binary was tested; code_commit is HEAD, not the exact source." if dirty else "Working tree clean at code_commit (tracked files).",
        "Redacted with the Doctor rules (home -> ~, user -> <user>, host -> <host>); raw/ in the run folder is not redacted and is not part of this record.",
    ]
    if counts.get("known_pending"):
        limitations.append("known_pending cases expect fixes landing on 2026-10-22: " + ", ".join(f"{cs.id} ({PENDING[cs.id]})" for cs in cases.values() if cs.status == "known_pending"))
    if elev:
        limitations.append("Run from an elevated process; ordinary-user behaviour is not shown by this record.")
    if aborted:
        limitations.append("The run aborted with an unexpected error before every case finished; see results.json aborted.")
    actual = f"{len(cases)} cases: " + ", ".join(f"{n} {s}" for s, n in sorted(counts.items())) + "; " + "; ".join(
        f"{cs.id} {cs.status}" + (f" ({cs.problems[0]})" if cs.problems else "") for cs in cases.values() if cs.status != "passed")
    record = {
        "schema_version": "1.0", "test_id": "SCN-07", "run_id": "run_win_verify_" + stamp.lower(), "recorded_at": results["finished_at"],
        "code_commit": head if head and re.fullmatch(r"[0-9a-f]{40}", head) else None, "binary_sha256": binary_sha, "device_id": device,
        "os": osf, "filesystem": {"type": filesystem_of(fx), "volume_id": None},
        "permission_context": {"elevated": elev, "signing_identity": None, "grants": []},
        "scope_digest": None, "fixture_manifest_sha256": results["fixture_manifest_sha256"], "backend": "win.directory",
        "config": {"tool": "tools/win_verify.py", "strict": args.strict, "per_command_timeout_s": args.timeout, "cases": {cs.id: cs.maps_to for cs in cases.values()},
                   "pending_fixes": PENDING, "source_cases": "docs/acceptance/windows-2026-10-22-cases.md"},
        "command": ["python", "-I", "tools/win_verify.py", *(["--cli", RED.text(str(cli))] if args.cli else []), *(["--strict"] if args.strict else [])],
        "seed": None,
        "expected": "Every case in config.cases meets the expectation in results.json (written in tools/win_verify.py before the run); fixture, sentinel and junction target unchanged; nothing approved or executed.",
        "actual": RED.text(actual)[:4000], "terminal_state": status, "exit_code": code,
        "measurements": {"counts": counts, "cases": {cs.id: {"status": cs.status, "problems": [RED.text(p) for p in cs.problems]} for cs in cases.values()},
                         "json_records_checked": len(run.records)},
        "observer": "tools/win_verify.py", "limitations": limitations, "status": status,
    }
    (out / "run-record.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")

    mark = {"passed": "pass", "failed": "**FAIL**", "known_pending": "known pending", "inconclusive": "inconclusive", "not_run": "not run"}
    lines = [
        f"# CleanManager Windows verification — {stamp}", "",
        f"- commit: `{head}`{' (working tree modified)' if dirty else ''}",
        f"- binary: `{RED.text(str(cli))}` sha256 `{binary_sha}`" + (f" (built: {build_note})" if build_note else ""),
        f"- OS: {osf['name']} build {osf['build']}, {osf['architecture']}; elevated: {elev}",
        f"- fixture: `{RED.text(str(fx))}` ({'kept' if args.keep_fixture else 'removed'}); store: run folder `store/`",
        f"- result: **{status}** (exit {code}{', strict' if args.strict else ''}) — " + ", ".join(f"{n} {s}" for s, n in sorted(counts.items())), "",
        "| case | maps to | result | details |", "| --- | --- | --- | --- |",
    ]
    for cs in cases.values():
        detail = "; ".join(cs.problems) or cs.note or ""
        if cs.id in PENDING:
            label = "fix present" if cs.status == "passed" else "pending fix"
            detail = (detail + " — " if detail else "") + f"{label}: {PENDING[cs.id]}"
        lines.append(f"| {cs.id} {cs.title} | {cs.maps_to} | {mark.get(cs.status, cs.status)} | {RED.text(detail).replace('|', '/')} |")
    lines += ["",
              "known pending = the expectation is a fix landing on 2026-10-22; on an older binary the case shows here instead of passing. "
              "Use `--strict` once the fixes are in to make them fail the run.", "",
              "## Files", "",
              "- `results.json`, `run-record.json` and this summary are redacted (home `~`, user `<user>`, host `<host>`).",
              "- `raw/` keeps every command's unredacted stdout/stderr for local debugging; do not publish it.", "",
              "## How to submit", "",
              "Paste this summary into an issue and attach `run-record.json`. To register the run, a maintainer copies "
              "`run-record.json` to `docs/evidence/runs/<run_id>.json` (it follows schemas/v1/run-record.schema.json) and runs "
              "`python -I tools/validate_docs.py`. A passed record does not change any device-matrix claim by itself."]
    (out / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print("\n".join(lines[:lines.index("| --- | --- | --- | --- |") + 1 + len(cases)]))
    print(f"\nrun folder: {RED.text(str(out))}")
    return code


if __name__ == "__main__":
    sys.exit(main())
