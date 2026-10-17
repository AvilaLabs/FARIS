#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Study-file lint of the reference geometries: citation and omission fields (requirement GEO-009).

    python3 scripts/reference_geometry_lint.py [--root DIR]

Two things are checked.

Reference page. docs/REFERENCE_GEOMETRIES.md has at least three reference sets (one `## ` section each);
every section has a **Citation** line with a link (or says that it has no outside source and links the
derivation), a **Use** line and an **Omits** line of real length; every path in a **Files** line that is
not marked external exists in the repository.

Scenario files. Every scenario under scenarios/ (a JSON file with a `geometry`) has references, each with
an id, a title, an https URL and a `use` that says how the source is used, and an assumptions list in which
at least one entry states what the geometry omits or idealises.

Exit status 1 on any finding. Output: file: finding.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PAGE = Path("docs") / "REFERENCE_GEOMETRIES.md"
MIN_SETS = 3
MIN_OMITS_CHARS = 80
OMISSION = re.compile(r"\b(omit\w*|no resolved|not resolved|idealis\w*|idealiz\w*|without|excludes?|not modelled|not modeled|no ports?)\b", re.I)


def bullet(section: str, label: str) -> str | None:
    """The text of the bullet that starts with **label**, up to the next bullet."""
    m = re.search(rf"^- \*\*{label}[^*]*\*\*(.*?)(?=^- \*\*|\Z)", section, re.S | re.M)
    return re.sub(r"\s+", " ", m.group(1)).strip() if m else None


def lint_page(root: Path) -> list[str]:
    path = root / PAGE
    if not path.is_file():
        return [f"{PAGE}: missing"]
    sections = re.split(r"^## ", path.read_text(encoding="utf-8"), flags=re.M)[1:]
    findings = []
    if len(sections) < MIN_SETS:
        findings.append(f"{PAGE}: {len(sections)} reference sets, need {MIN_SETS}")
    for section in sections:
        title = section.splitlines()[0]
        citation, use, omits, files = (bullet(section, n) for n in ("Citation", "Use", "Omits", "Files"))
        if not citation:
            findings.append(f"{PAGE}: {title}: no Citation line")
        elif not re.search(r"\]\([^)]+\)", citation):
            findings.append(f"{PAGE}: {title}: the citation has no link")
        if not use:
            findings.append(f"{PAGE}: {title}: no Use line")
        if not omits:
            findings.append(f"{PAGE}: {title}: no Omits line")
        elif len(omits) < MIN_OMITS_CHARS:
            findings.append(f"{PAGE}: {title}: the omissions are not stated in {MIN_OMITS_CHARS} characters")
        if files and not re.match(r"\s*external\b", files, re.I):
            for name in re.findall(r"`([^`]+)`", files):
                if "/" in name and not (root / name).exists():
                    findings.append(f"{PAGE}: {title}: file {name} does not exist")
    return findings


def lint_scenario(name: str, doc: dict) -> list[str]:
    findings = []
    refs = doc.get("references")
    if not isinstance(refs, list) or not refs:
        findings.append(f"{name}: no references")
    else:
        for i, ref in enumerate(refs):
            if not isinstance(ref, dict):
                findings.append(f"{name}: reference {i} is not an object")
                continue
            for field in ("id", "title", "use"):
                if not str(ref.get(field, "")).strip():
                    findings.append(f"{name}: reference {ref.get('id', i)} has no {field}")
            if not str(ref.get("url", "")).startswith("https://"):
                findings.append(f"{name}: reference {ref.get('id', i)} has no https URL")
    assumptions = doc.get("assumptions")
    if not isinstance(assumptions, list) or not any(isinstance(a, str) and OMISSION.search(a) for a in assumptions):
        findings.append(f"{name}: no assumption states what the geometry omits or idealises")
    return findings


def lint_scenarios(root: Path) -> list[str]:
    findings = []
    for path in sorted((root / "scenarios").glob("*/*.json")):
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except ValueError as e:
            findings.append(f"{path.relative_to(root)}: not valid JSON ({e})")
            continue
        if isinstance(doc, dict) and "geometry" in doc:
            findings += lint_scenario(str(path.relative_to(root)), doc)
    return findings


def run(root: Path = ROOT) -> list[str]:
    return lint_page(root) + lint_scenarios(root)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="lint the reference geometries for citation and omission fields")
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    findings = run(args.root)
    for f in findings:
        print(f)
    print(f"reference geometry lint: {'clean' if not findings else str(len(findings)) + ' finding(s)'}", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
