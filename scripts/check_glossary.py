#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint of the glossary, docs/glossary.md (requirement DOC-034).

    python3 scripts/check_glossary.py [--root DIR]

Each entry is a bullet `- **Term.** Definition. [Page](link)`. The check fails when:

- a term appears twice (case-insensitive) - each term is defined once;
- a definition is longer than 40 words (the link text is not counted);
- an entry has no link, or its link target does not exist, or its #anchor is not a heading there;
- a term that FARIS shows is missing: the four verdict words and the other canonical forms in
  scripts/wording_lint_terms.json, and the kind labels in the table of docs/guide/results.md.

The "terminology table" of requirement UX-040 does not exist yet; the two lists above are the vocabulary the
UI uses today and stand in for it. When UX-040 is built, point REQUIRED at it.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_links  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GLOSSARY = Path("docs") / "glossary.md"
MAX_WORDS = 40
ENTRY = re.compile(r"^- \*\*(?P<term>[^*]+?)\.\*\*\s+(?P<body>.+)$")
LINK = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")


def parse(text: str) -> list[dict]:
    entries = []
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("- "):
            match = ENTRY.match(line)
            if not match:
                entries.append({"line": number, "term": None, "raw": line})
                continue
            body = match.group("body")
            links = LINK.findall(body)
            prose = LINK.sub("", body)
            entries.append({"line": number, "term": match.group("term").strip(), "links": links,
                            "words": len(re.findall(r"[^\s]+", prose))})
    return entries


def required_terms(root: Path) -> set[str]:
    terms = {"PASS", "FAIL", "INCONCLUSIVE", "NOT_EVALUATED"}
    lint = json.loads((root / "scripts" / "wording_lint_terms.json").read_text(encoding="utf-8"))
    terms.update(f for f in lint["verdict_words"]["canonical_forms"] if f.isupper())
    results = (root / "docs" / "guide" / "results.md").read_text(encoding="utf-8")
    in_table = False
    for line in results.splitlines():
        if line.startswith("| Badge"):
            in_table = True
        elif in_table and line.startswith("|"):
            cell = line.split("|")[1].strip()
            if cell and not set(cell) <= {"-", " "}:
                terms.add(cell)
        elif in_table:
            break
    return terms


def check(root: Path) -> list[str]:
    path = root / GLOSSARY
    if not path.is_file():
        return [f"{GLOSSARY} does not exist"]
    problems, seen = [], {}
    entries = parse(path.read_text(encoding="utf-8"))
    for e in entries:
        where = f"{GLOSSARY}:{e['line']}"
        if e["term"] is None:
            problems.append(f"{where}: bullet is not of the form '- **Term.** definition [Page](link)'")
            continue
        key = e["term"].lower()
        if key in seen:
            problems.append(f"{where}: '{e['term']}' is already defined at line {seen[key]}")
        seen[key] = e["line"]
        if e["words"] > MAX_WORDS:
            problems.append(f"{where}: '{e['term']}' has {e['words']} words; the limit is {MAX_WORDS}")
        if not e["links"]:
            problems.append(f"{where}: '{e['term']}' has no link to a page that explains it")
        for _, target in e["links"]:
            file_part, _, fragment = target.partition("#")
            dest = (path.parent / file_part).resolve()
            if not dest.is_file():
                problems.append(f"{where}: link {target} does not exist")
            elif fragment and fragment not in check_links.anchors_of(dest.read_text(encoding="utf-8")):
                problems.append(f"{where}: no heading '{fragment}' in {file_part}")
    defined = {t.lower() for t in seen}
    for term in sorted(required_terms(root)):
        if term.lower() not in defined:
            problems.append(f"{GLOSSARY}: the UI shows '{term}' but the glossary does not define it")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    problems = check(args.root)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    n = len(parse((args.root / GLOSSARY).read_text(encoding="utf-8")))
    print(f"glossary: PASS, {n} terms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
