#!/usr/bin/env python3
"""Validate repository Markdown structure and local links."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
MARKDOWN_LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")


def markdown_files() -> list[Path]:
    files = [
        REPO_ROOT / "README.md",
        REPO_ROOT / "README.zh-CN.md",
        REPO_ROOT / "AGENTS.md",
        REPO_ROOT / "web/console/README.md",
    ]
    files.extend(sorted((REPO_ROOT / ".agents").rglob("*.md")))
    files.extend(sorted((REPO_ROOT / "docs").rglob("*.md")))
    return files


def content_lines_outside_fences(lines: list[str]) -> list[tuple[int, str]]:
    outside: list[tuple[int, str]] = []
    fence: str | None = None
    for line_number, line in enumerate(lines, 1):
        marker = line.lstrip()[:3]
        if marker in {"```", "~~~"}:
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            continue
        if fence is None:
            outside.append((line_number, line))
    return outside


def check_file(path: Path) -> list[str]:
    errors: list[str] = []
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    outside = content_lines_outside_fences(lines)

    h1 = [(line_number, line) for line_number, line in outside if line.startswith("# ")]
    if len(h1) != 1:
        errors.append(f"{path.relative_to(REPO_ROOT)}: expected one H1, found {len(h1)}")

    for line_number, line in outside:
        for raw_target in MARKDOWN_LINK.findall(line):
            target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path_target = target.split("#", 1)[0]
            if path_target and not (path.parent / path_target).resolve().exists():
                errors.append(
                    f"{path.relative_to(REPO_ROOT)}:{line_number}: "
                    f"missing local link target {target}"
                )

    relative = path.relative_to(REPO_ROOT)
    first_lines = "\n".join(lines[:12])
    if relative.parts[:2] in {
        ("docs", "user"),
        ("docs", "development"),
        ("docs", "archive"),
    } and not re.search(r"^> (状态|Status)[:：]", first_lines, re.MULTILINE):
        errors.append(f"{relative}: missing status metadata in the first 12 lines")

    if relative.parts[:2] == ("docs", "reference"):
        if not re.search(r"^> (类型|Type)[:：]", first_lines, re.MULTILINE):
            errors.append(f"{relative}: missing reference type metadata")
        if relative.name != "README.md":
            if not re.search(
                r"^> (最近核对|Last verified)[:：]", first_lines, re.MULTILINE
            ):
                errors.append(f"{relative}: missing external verification date")
            if not re.search(
                r"^> (权威来源|Authoritative source)[:：]", first_lines, re.MULTILINE
            ):
                errors.append(f"{relative}: missing authoritative source")

    return errors


def main() -> int:
    files = markdown_files()
    errors: list[str] = []
    for path in files:
        if not path.exists():
            errors.append(f"{path.relative_to(REPO_ROOT)}: file does not exist")
            continue
        errors.extend(check_file(path))

    if errors:
        print("documentation validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(f"documentation validation passed ({len(files)} Markdown files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
