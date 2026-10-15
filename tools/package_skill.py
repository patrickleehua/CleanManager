"""Validate and package the bundled Agent Skill (MVP 12.4, parity 8.3) into a deterministic .skill archive.

Follows the skill-creator conventions: SKILL.md with YAML frontmatter (name, description; optional license,
compatibility, metadata, allowed-tools), references/ loaded on demand, evals/ excluded from the package.
Usage: python -I tools/package_skill.py [--check]   (--check fails if skills/dist is out of date)
"""
from __future__ import annotations
import argparse
import io
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skills" / "cleanmanager"
OUT = ROOT / "skills" / "dist" / "cleanmanager.skill"
ALLOWED = {"name", "description", "license", "allowed-tools", "metadata", "compatibility"}
# Shell commands that delete, move or empty things: a skill must never offer them as a substitute.
SHELL_DELETE = re.compile(r"^\s*(?:sudo\s+)?(rm|del|erase|rmdir|rd|mv|move|Remove-Item|Move-Item|Clear-RecycleBin|robocopy|git\s+clean|shred)\b", re.I)


def frontmatter(text: str) -> dict[str, str]:
    m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    assert m, "SKILL.md needs YAML frontmatter"
    keys: dict[str, str] = {}
    for line in m.group(1).splitlines():
        if line and not line[0].isspace():
            key, _, value = line.partition(":")
            keys[key.strip()] = value.strip()
    return keys


def validate() -> list[Path]:
    text = (SKILL / "SKILL.md").read_text(encoding="utf-8")
    fm = frontmatter(text)
    assert set(fm) <= ALLOWED, f"unexpected frontmatter keys: {sorted(set(fm) - ALLOWED)}"
    assert re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", fm.get("name", "")) and len(fm["name"]) <= 64, "name must be kebab-case"
    assert fm["name"] == SKILL.name, "name must equal the folder name"
    d = fm.get("description", "")
    assert 0 < len(d) <= 1024 and "<" not in d and ">" not in d, "description: 1..1024 chars, no angle brackets"
    assert len(fm.get("compatibility", "")) <= 500, "compatibility too long"
    assert len(text.splitlines()) < 500, "keep SKILL.md under 500 lines"
    files = sorted(p for p in SKILL.rglob("*") if p.is_file() and "evals" not in p.relative_to(SKILL).parts and "__pycache__" not in p.parts)
    for p in files:
        if p.suffix != ".md":
            continue
        body = p.read_text(encoding="utf-8")
        fence = False
        for n, line in enumerate(body.splitlines(), 1):
            if line.startswith("```"):
                fence = not fence
                continue
            if fence:
                assert not SHELL_DELETE.match(line), f"{p}:{n}: shell deletion offered as a command"
                assert not re.search(r"--(yes|force|approve|assume-yes)", line), f"{p}:{n}: no self-approval flags"
                assert line.strip() == "" or line.lstrip().startswith(("cleanmanager ", "#")), f"{p}:{n}: code blocks hold cleanmanager commands only"
        for target in re.findall(r"\]\(([^)#]+)\)", body):
            if not re.match(r"^[a-z]+:", target):
                assert (p.parent / target).exists(), f"{p}: broken link {target}"
    # The rule library stays in the core: no rule IDs or rule JSON copied into the skill.
    rule_ids = {q.stem for q in (ROOT / "rules" / "windows").glob("*.json")} | {q.stem for q in (ROOT / "rules" / "macos").glob("*.json")}
    blob = "\n".join(p.read_text(encoding="utf-8") for p in files)
    copied = sorted(r for r in rule_ids if r in blob)
    assert not copied, f"skill copies rule IDs: {copied}"
    return files


def archive(files: list[Path]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for p in files:
            info = zipfile.ZipInfo(str(Path(SKILL.name) / p.relative_to(SKILL)).replace("\\", "/"), date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, p.read_bytes().replace(b"\r\n", b"\n"))
    return buf.getvalue()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="fail when the packaged archive is missing or stale")
    args = ap.parse_args()
    files = validate()
    data = archive(files)
    if args.check:
        if not OUT.exists() or OUT.read_bytes() != data:
            print(f"{OUT.relative_to(ROOT)} is out of date; run python -I tools/package_skill.py", file=sys.stderr)
            return 1
        print(f"{OUT.relative_to(ROOT)} is up to date ({len(files)} files)")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(data)
    print(f"packaged {len(files)} files into {OUT.relative_to(ROOT)} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
