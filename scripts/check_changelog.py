#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Layout check of CHANGELOG.md (requirement DOC-040).

    python3 scripts/check_changelog.py [--root DIR]

Fails when:

- the top version section is not "Unreleased" or a dated "X.Y.Z - YYYY-MM-DD" heading, or versions are not newest
  first, or a version is repeated;
- a section uses a heading outside the set below, or repeats a heading (two "Added" blocks in one version);
- a section is empty;
- "Numerical changes", when present, is not the first heading of its section (a change that alters a published
  number is flagged at the top);
- the version in Cargo.toml has no section (a release without notes), unless an "Unreleased" section is on top.

It cannot tell whether a change alters a number; that needs the golden-study comparison of requirement PRV-027,
which is not built. The check keeps the place for the flag and makes it hard to hide.
"""
from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADINGS = {"Numerical changes", "Added", "Changed", "Deprecated", "Removed", "Fixed", "Security",
            # headings of the first releases, kept as they were published
            "Limits", "Study", "Uncertainty", "Magnets", "Files and export", "Evidence", "Known limits"}
VERSION = re.compile(r"^##\s+(Unreleased|(\d+)\.(\d+)\.(\d+)\s+[—-]\s+(\d{4}-\d{2}-\d{2}))\s*$")


def sections(text: str) -> list[tuple[str, int, list[tuple[str, int, str]]]]:
    """[(heading, line, [(subheading, line, body)])] for each `## ` section."""
    out: list[tuple[str, int, list]] = []
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("## "):
            out.append((line, number, []))
        elif line.startswith("### ") and out:
            out[-1][2].append([line[4:].strip(), number, ""])
        elif out and out[-1][2]:
            out[-1][2][-1][2] += line + "\n"
    return [(h, n, [tuple(s) for s in subs]) for h, n, subs in out]


def problems_in(text: str, current: str) -> list[str]:
    problems = []
    parsed = sections(text)
    if not parsed:
        return ["CHANGELOG.md has no version sections"]
    seen, previous = set(), None
    for index, (heading, number, subs) in enumerate(parsed):
        match = VERSION.match(heading)
        if not match:
            problems.append(f"line {number}: '{heading}' is not 'Unreleased' or 'X.Y.Z — YYYY-MM-DD'")
            continue
        name = match.group(1).split()[0]
        if name == "Unreleased" and index != 0:
            problems.append(f"line {number}: 'Unreleased' must be the top section")
        if name in seen:
            problems.append(f"line {number}: version {name} appears twice")
        seen.add(name)
        if match.group(2):
            version = tuple(int(match.group(i)) for i in (2, 3, 4))
            if previous is not None and version >= previous:
                problems.append(f"line {number}: {name} is not older than the section above it")
            previous = version
        titles = [s[0] for s in subs]
        if not subs:
            problems.append(f"line {number}: {name} has no sections (Added, Changed, Fixed, ...)")
        for title, sub_line, body in subs:
            if title not in HEADINGS:
                problems.append(f"line {sub_line}: '### {title}' is not one of {', '.join(sorted(HEADINGS))}")
            if not body.strip():
                problems.append(f"line {sub_line}: '### {title}' under {name} is empty")
        for title in sorted({t for t in titles if titles.count(t) > 1}):
            problems.append(f"{name} has more than one '### {title}'; merge them")
        if "Numerical changes" in titles and titles[0] != "Numerical changes":
            problems.append(f"{name}: 'Numerical changes' must come first")
    top = VERSION.match(parsed[0][0])
    if top and top.group(1) != "Unreleased" and top.group(1).split()[0] != current:
        problems.append(f"the newest section is {top.group(1).split()[0]} but Cargo.toml is {current}; add a section for {current}")
    if current not in seen and "Unreleased" not in seen:
        problems.append(f"no section for the workspace version {current}")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    current = tomllib.loads((args.root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    problems = problems_in((args.root / "CHANGELOG.md").read_text(encoding="utf-8"), current)
    if problems:
        print("\n".join("changelog: " + p for p in problems), file=sys.stderr)
        return 1
    print("changelog: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
