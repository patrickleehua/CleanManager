"""Generate docs/dependencies.md from `cargo metadata --locked`; never downloads into the source tree."""
from __future__ import annotations
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Why each direct third-party dependency exists. Transitive crates list who requires them.
PURPOSE = {
    "serde": "Typed (de)serialization of protocol records, manifests and rules",
    "serde_json": "Strict JSON input/output for the machine protocol (CLI-03/04)",
    "sha2": "SHA-256 for plan digests (CM-C14N-v1) and fixture/sentinel hashes",
    "base64": "Lossless raw path encoding (UTF-16LE / raw bytes) in protocol records",
    "thiserror": "Typed error enums in cm-core and cm-store",
    "rusqlite": "SQLite access for the rebuildable scan index and the separate audit store (bundled SQLite, no system library)",
    "windows-sys": "Windows API bindings for process liveness, scan backends and verified file actions",
    "windows": "COM bindings for the Shell IFileOperation recycle-bin action and its progress sink",
    "windows-core": "Required by the windows crate COM implement macro (referenced by name)",
    "libc": "macOS system calls (process liveness via kill/proc_pidinfo, later POSIX scan backend)",
    "ureq": "Blocking HTTP client for the optional AI diagnosis API adapters and the person-initiated CLI self-update download (native-tls: OS TLS stack and trust store; diagnosis: no redirects, no retries; self-update: https only, at most 5 redirects, size-bounded)",
    "ed25519-dalek": "Ed25519 signature verification of the CLI self-update manifest with release keys embedded at build time (LIF-01); vetted dalek-cryptography implementation, verification only (no default features, no key generation)",
}


