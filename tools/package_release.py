"""Build the standalone CLI release layout and zip under target/release-dist/ (MVP 14.1/14.3).

    python -I tools/package_release.py [--skip-build] [--smoke]
    python -I tools/package_release.py envelope --payload P --signature SIG --public-key-hex HEX [--out OUT]

Package: release build of `cleanmanager` with build-machine paths remapped, the platform rule pack and tested registry, the
full third-party notices, the privacy statement, a short README and `cleanmanager-release.json` (every file with SHA-256/size).
Checks before zipping: no research/evidence/example/test material; no repository path, user profile, user name or Cargo home
in any packaged file (UTF-8 and UTF-16LE); the debug-only test-key variable is absent from the release program.
`--smoke` unpacks the zip into a fresh directory outside the source tree and runs the packaged CLI there without
CM_RULES_DIR: capabilities, a small scan + analyze, `self update --check` and the `self uninstall` preview.
Also writes an unsigned update payload (`*.update-payload.json`, `cleanmanager-update/1`) for the release signer; `envelope`
wraps a detached Ed25519 signature of it. Real code signing and installers are out of scope (MVP 14.2).
"""
from __future__ import annotations
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "target" / "release-dist"
BUILD_DIR = DIST / "build"
EXE = "cleanmanager.exe" if os.name == "nt" else "cleanmanager"
PLATFORM_RULES = "windows" if os.name == "nt" else "macos"
FORBIDDEN_PARTS = {".research", "evidence", "output", "examples", "tests", ".git", "node_modules", "fuzz_corpus"}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def version() -> str:
    return tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]


def target_id() -> str:
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    return f"{'windows' if os.name == 'nt' else 'macos'}-{arch}"


