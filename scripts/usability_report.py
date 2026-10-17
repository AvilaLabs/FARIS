#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Check usability study reports against the Common Industry Format template (requirement UX-009).

    python3 scripts/usability_report.py [--root DIR]

docs/usability/cif-template.md follows ISO/IEC 25062: Title page, Executive summary, Introduction, Method,
Results, Appendices. This script checks two things:

- the template itself has those six second-level headings in that order, and the sub-headings the format asks
  for under Method (participants, context, tasks, procedure, measures) and Results;
- every report in docs/usability/reports/*.md has the same headings in the same order, and no `[placeholder]`
  left over from the template. The title page of a report must carry the three hashes (protocol, task script,
  analysis plan) as 64 hex digits (UX-010, UX-070, UX-074).

Exit status 1 on any problem. With no reports yet the check covers the template only.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATE = Path("docs") / "usability" / "cif-template.md"
REPORTS = Path("docs") / "usability" / "reports"
TOP = ["Title page", "Executive summary", "Introduction", "Method", "Results", "Appendices"]
UNDER = {
    "Method": ["Participants", "Context of product use", "Tasks", "Procedure", "Measures"],
    "Results": ["Data analysis", "Effectiveness", "Efficiency", "Satisfaction", "Findings and recommendations"],
}
HASH_LINES = ("Protocol hash", "Task script hash", "Analysis plan hash")
HEX64 = re.compile(r"\b[0-9a-f]{64}\b")


def headings(text: str) -> list[tuple[int, str]]:
    out, fence = [], False
    for line in text.splitlines():
        if line.startswith("```"):
            fence = not fence
        match = None if fence else re.match(r"^(##|###) (.+?)\s*$", line)
        if match:
            out.append((len(match.group(1)), match.group(2)))
    return out


def structure_problems(text: str) -> list[str]:
    found = headings(text)
    top = [h for level, h in found if level == 2]
    if top != TOP:
        return [f"second-level headings are {top}, expected {TOP}"]
    problems = []
    for parent, names in UNDER.items():
        inside, current = [], None
        for level, h in found:
            if level == 2:
                current = h
            elif current == parent:
                inside.append(h)
        for name in names:
            if name not in inside:
                problems.append(f"no '{name}' heading under {parent}")
    return problems


def report_problems(text: str) -> list[str]:
    problems = structure_problems(text)
    for placeholder in sorted(set(re.findall(r"\[[^\]\n]{2,}\](?!\()", text))):
        problems.append(f"placeholder left in the report: {placeholder}")
    title = text.split("## Executive summary")[0]
    for label in HASH_LINES:
        line = next((l for l in title.splitlines() if l.lstrip("- ").startswith(label)), None)
        if line is None or not HEX64.search(line):
            problems.append(f"the title page has no 64-digit hash on the '{label}' line")
    return problems


def check(root: Path = ROOT) -> list[str]:
    template = root / TEMPLATE
    if not template.is_file():
        return [f"{TEMPLATE.as_posix()} does not exist"]
    problems = [f"{TEMPLATE.as_posix()}: {p}" for p in structure_problems(template.read_text(encoding="utf-8"))]
    for report in sorted((root / REPORTS).glob("*.md")):
        name = report.relative_to(root).as_posix()
        problems += [f"{name}: {p}" for p in report_problems(report.read_text(encoding="utf-8"))]
    return problems


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    args = ap.parse_args(argv)
    problems = check(args.root)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        return 1
    n = len(list((args.root / REPORTS).glob("*.md")))
    print(f"usability report template follows the Common Industry Format; {n} reports checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
