"""Third-party notices for the release packages (MVP 14.3), generated from both dependency inventories.

- `docs/third-party-notices.md`: every third-party component that ships in the CLI package (normal-dependency closure of
  `cm-cli` in the root Cargo.lock) and in the desktop package (closure of the desktop host in apps/desktop/src-tauri/Cargo.lock
  plus the runtime npm packages of apps/desktop/package-lock.json), with version, declared license and source. It records the
  SHA-256 of the three lock files; tools/validate_docs.py rejects it when a lock file changed without regenerating.
- `--full OUT` additionally writes a plain-text NOTICE file with the license/notice files each crate ships (read from the local
  Cargo registry sources; nothing is downloaded), used by tools/package_release.py as THIRD-PARTY-NOTICES.txt.

Usage: python -I tools/generate_notices.py [--full OUT --scope cli|desktop|all]
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / "apps/desktop"
LOCKS = {"cli": ROOT / "Cargo.lock", "desktop": DESKTOP / "src-tauri/Cargo.lock", "npm": DESKTOP / "package-lock.json"}
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense", "authors")


def metadata(cwd: Path) -> dict:
    return json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked"], cwd=cwd, check=True,
                                     capture_output=True, text=True, encoding="utf-8").stdout)


def closure(meta: dict, root_names: set[str]) -> list[dict]:
    """Third-party packages reachable from `root_names` through normal (non-dev, non-build) dependencies."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    todo = [pid for pid, p in packages.items() if p["name"] in root_names and p["source"] is None]
    seen = set(todo)
    while todo:
        node = nodes[todo.pop()]
        for dep in node["deps"]:
            if not any(k.get("kind") is None for k in dep["dep_kinds"]):
                continue
            if dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    return sorted((packages[i] for i in seen if packages[i]["source"] is not None), key=lambda p: (p["name"], p["version"]))


def npm_runtime() -> list[dict]:
    lock = json.loads(LOCKS["npm"].read_text(encoding="utf-8"))
    out = []
    for key, p in sorted(lock["packages"].items()):
        if not key or p.get("dev"):
            continue
        out.append({"name": key.rsplit("node_modules/", 1)[1], "version": p["version"], "license": p.get("license", "UNKNOWN"), "key": key})
    return out


def lock_digests() -> dict[str, str]:
    return {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in LOCKS.items()}


def source(p: dict) -> str:
    return p.get("repository") or ("crates.io" if (p["source"] or "").startswith("registry+") else p["source"])


def markdown(cli: list[dict], desktop: list[dict], npm: list[dict]) -> str:
    d = lock_digests()
    def rows(items):
        return "\n".join(f"| `{p['name']}` | {p['version']} | {p['license'] or 'UNKNOWN'} | {source(p)} |" for p in items)
    licenses: dict[str, int] = {}
    for p in [*cli, *desktop, *npm]:
        licenses[p.get("license") or "UNKNOWN"] = licenses.get(p.get("license") or "UNKNOWN", 0) + 1
    summary = "\n".join(f"| {k} | {v} |" for k, v in sorted(licenses.items(), key=lambda kv: (-kv[1], kv[0])))
    npm_rows = "\n".join(f"| {p['name']} | {p['version']} | {p['license']} |" for p in npm)
    return f"""# 第三方声明（Third-party notices）

本文件由 `python -I tools/generate_notices.py` 依据锁文件生成（`tools/dependency_inventory.py` 也会重新生成），手工修改会被文档验证器拒绝。它列出**随发行包分发**的第三方组件：CLI 包是 `cm-cli` 的普通依赖闭包（不含仅测试/仅构建的 crate）；桌面包是桌面宿主的普通依赖闭包加运行时 npm 包（`dev` 包不进入安装包）。完整依赖清单（含开发与构建依赖）见 [依赖与工具链清单](dependencies.md) 与 [桌面端依赖清单](dependencies-desktop.md)。

发行 zip 中的 `THIRD-PARTY-NOTICES.txt` 由 `tools/package_release.py` 以 `--full` 生成，逐个附上各 crate 自带的许可/声明文件原文。本项目自身许可证尚未决定（见 [分发与许可](distribution-and-licensing.md)），本文件不构成对项目自身许可的声明。

锁文件摘要（SHA-256）：

- `Cargo.lock`：`{d['cli']}`
- `apps/desktop/src-tauri/Cargo.lock`：`{d['desktop']}`
- `apps/desktop/package-lock.json`：`{d['npm']}`

## 许可汇总

| 许可表达式 | 组件数 |
| --- | --- |
{summary}

## CLI 发行包（{len(cli)} 个 crate）

| Crate | 版本 | 许可 | 来源 |
| --- | --- | --- | --- |
{rows(cli)}

## 桌面发行包：Rust crate（{len(desktop)}）

| Crate | 版本 | 许可 | 来源 |
| --- | --- | --- | --- |
{rows(desktop)}

## 桌面发行包：运行时 npm 包（{len(npm)}）

| 包 | 版本 | 许可 |
| --- | --- | --- |
{npm_rows}
"""