def build() -> Path:
    cargo_home = os.environ.get("CARGO_HOME") or str(Path.home() / ".cargo")
    # rustc applies the last matching remapping: general first, most specific last.
    remaps = [f"--remap-path-prefix={Path.home()}=/home", f"--remap-path-prefix={cargo_home}=/cargo", f"--remap-path-prefix={ROOT}=/cleanmanager"]
    flags = remaps + (["-C", "link-arg=/PDBALTPATH:%_PDB%"] if os.name == "nt" else [])
    # CARGO_ENCODED_RUSTFLAGS (0x1f-separated) keeps paths with spaces intact.
    env = {k: v for k, v in os.environ.items() if k != "RUSTFLAGS"}
    env.update({"CARGO_TARGET_DIR": str(BUILD_DIR), "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "cm-cli"], cwd=ROOT, env=env, check=True)
    return BUILD_DIR / "release" / EXE


def readme(ver: str) -> str:
    return f"""CleanManager command-line tool {ver} ({target_id()})

Run `cleanmanager help --format text` for the command list and `cleanmanager capabilities` for what this build supports.
Nothing touches your files without a plan you reviewed and approved in a terminal.

Install: keep this folder anywhere, or copy it to a folder you choose with
    cleanmanager self install --dir D:\\Tools\\CleanManager [--add-to-path]
(never overwrites files; other `cleanmanager` programs on PATH are reported, not replaced).
Update: cleanmanager self update --check   (signed release manifest; package-manager copies are pointed to their manager)
Uninstall: cleanmanager self uninstall       (shows the plan; keeps settings, history and quarantine by default)

Data stays on this device: see PRIVACY.md. Third-party licenses: THIRD-PARTY-NOTICES.txt.
This package is not code-signed yet; verify cleanmanager-release.json hashes against the published zip checksum.
"""


def layout(exe: Path, ver: str) -> tuple[Path, list[str]]:
    name = f"cleanmanager-{ver}-{target_id()}"
    out = DIST / name
    if out.exists():
        shutil.rmtree(out)
    (out / "rules").mkdir(parents=True)
    shutil.copy2(exe, out / EXE)
    for sub in ["registry", PLATFORM_RULES]:
        dest = out / "rules" / sub
        dest.mkdir()
        for f in sorted((ROOT / "rules" / sub).glob("*.json")):
            shutil.copy2(f, dest / f.name)
    spec = importlib.util.spec_from_file_location("generate_notices", ROOT / "tools" / "generate_notices.py")
    notices = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(notices)
    cli, _desktop, _npm = notices.collect()
    text, missing = notices.full_text(cli, [], f"CleanManager CLI {ver}: third-party notices")
    (out / "THIRD-PARTY-NOTICES.txt").write_text(text, encoding="utf-8", newline="\n")
    shutil.copy2(ROOT / "docs" / "privacy.md", out / "PRIVACY.md")
    (out / "README.txt").write_text(readme(ver), encoding="utf-8", newline="\n")
    files = []
    for p in sorted(x for x in out.rglob("*") if x.is_file()):
        rel = p.relative_to(out).as_posix()
        files.append({"path": rel, "sha256": sha256(p), "size": p.stat().st_size})
    manifest = {"contract": "cleanmanager-release/1", "product": "cleanmanager-cli", "version": ver, "target": target_id(),
                "install_source": "zip", "files": files}
    (out / "cleanmanager-release.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    return out, missing


def privacy_needles() -> list[str]:
    needles = {str(ROOT), str(ROOT).replace("\\", "/"), str(Path.home()), str(Path.home()).replace("\\", "/")}
    cargo_home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    needles.add(str(cargo_home))
    # A home-relative tail of the Cargo home (e.g. after a too-general remap) still describes this machine.
    if cargo_home.is_relative_to(Path.home()) and len(cargo_home.relative_to(Path.home()).parts) > 1:
        needles.add(str(cargo_home.relative_to(Path.home())))
    user = os.environ.get("USERNAME") or os.environ.get("USER") or ""
    if len(user) >= 4:
        needles.add(user)
    return sorted(n for n in needles if n)


def check(out: Path) -> list[str]:
    problems = []
    needles = privacy_needles()
    for p in sorted(x for x in out.rglob("*") if x.is_file()):
        rel = p.relative_to(out)
        if FORBIDDEN_PARTS & {part.lower() for part in rel.parts}:
            problems.append(f"research/evidence/test material packaged: {rel}")
        data = p.read_bytes()
        lower = data.lower()
        for n in needles:
            for label, enc in (("UTF-8", n.lower().encode("utf-8")), ("UTF-16", n.lower().encode("utf-16-le"))):
                if enc in lower:
                    problems.append(f"{rel} contains a build-machine path or user name ({label})")
                    break
        if p.name == EXE:
            for marker in (b"CM_UPDATE_TEST_PUBLIC_KEY", "CM_UPDATE_TEST_PUBLIC_KEY".encode("utf-16-le")):
                if marker in data:
                    problems.append(f"{rel} contains the debug-only test key variable")
    return problems


def make_zip(out: Path) -> Path:
    z = out.parent / (out.name + ".zip")
    if z.exists():
        z.unlink()
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as zf:
        for p in sorted(x for x in out.rglob("*") if x.is_file()):
            info = zipfile.ZipInfo(f"{out.name}/{p.relative_to(out).as_posix()}", date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            zf.writestr(info, p.read_bytes())
    (z.parent / (z.name + ".sha256")).write_text(f"{sha256(z)}  {z.name}\n", encoding="utf-8", newline="\n")
    return z


def update_payload(out: Path, ver: str) -> Path:
    release = json.loads((out / "cleanmanager-release.json").read_text(encoding="utf-8"))
    payload = {"contract": "cleanmanager-update/1", "product": "cleanmanager-cli", "version": ver, "published_at": None,
               "compat": {"protocol_schema": "1.0", "index_schema": 1, "audit_schema": 1},
               "targets": {target_id(): {"files": [{**f, "url": f"files/{f['path']}"} for f in release["files"]]}}}
    p = DIST / f"{out.name}.update-payload.json"
    p.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8", newline="\n")
    return p


def run(exe: Path, args: list[str], cwd: Path, env: dict) -> tuple[int, dict | None, str]:
    p = subprocess.run([str(exe), *args, "--json"], cwd=cwd, env=env, capture_output=True, timeout=300)
    text = p.stdout.decode("utf-8", errors="replace").strip()
    try:
        rec = json.loads(text.splitlines()[-1]) if text else None
    except json.JSONDecodeError:
        rec = None
    return p.returncode, rec, text[:400]


def smoke(z: Path) -> dict:
    """Unpack outside the source tree and use the packaged CLI with no CM_RULES_DIR."""
    work = Path(tempfile.mkdtemp(prefix="cm-release-smoke-"))
    assert not work.resolve().is_relative_to(ROOT), "smoke directory must be outside the source tree"
    with zipfile.ZipFile(z) as zf:
        zf.extractall(work / "unpacked")
    inst = next((work / "unpacked").iterdir())
    exe = inst / EXE
    store = work / "store"
    data = work / "data"
    (data / "sub").mkdir(parents=True)
    (data / "a.log").write_bytes(b"x" * 4096)
    (data / "sub" / "b.tmp").write_bytes(b"y" * 1024)
    env = {k: v for k, v in os.environ.items() if k not in ("CM_RULES_DIR", "CM_STORE_DIR", "CM_UPDATE_TEST_PUBLIC_KEY")}
    env["CM_STORE_DIR"] = str(store)
    results = {"directory_outside_source_tree": True, "cm_rules_dir_set": False, "steps": []}

    def step(name, args, want_kind, want_exit=0):
        code, rec, text = run(exe, args, work, env)
        ok = code == want_exit and rec is not None and rec.get("kind") == want_kind
        results["steps"].append({"name": name, "argv": ["cleanmanager", *args], "exit": code, "kind": rec and rec.get("kind"), "ok": ok})
        if not ok:
            raise SystemExit(f"smoke step {name} failed: exit {code}: {text}")
        return rec

    caps = step("capabilities", ["capabilities"], "capabilities")
    results["engine_version"] = caps["engine_version"]
    scan = step("scan", ["scan", "--root", str(data), "--store", str(store)], "scan_result")
    analysis = step("analyze", ["analyze", "--scan", scan["scan_id"], "--store", str(store)], "analysis_result")
    results["scan"] = {"status": scan["status"], "completeness": scan["completeness"], "entries_seen": scan["entries_seen"]}
    results["analysis_rule_set_digest"] = analysis.get("rule_set_digest") or analysis.get("rules_digest")
    # Without the `rules` folder the program falls back to the pack compiled into it (same digest).
    shutil.move(str(inst / "rules"), str(work / "rules-moved-away"))
    embedded = step("analyze (embedded rule pack)", ["analyze", "--scan", scan["scan_id"], "--store", str(store)], "analysis_result")
    results["embedded_rule_pack_same_digest"] = (embedded.get("rule_set_digest") or embedded.get("rules_digest")) == results["analysis_rule_set_digest"]
    if not results["embedded_rule_pack_same_digest"]:
        raise SystemExit("the embedded rule pack differs from the packaged one")
    shutil.move(str(work / "rules-moved-away"), str(inst / "rules"))
    upd = step("self update --check", ["self", "update", "--check"], "release_info", want_exit=7)
    results["self_update_check"] = {"status": upd["status"], "install_method": upd["install_method"], "reason": upd["reason"]}
    rem = step("self uninstall (preview)", ["self", "uninstall", "--store", str(store)], "self_remove_preview")
    results["self_uninstall_preview"] = {"status": rem["status"], "items": len(rem["remove"]), "keep": [k["data"] for k in rem["keep"]],
                                         "program_files_planned": sum(1 for r in rem["remove"] if r["component"] == "cli")}
    if rem["status"] != "preview" or not exe.exists():
        raise SystemExit("self uninstall preview changed something")
    shutil.rmtree(work, ignore_errors=True)
    return results


def envelope(args) -> None:
    payload = Path(args.payload).read_bytes()
    sig = Path(args.signature).read_bytes()
    if len(sig) != 64:
        raise SystemExit("expected a 64-byte raw Ed25519 signature")
    key = bytes.fromhex(args.public_key_hex)
    if len(key) != 32:
        raise SystemExit("expected a 32-byte Ed25519 public key in hex")
    env = {"contract": "cleanmanager-update-envelope/1", "payload_base64": base64.b64encode(payload).decode(),
           "signatures": [{"key_id": hashlib.sha256(key).hexdigest()[:16], "algorithm": "ed25519", "signature_base64": base64.b64encode(sig).decode()}]}
    out = Path(args.out or str(Path(args.payload).with_suffix(".envelope.json")))
    out.write_text(json.dumps(env, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {out} (signature is verified by the CLI, not here).")


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd")
    env_p = sub.add_parser("envelope")
    env_p.add_argument("--payload", required=True)
    env_p.add_argument("--signature", required=True)
    env_p.add_argument("--public-key-hex", required=True)
    env_p.add_argument("--out")
    ap.add_argument("--skip-build", action="store_true")
    ap.add_argument("--smoke", action="store_true")
    args = ap.parse_args()
    if args.cmd == "envelope":
        envelope(args)
        return 0
    ver = version()
    exe = BUILD_DIR / "release" / EXE if args.skip_build else build()
    out, missing = layout(exe, ver)
    problems = check(out)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    z = make_zip(out)
    payload = update_payload(out, ver)
    summary = {"version": ver, "target": target_id(), "zip": z.name, "zip_sha256": sha256(z), "files": sum(1 for x in out.rglob("*") if x.is_file()),
               "rules": len(list((out / "rules" / PLATFORM_RULES).glob("*.json"))), "registry_entries": len(list((out / "rules" / "registry").glob("*.json"))),
               "notices_without_license_file": missing, "privacy_and_exclusion_checks": "passed",
               "embedded_update_keys": bool(os.environ.get("CM_UPDATE_PUBLIC_KEYS")), "embedded_update_channel": bool(os.environ.get("CM_UPDATE_MANIFEST_URL")),
               "update_payload": payload.name, "signed": False}
    if args.smoke:
        summary["smoke"] = smoke(z)
    (DIST / "package-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
