#!/usr/bin/env python3
"""Fail when a release tag and publishable package versions diverge."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def read_toml(path: str) -> dict:
    with (ROOT / path).open("rb") as file:
        return tomllib.load(file)


def read_json(path: str) -> dict:
    with (ROOT / path).open(encoding="utf-8") as file:
        return json.load(file)


def read_node_binding_version() -> str:
    source = (ROOT / "bindings/node/index.js").read_text(encoding="utf-8")
    versions = set(re.findall(r"bindingPackageVersion !== '([^']+)'", source))
    if len(versions) != 1:
        raise ValueError(
            f"expected one generated Node binding version, found {sorted(versions)}"
        )
    return versions.pop()


def read_formats_version() -> str:
    text = (ROOT / "docs/FORMATS_AND_LIMITS.md").read_text(encoding="utf-8")
    versions = set(
        re.findall(r"spoor `v(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)`", text)
    )
    if len(versions) != 1:
        raise ValueError(
            f"expected one formats.md version, found {sorted(versions)}"
        )
    return versions.pop()


def main() -> int:
    if len(sys.argv) != 2 or not re.fullmatch(r"v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", sys.argv[1]):
        print("usage: check-release-version.py vMAJOR.MINOR.PATCH", file=sys.stderr)
        return 2

    expected = sys.argv[1][1:]
    versions = {
        "Cargo workspace": read_toml("Cargo.toml")["workspace"]["package"]["version"],
        "Cargo workspace spoor-core dependency": read_toml("Cargo.toml")["workspace"][
            "dependencies"
        ]["spoor-core"]["version"],
        "Cargo WASM spoor-core dependency": read_toml("crates/spoor-wasm/Cargo.toml")[
            "dependencies"
        ]["spoor-core"]["version"],
        "Python pyspoor": read_toml("bindings/python/pyproject.toml")["project"]["version"],
        "Node @harrisonwang/spoor": read_json("bindings/node/package.json")["version"],
        "Node package-lock": read_json("bindings/node/package-lock.json")["version"],
        "Node package-lock root": read_json("bindings/node/package-lock.json")["packages"][
            ""
        ]["version"],
        "Node generated binding": read_node_binding_version(),
        "WASM @harrisonwang/spoor-wasm": read_json("crates/spoor-wasm/package.json")["version"],
        "formats.md": read_formats_version(),
    }

    lock_packages = read_toml("Cargo.lock")["package"]
    for name in ("spoor-core", "spoor-cli", "spoor-wasm", "pyspoor-native", "spoor-node"):
        versions[f"Cargo.lock {name}"] = next(
            package["version"] for package in lock_packages if package["name"] == name
        )

    tauri_lock_packages = read_toml("examples/tauri-desktop/src-tauri/Cargo.lock")["package"]
    versions["tauri-desktop Cargo.lock spoor-core"] = next(
        package["version"] for package in tauri_lock_packages if package["name"] == "spoor-core"
    )

    mismatches = {
        label: version for label, version in versions.items() if version != expected
    }
    if mismatches:
        print(f"release tag {sys.argv[1]} expects version {expected}", file=sys.stderr)
        for label, version in mismatches.items():
            print(f"- {label}: {version}", file=sys.stderr)
        return 1

    print(f"release versions match {sys.argv[1]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
