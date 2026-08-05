#!/usr/bin/env python3
"""Generate redistributable third-party license materials from locked dependencies.

The output intentionally preserves each dependency's supplied license and NOTICE
files rather than reducing them to SPDX identifiers.  This is required for
licenses such as MIT and Apache-2.0 whose redistribution conditions include
copyright and notice retention.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LICENSE_NAMES = ("license", "licence", "copying", "copyright", "notice")


def run(*command: str, cwd: Path = ROOT) -> str:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout


def safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", value).strip("_")


def license_files(directory: Path) -> list[Path]:
    files = [
        path
        for path in directory.iterdir()
        if path.is_file()
        and any(
            path.name.lower().startswith(f"{candidate}.")
            or path.name.lower().startswith(f"{candidate}-")
            or path.name.lower() == candidate
            for candidate in LICENSE_NAMES
        )
    ]
    return sorted(files, key=lambda path: path.name.lower())


def copy_materials(
    output: Path, ecosystem: str, name: str, version: str, source: Path
) -> list[str]:
    destination = output / "LICENSES" / ecosystem / safe_name(f"{name}@{version}")
    destination.mkdir(parents=True, exist_ok=True)
    copied = []
    for material in license_files(source):
        target = destination / material.name
        shutil.copyfile(material, target)
        copied.append(target.relative_to(output).as_posix())
    if not copied:
        # A small number of registry packages declare an SPDX expression but do
        # not publish a standalone license file. Keep their upstream manifest
        # and README so the shipped material still preserves the declaration,
        # authorship metadata, and any license section supplied by upstream.
        for candidate in ("Cargo.toml", "package.json", "README.md", "README"):
            material = source / candidate
            if material.is_file():
                target = destination / f"UPSTREAM_{material.name}"
                shutil.copyfile(material, target)
                copied.append(target.relative_to(output).as_posix())
    return copied


def cargo_packages() -> list[tuple[str, str, str | None, Path, list[str]]]:
    metadata = json.loads(
        run(
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--features",
            "embedded-console-ui,mcp-server",
        )
    )
    packages = {package["id"]: package for package in metadata["packages"]}
    root_id = next(
        package["id"]
        for package in metadata["packages"]
        if package["name"] == "ai-gateway" and package["manifest_path"] == str(ROOT / "Cargo.toml")
    )
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    reachable = {root_id}
    pending = [root_id]
    while pending:
        node = nodes[pending.pop()]
        for dependency in node["deps"]:
            if any(kind["kind"] in (None, "build") for kind in dependency["dep_kinds"]):
                if dependency["pkg"] not in reachable:
                    reachable.add(dependency["pkg"])
                    pending.append(dependency["pkg"])

    result = []
    for package_id in reachable:
        package = packages[package_id]
        if package_id == root_id:
            continue
        source = Path(package["manifest_path"]).parent
        result.append(
            (
                package["name"],
                package["version"],
                package.get("license"),
                source,
                copy_materials(
                    OUTPUT, "cargo", package["name"], package["version"], source
                ),
            )
        )
    return sorted(result)


def walk_npm_dependencies(dependencies: dict[str, object]) -> Iterable[dict[str, object]]:
    for dependency in dependencies.values():
        assert isinstance(dependency, dict)
        yield dependency
        nested = dependency.get("dependencies", {})
        if isinstance(nested, dict):
            yield from walk_npm_dependencies(nested)


def npm_packages(console_dir: Path) -> list[tuple[str, str, str | None, Path, list[str]]]:
    frozen_tree = console_dir / "production-dependencies.json"
    listing = (
        json.loads(frozen_tree.read_text(encoding="utf-8"))
        if frozen_tree.is_file()
        else json.loads(
            run(
                "pnpm",
                "list",
                "--prod",
                "--depth",
                "Infinity",
                "--json",
                cwd=console_dir,
            )
        )
    )
    seen: set[tuple[str, str]] = set()
    result = []
    root_dependencies = listing[0].get("dependencies", {})
    assert isinstance(root_dependencies, dict)
    for dependency in walk_npm_dependencies(root_dependencies):
        name = str(dependency["from"])
        version = str(dependency["version"])
        if (name, version) in seen:
            continue
        seen.add((name, version))
        source = Path(str(dependency["path"]))
        manifest = json.loads((source / "package.json").read_text(encoding="utf-8"))
        result.append(
            (
                name,
                version,
                manifest.get("license"),
                source,
                copy_materials(OUTPUT, "npm", name, version, source),
            )
        )
    return sorted(result)


def write_index(
    output: Path,
    cargo: list[tuple[str, str, str | None, Path, list[str]]],
    npm: list[tuple[str, str, str | None, Path, list[str]]],
) -> None:
    missing = [
        f"{ecosystem}:{name}@{version}"
        for ecosystem, packages in (("cargo", cargo), ("npm", npm))
        for name, version, _license, _source, materials in packages
        if not materials
    ]
    if missing:
        raise RuntimeError(
            "dependencies without a top-level license/notice file:\n"
            + "\n".join(sorted(missing))
        )

    lines = [
        "# Third-party license materials",
        "",
        "Generated from `Cargo.lock` and `web/console/pnpm-lock.yaml`; do not edit.",
        "The `LICENSES/` tree preserves the license and notice files supplied by each",
        "redistributed dependency. For packages that publish no standalone license",
        "file, it instead retains their upstream manifest and README as `UPSTREAM_*`",
        "material; this condition should be reviewed before changing those packages.",
        "",
    ]
    for title, packages in (("Rust dependencies", cargo), ("Console production dependencies", npm)):
        lines.extend([f"## {title}", "", "| Package | Declared license | Materials |", "| --- | --- | --- |"])
        for name, version, license_id, _source, materials in packages:
            lines.append(
                f"| `{name}@{version}` | `{license_id or 'NOASSERTION'}` | "
                + "<br>".join(f"`{material}`" for material in materials)
                + " |"
            )
        lines.append("")
    (output / "THIRD_PARTY_NOTICES.md").write_text("\n".join(lines), encoding="utf-8")


OUTPUT: Path


def main() -> int:
    global OUTPUT
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "target" / "third-party-licenses",
        help="directory that receives THIRD_PARTY_NOTICES.md and LICENSES/",
    )
    arguments = parser.parse_args()
    OUTPUT = arguments.output.resolve()
    if OUTPUT.exists():
        shutil.rmtree(OUTPUT)
    OUTPUT.mkdir(parents=True)

    try:
        cargo = cargo_packages()
        npm = npm_packages(ROOT / "web" / "console")
        write_index(OUTPUT, cargo, npm)
    except (OSError, RuntimeError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"license material generation failed: {error}", file=sys.stderr)
        return 1

    print(OUTPUT)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
