#!/usr/bin/env python3
"""Normalize workspace package versions before generating a cargo-chef recipe.

Release preparation changes the workspace package versions without changing
third-party dependencies. cargo-chef includes those versions in its recipe,
which otherwise invalidates the complete dependency layer for every release.
This script runs only inside the disposable Docker planner context.
"""

from __future__ import annotations

import argparse
import re
import tomllib
from pathlib import Path


WORKSPACE_PACKAGES = {
    "ai-gateway": Path("Cargo.toml"),
    "ai-gateway-perf": Path("tools/forwarding-perf/Cargo.toml"),
}


def replace_manifest_version(path: Path, version: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_package = False
    replacements = 0
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = stripped == "[package]"
            continue
        if not in_package:
            continue
        match = re.match(r'^(\s*version\s*=\s*)"[^"]+"(\s*(?:#.*)?\r?\n?)$', line)
        if match:
            lines[index] = f'{match.group(1)}"{version}"{match.group(2)}'
            replacements += 1
            break
    if replacements != 1:
        raise RuntimeError(f"{path} must contain one literal [package].version")
    path.write_text("".join(lines), encoding="utf-8")


def replace_lock_versions(path: Path, version: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    package_start = [
        index for index, line in enumerate(lines) if line.strip() == "[[package]]"
    ]
    package_start.append(len(lines))
    replacements: dict[str, int] = {name: 0 for name in WORKSPACE_PACKAGES}

    for start, end in zip(package_start, package_start[1:]):
        name = None
        version_index = None
        for index in range(start + 1, end):
            name_match = re.match(r'^name\s*=\s*"([^"]+)"', lines[index])
            if name_match:
                name = name_match.group(1)
            if re.match(r"^version\s*=", lines[index]):
                version_index = index
        if name not in WORKSPACE_PACKAGES:
            continue
        if version_index is None:
            raise RuntimeError(f"Cargo.lock package {name} has no version")
        suffix_match = re.match(
            r'^(\s*version\s*=\s*)"[^"]+"(\s*(?:#.*)?\r?\n?)$',
            lines[version_index],
        )
        if suffix_match is None:
            raise RuntimeError(f"Cargo.lock package {name} has a non-literal version")
        lines[version_index] = (
            f'{suffix_match.group(1)}"{version}"{suffix_match.group(2)}'
        )
        replacements[name] += 1

    invalid = {name: count for name, count in replacements.items() if count != 1}
    if invalid:
        raise RuntimeError(
            "Cargo.lock must contain each workspace package exactly once: "
            + ", ".join(f"{name}={count}" for name, count in sorted(invalid.items()))
        )
    path.write_text("".join(lines), encoding="utf-8")


def validate(root: Path, version: str) -> None:
    for name, relative_path in WORKSPACE_PACKAGES.items():
        with (root / relative_path).open("rb") as handle:
            manifest = tomllib.load(handle)
        actual = manifest["package"]["version"]
        if actual != version:
            raise RuntimeError(
                f"{relative_path} package version is {actual!r}, expected {version!r}"
            )

    with (root / "Cargo.lock").open("rb") as handle:
        lock = tomllib.load(handle)
    versions = {
        package["name"]: package["version"]
        for package in lock["package"]
        if package["name"] in WORKSPACE_PACKAGES
    }
    expected = {name: version for name in WORKSPACE_PACKAGES}
    if versions != expected:
        raise RuntimeError(
            f"Cargo.lock workspace versions are {versions!r}, expected {expected!r}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--version", default="0.0.0")
    arguments = parser.parse_args()
    root = arguments.root.resolve()

    for relative_path in WORKSPACE_PACKAGES.values():
        replace_manifest_version(root / relative_path, arguments.version)
    replace_lock_versions(root / "Cargo.lock", arguments.version)
    validate(root, arguments.version)
    print(f"normalized cargo-chef workspace versions to {arguments.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