def main():
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked"],
                                     cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    members = set(meta["workspace_members"])
    packages = {p["id"]: p for p in meta["packages"]}
    required_by: dict[str, set[str]] = {}
    for node in meta["resolve"]["nodes"]:
        for dep in node["deps"]:
            required_by.setdefault(dep["pkg"], set()).add(packages[node["id"]]["name"])
    direct = {packages[d["pkg"]]["name"] for n in meta["resolve"]["nodes"] if n["id"] in members for d in n["deps"]
              if d["pkg"] not in members}
    rows = []
    for pid, p in sorted(packages.items(), key=lambda kv: (kv[1]["name"], kv[1]["version"])):
        if pid in members:
            continue
        name = p["name"]
        purpose = PURPOSE.get(name) if name in direct else "Transitive: required by " + ", ".join(sorted(required_by.get(pid, [])))
        if purpose is None:
            raise SystemExit(f"direct dependency {name} has no documented purpose in tools/dependency_inventory.py")
        source = "crates.io" if (p["source"] or "").startswith("registry+https://github.com/rust-lang/crates.io-index") else p["source"]
        rows.append(f"| `{name}` | {p['version']} | {p['license']} | {source} | {purpose} |")
    toolchain = (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8").split('channel = "')[1].split('"')[0]
    text = f"""# 依赖与工具链清单

本文件由 `python tools/dependency_inventory.py` 依据 `Cargo.lock` 生成；文档验证器会比对两者，手工修改会被拒绝。

- Rust 工具链：`{toolchain}`（见 [rust-toolchain.toml](../rust-toolchain.toml)），workspace 声明的最低版本见根 `Cargo.toml` 的 `rust-version`。
- 文档验证依赖：[requirements-docs.txt](../requirements-docs.txt)（`jsonschema`，MIT），只安装到被忽略的 `.research/python-deps`。
- 不捆绑第三方二进制：构建只引用 workspace 内 crate 与下表 crates.io 包；`.research/` 中的 WizTree 与 Mole 研究副本被 Git 忽略，任何 `Cargo.toml` 都不得引用它们（验证器检查）。
- 本项目自身许可证尚未决定，crate 暂标 `UNLICENSED`；发布前需在 [分发与许可](distribution-and-licensing.md) 中确定并生成 notices。

| Crate | 版本 | 许可 | 来源 | 用途 |
| --- | --- | --- | --- | --- |
""" + "\n".join(rows) + "\n"
    (ROOT / "docs/dependencies.md").write_text(text, encoding="utf-8")
    print(f"Documented {len(rows)} locked third-party crates.")
    desktop()


# Direct dependencies of the desktop host (apps/desktop): Tauri crates and npm packages.
DESKTOP_PURPOSE = {
    "tauri": "Desktop shell: native window, IPC commands to the shared Rust core, capability-scoped permissions; tray-icon feature for the optional resident tray (RES-01, off by default)",
    "tauri-build": "Build-time generation of the Tauri context, permissions and Windows resources",
    "tauri-plugin-dialog": "Native folder picker for choosing scan scopes (no file contents read)",
    "serde": "Typed (de)serialization of IPC payloads",
    "serde_json": "JSON values passed between the core and the webview",
    "@tauri-apps/api": "Webview side of the Tauri IPC bridge",
    "@tauri-apps/cli": "Development/build CLI for the Tauri app (not shipped)",
    "typescript": "Type checking of the webview sources (not shipped)",
    "vite": "Bundler for the webview assets (not shipped)",
    "vitest": "Unit tests for the webview logic (not shipped)",
}


def desktop():
    """Write docs/dependencies-desktop.md from the desktop Cargo.lock and package-lock.json."""
    app = ROOT / "apps/desktop"
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked"],
                                     cwd=app / "src-tauri", check=True, capture_output=True, text=True).stdout)
    packages = {p["id"]: p for p in meta["packages"]}
    local = {pid for pid, p in packages.items() if p["source"] is None}
    root_ids = set(meta["workspace_members"])
    direct = {packages[d["pkg"]]["name"] for n in meta["resolve"]["nodes"] if n["id"] in root_ids for d in n["deps"]
              if d["pkg"] not in local}
    required_by: dict[str, set[str]] = {}
    for node in meta["resolve"]["nodes"]:
        for dep in node["deps"]:
            required_by.setdefault(dep["pkg"], set()).add(packages[node["id"]]["name"])
    crate_rows = []
    for pid, p in sorted(packages.items(), key=lambda kv: (kv[1]["name"], kv[1]["version"])):
        if pid in local:
            continue
        name = p["name"]
        if name in direct:
            purpose = DESKTOP_PURPOSE.get(name)
            if purpose is None:
                raise SystemExit(f"direct desktop dependency {name} has no documented purpose in tools/dependency_inventory.py")
        else:
            purpose = "Transitive: required by " + ", ".join(sorted(required_by.get(pid, [])))
        source = "crates.io" if (p["source"] or "").startswith("registry+https://github.com/rust-lang/crates.io-index") else p["source"]
        crate_rows.append(f"| `{name}` | {p['version']} | {p['license']} | {source} | {purpose} |")
    lock = json.loads((app / "package-lock.json").read_text(encoding="utf-8"))
    top = lock["packages"][""]
    direct_npm = {**top.get("dependencies", {}), **top.get("devDependencies", {})}
    npm_rows = []
    for key, p in sorted(lock["packages"].items()):
        if not key:
            continue
        name = key.rsplit("node_modules/", 1)[1]
        if name in direct_npm:
            purpose = DESKTOP_PURPOSE.get(name)
            if purpose is None:
                raise SystemExit(f"direct npm dependency {name} has no documented purpose in tools/dependency_inventory.py")
        else:
            purpose = "Transitive"
        scope = "dev" if p.get("dev") else "runtime"
        extra = " (optional, platform-specific)" if p.get("optional") else ""
        npm_rows.append(f"| {name} | {p['version']} | {p.get('license', 'UNKNOWN')} | {scope}{extra} | {purpose} |")
    text = f"""# 桌面端依赖清单

本文件由 `python tools/dependency_inventory.py` 依据 `apps/desktop/src-tauri/Cargo.lock` 与 `apps/desktop/package-lock.json` 生成；文档验证器会比对三者，手工修改会被拒绝。核心与 CLI 的依赖见 [依赖与工具链清单](dependencies.md)。

- Node.js 与 npm 只用于构建 webview 资源与运行前端测试；`dev` 范围的包不进入安装包。
- 桌面端通过路径依赖复用 workspace 内的 `cm-core`、`cm-app`、`cm-store`、`cm-platform`，下表不重复列出它们及其 crates.io 依赖以外的本地 crate。
- 不捆绑第三方二进制，`.research/` 中的研究副本不被任何清单引用（验证器检查）。

## Rust crates（{len(crate_rows)}）

| Crate | 版本 | 许可 | 来源 | 用途 |
| --- | --- | --- | --- | --- |
""" + "\n".join(crate_rows) + f"""

## npm 包（{len(npm_rows)}）

| 包 | 版本 | 许可 | 范围 | 用途 |
| --- | --- | --- | --- | --- |
""" + "\n".join(npm_rows) + "\n"
    (ROOT / "docs/dependencies-desktop.md").write_text(text, encoding="utf-8")
    print(f"Documented {len(crate_rows)} desktop crates and {len(npm_rows)} npm packages.")


def notices():
    """Regenerate docs/third-party-notices.md from the same lock files (MVP 14.3)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("generate_notices", ROOT / "tools" / "generate_notices.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    cli, desktop, npm = mod.collect()
    mod.write_markdown(cli, desktop, npm)
    print(f"Listed {len(cli)} CLI, {len(desktop)} desktop crates and {len(npm)} runtime npm packages in docs/third-party-notices.md.")


if __name__ == "__main__":
    main()
    notices()
