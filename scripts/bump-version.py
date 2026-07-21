#!/usr/bin/env python3
"""Single-entry version bump for every publishable spoor package, then validate.

Usage:
  scripts/bump-version.py X.Y.Z            # apply, then auto-validate
  scripts/bump-version.py vX.Y.Z           # leading v is accepted
  scripts/bump-version.py X.Y.Z --dry-run  # preview without writing

The current version is read from ``Cargo.toml`` (the workspace package version),
so only the target version is passed in. Every location this script rewrites is
also verified by ``scripts/check-release-version.py`` (including the limitations
doc version stamp and the standalone ``examples/tauri-desktop`` lockfile).

Invariant: the set of files this script rewrites equals the set
``check-release-version.py`` verifies. After writing, this script runs that
verifier, so the auto-validation fully covers every location it touched. Keep
the two scripts in sync when adding a version location.
"""

from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?")
# spoor-family crates whose Cargo.lock entries track the workspace version.
LOCK_PACKAGE = re.compile(r"^(?:spoor[0-9A-Za-z_-]*|pyspoor-native)$")

# Literal substring rewrites. Each template embeds the version via ``{v}``.
# ``expect`` (when present) asserts the exact total occurrence count across the
# spec's templates — a guard that fails loudly if a file's shape drifts.
# Templates without ``expect`` only require at least one match each.
SPECS: list[dict] = [
    {"path": "Cargo.toml", "templates": ['version = "{v}"'], "expect": 2},
    {"path": "crates/spoor-wasm/Cargo.toml", "templates": ['version = "{v}"'], "expect": 1},
    {"path": "bindings/python/pyproject.toml", "templates": ['version = "{v}"'], "expect": 1},
    {"path": "crates/spoor-wasm/package.json", "templates": ['"version": "{v}"'], "expect": 1},
    {"path": "bindings/node/package.json", "templates": ['"version": "{v}"'], "expect": 1},
    {"path": "bindings/node/package-lock.json", "templates": ['"version": "{v}"'], "expect": 2},
    # napi regenerates index.js, so the count is not fixed — require >= 1 each.
    {"path": "bindings/node/index.js", "templates": ["!== '{v}'", "expected {v} but got"]},
    # Documentation prose: at least one ``vX.Y.Z`` mention.
    {"path": "docs/FORMATS_AND_LIMITS.md", "templates": ["v{v}"]},
]

CARGO_LOCKS = ("Cargo.lock", "examples/tauri-desktop/src-tauri/Cargo.lock")


class BumpError(Exception):
    pass


def current_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as file:
        return tomllib.load(file)["workspace"]["package"]["version"]


def rewrite_literals(spec: dict, old: str, new: str) -> tuple[str, int]:
    path = ROOT / spec["path"]
    text = path.read_text(encoding="utf-8")
    total = 0
    for template in spec["templates"]:
        needle = template.format(v=old)
        count = text.count(needle)
        if count == 0:
            raise BumpError(f"{spec['path']}: 未找到模式 {needle!r}（文件结构可能已变）")
        text = text.replace(needle, template.format(v=new))
        total += count
    if "expect" in spec and total != spec["expect"]:
        raise BumpError(
            f"{spec['path']}: 预期替换 {spec['expect']} 处，实际 {total} 处"
        )
    return text, total


def rewrite_cargo_lock(rel: str, old: str, new: str) -> tuple[str, list[str]]:
    path = ROOT / rel
    text = path.read_text(encoding="utf-8")
    changed: list[str] = []

    pattern = re.compile(
        r'(?P<head>\[\[package\]\]\nname = "(?P<name>[^"]+)"\nversion = ")'
        + re.escape(old)
        + r'(?P<tail>")'
    )

    def replace(match: re.Match) -> str:
        name = match.group("name")
        if not LOCK_PACKAGE.match(name):
            return match.group(0)
        changed.append(name)
        return match.group("head") + new + match.group("tail")

    new_text = pattern.sub(replace, text)
    if not changed:
        raise BumpError(f"{rel}: 未找到版本为 {old} 的 spoor 包")
    return new_text, changed


def main() -> int:
    args = sys.argv[1:]
    dry_run = "--dry-run" in args
    positional = [arg for arg in args if not arg.startswith("--")]
    if len(positional) != 1:
        print("usage: bump-version.py X.Y.Z [--dry-run]", file=sys.stderr)
        return 2

    new = positional[0].lstrip("v")
    if not SEMVER.fullmatch(new):
        print(f"无效版本号: {positional[0]}（应为 X.Y.Z）", file=sys.stderr)
        return 2

    old = current_version()
    if old == new:
        print(f"版本已是 {new}，无需修改")
        return 0

    print(f"版本 {old} -> {new}{'（dry-run）' if dry_run else ''}")

    try:
        edits: list[tuple[str, str, str]] = []
        for spec in SPECS:
            text, total = rewrite_literals(spec, old, new)
            edits.append((spec["path"], text, f"{total} 处"))
        for rel in CARGO_LOCKS:
            text, changed = rewrite_cargo_lock(rel, old, new)
            edits.append((rel, text, f"{len(changed)} 个包: {', '.join(changed)}"))
    except BumpError as error:
        print(f"中止，未写入任何文件: {error}", file=sys.stderr)
        return 1

    for rel, _text, info in edits:
        print(f"  {rel}: {info}")

    if dry_run:
        print("dry-run: 未写入文件")
        return 0

    for rel, text, _info in edits:
        # newline="" keeps LF on Windows too, so the bump never churns line endings.
        (ROOT / rel).write_text(text, encoding="utf-8", newline="")

    print("运行 check-release-version.py 自动校验 ...")
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "check-release-version.py"), f"v{new}"]
    )
    if result.returncode == 0:
        print(f"完成。建议下一步: git commit 后打 tag v{new}")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
