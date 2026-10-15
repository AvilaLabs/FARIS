#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Deprecation lint (requirement DOC-043).

    python3 scripts/check_deprecations.py [--root DIR]

Reads the register in docs/DEPRECATION_POLICY.md (the table between the deprecations:begin and :end markers) and
fails when an entry has no replacement, when its removal release is less than two minor releases after the
release that announced it, when the removal release has been reached by the workspace version but the item is
still listed, or when the announcing release is not in the changelog. Versions are X.Y.Z.
"""
from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POLICY = Path("docs") / "DEPRECATION_POLICY.md"
MIN_MINOR_GAP = 2


def version(text: str) -> tuple[int, int, int]:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", text.strip())
    if not match:
        raise ValueError(f"{text!r} is not a version X.Y.Z")
    return tuple(int(g) for g in match.groups())  # type: ignore[return-value]


def register(text: str) -> list[dict]:
    match = re.search(r"<!-- deprecations:begin -->\n(.*?)<!-- deprecations:end -->", text, re.S)
    if not match:
        raise ValueError("the register markers (deprecations:begin and :end) are missing")
    rows = []
    for line in match.group(1).splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 4 or cells[0] in ("Item", "") or set(cells[0]) <= {"-", " "}:
            continue
        rows.append(dict(zip(("item", "replacement", "announced", "removal"), cells)))
    return rows


def problems_in(rows: list[dict], current: str, changelog: str) -> list[str]:
    problems = []
    for r in rows:
        name = r["item"]
        if not r["replacement"] or r["replacement"].lower() in ("none", "-"):
            problems.append(f"{name}: no replacement named")
        try:
            announced, removal = version(r["announced"]), version(r["removal"])
        except ValueError as e:
            problems.append(f"{name}: {e}")
            continue
        # Two minor releases after the announcing one, e.g. announced 0.3.0 -> removal no earlier than 0.5.0.
        earliest = (announced[0], announced[1] + MIN_MINOR_GAP, 0)
        if removal < earliest:
            problems.append(f"{name}: removal in {r['removal']} is less than {MIN_MINOR_GAP} minor releases after {r['announced']}")
        if version(current) >= removal:
            problems.append(f"{name}: removal release {r['removal']} has been reached ({current}); remove the item and this entry")
        if not re.search(rf"^##\s+{re.escape(r['announced'])}\b|^##\s+Unreleased", changelog, re.M):
            problems.append(f"{name}: the announcing release {r['announced']} has no section in CHANGELOG.md")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    try:
        rows = register((args.root / POLICY).read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"deprecations: {e}", file=sys.stderr)
        return 1
    current = tomllib.loads((args.root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    changelog = (args.root / "CHANGELOG.md").read_text(encoding="utf-8")
    problems = problems_in(rows, current, changelog)
    if problems:
        print("\n".join("deprecations: " + p for p in problems), file=sys.stderr)
        return 1
    print(f"deprecations: PASS, {len(rows)} deprecated items")
    return 0


if __name__ == "__main__":
    sys.exit(main())