def license_files(p: dict) -> list[Path]:
    d = Path(p["manifest_path"]).parent
    if not d.is_dir():
        return []
    return sorted(f for f in d.iterdir() if f.is_file() and f.name.lower().startswith(LICENSE_PREFIXES))


def full_text(items: list[dict], npm: list[dict], title: str) -> tuple[str, list[str]]:
    out = [f"{title}\n{'=' * len(title)}\n",
           "This file lists every third-party component in this package with its declared license and the license and notice",
           "files the component ships. Generated by tools/generate_notices.py from the lock files; nothing was downloaded.\n"]
    missing = []
    for p in items:
        out.append(f"\n{'-' * 78}\n{p['name']} {p['version']}\nLicense: {p['license'] or 'UNKNOWN'}\nSource: {source(p)}\n")
        files = license_files(p)
        if not files:
            missing.append(f"{p['name']} {p['version']}")
            out.append("(The crate package ships no separate license file; the license expression above is from its manifest.)\n")
        for f in files:
            out.append(f"\n--- {f.name} ---\n{f.read_text(encoding='utf-8', errors='replace').rstrip()}\n")
    for p in npm:
        out.append(f"\n{'-' * 78}\n{p['name']} {p['version']} (npm)\nLicense: {p['license']}\n")
        pkg_dir = DESKTOP / p["key"]
        files = sorted(f for f in pkg_dir.iterdir() if f.is_file() and f.name.lower().startswith(LICENSE_PREFIXES)) if pkg_dir.is_dir() else []
        for f in files:
            out.append(f"\n--- {f.name} ---\n{f.read_text(encoding='utf-8', errors='replace').rstrip()}\n")
        if not files:
            missing.append(f"{p['name']} {p['version']} (npm; run npm ci to include its license file)")
    return "\n".join(out) + "\n", missing


def collect():
    cli = closure(metadata(ROOT), {"cm-cli"})
    desk_meta = metadata(DESKTOP / "src-tauri")
    roots = {packages["name"] for packages in desk_meta["packages"] if packages["id"] in desk_meta["workspace_members"]}
    desktop = closure(desk_meta, roots)
    return cli, desktop, npm_runtime()


def write_markdown(cli, desktop, npm) -> Path:
    path = ROOT / "docs/third-party-notices.md"
    path.write_text(markdown(cli, desktop, npm), encoding="utf-8", newline="\n")
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--full", type=Path, help="also write the full-text NOTICE file here")
    ap.add_argument("--scope", choices=["cli", "desktop", "all"], default="cli")
    args = ap.parse_args()
    cli, desktop, npm = collect()
    write_markdown(cli, desktop, npm)
    print(f"Listed {len(cli)} CLI crates, {len(desktop)} desktop crates and {len(npm)} runtime npm packages in docs/third-party-notices.md.")
    if args.full:
        items = {"cli": cli, "desktop": desktop, "all": sorted({(p["name"], p["version"]): p for p in [*cli, *desktop]}.values(), key=lambda p: (p["name"], p["version"]))}[args.scope]
        text, missing = full_text(items, npm if args.scope != "cli" else [], f"CleanManager {args.scope} package: third-party notices")
        args.full.parent.mkdir(parents=True, exist_ok=True)
        args.full.write_text(text, encoding="utf-8", newline="\n")
        print(f"Wrote {args.full} ({len(items)} components; {len(missing)} without a separate license file).")


if __name__ == "__main__":
    main()
