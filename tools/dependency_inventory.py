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


if __name__ == "__main__":
    main()
