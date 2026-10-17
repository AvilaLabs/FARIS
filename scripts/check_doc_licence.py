#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Licence lint for the handbook (requirement DOC-037).

1. book.toml loads docs/licence-footer.js on every page, and the script states AGPL-3.0-only, the
   repository licence (LICENSE).
2. Every figure file under docs/ and every figure a handbook page or the README shows has an entry in
   docs/figure-credits.json with a licence and a credit line; a figure whose origin is not "faris-capture"
   also names its "owner". An entry for a missing file is an error, and so is an external figure URL.
3. With --built DIR (a built handbook, for example dist/docs): every HTML page loads the footer script.

Exit status 1 and one line per problem.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIGURE_SUFFIXES = (".png", ".gif", ".svg", ".jpg", ".jpeg")
CREDITS = "docs/figure-credits.json"
FARIS_ORIGIN = "faris-capture"


def figure_files(root: Path = ROOT) -> list[str]:
    out = subprocess.run(["git", "ls-files", "docs"], cwd=root, capture_output=True, text=True, check=True).stdout
    return sorted(f for f in out.splitlines() if f.endswith(FIGURE_SUFFIXES))


def shown_figures(root: Path = ROOT) -> set[str]:
    """Figures that README.md and the handbook pages embed, as repository-relative paths."""
    shown = set()
    pages = [root / "README.md"] + sorted((root / "docs/guide").glob("*.md"))
    for page in pages:
        for target in re.findall(r"!\[[^\]]*\]\(([^)\s]+)", page.read_text(encoding="utf-8")):
            if "://" in target:
                shown.add(target)
                continue
            resolved = (page.parent / target.split("#")[0]).resolve()
            shown.add(resolved.relative_to(root.resolve()).as_posix() if root.resolve() in resolved.parents else target)
    return shown


def check_credits(files: list[str], shown: set[str], credits: dict) -> list[str]:
    problems = []
    entries = credits.get("figures", {})
    for name in sorted(set(files) | {s for s in shown if "://" not in s}):
        entry = entries.get(name)
        if entry is None:
            problems.append(f"{name}: figure has no entry in {CREDITS}")
            continue
        if not entry.get("licence") or not entry.get("credit"):
            problems.append(f"{name}: entry needs a licence and a credit line")
        elif entry.get("origin") != FARIS_ORIGIN and not entry.get("owner"):
            problems.append(f"{name}: a figure not captured from FARIS must name its owner")
    for name in sorted(entries):
        if name not in files:
            problems.append(f"{name}: credit entry for a figure that is not in docs/")
    for target in sorted(s for s in shown if "://" in s):
        problems.append(f"{target}: external figure; download it, credit it in {CREDITS} and store it under docs/")
    return problems


def check_footer(root: Path = ROOT) -> list[str]:
    problems = []
    book = (root / "book.toml").read_text(encoding="utf-8")
    match = re.search(r"additional-js\s*=\s*\[(.*?)\]", book)
    if not match or '"docs/licence-footer.js"' not in match.group(1):
        problems.append("book.toml: additional-js does not load docs/licence-footer.js")
    script = (root / "docs/licence-footer.js").read_text(encoding="utf-8")
    if "AGPL-3.0-only" not in script:
        problems.append("docs/licence-footer.js: does not state AGPL-3.0-only")
    if "GNU AFFERO GENERAL PUBLIC LICENSE" not in (root / "LICENSE").read_text(encoding="utf-8"):
        problems.append("LICENSE: is not the AGPL text")
    return problems


def check_built(directory: Path) -> list[str]:
    pages = [p for p in sorted(directory.rglob("*.html")) if p.name not in ("print.html", "toc.html")]
    if not pages:
        return [f"{directory}: no HTML pages"]
    return [f"{p.relative_to(directory)}: does not load the licence footer script" for p in pages
            if "licence-footer" not in p.read_text(encoding="utf-8")]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="handbook licence lint")
    ap.add_argument("--built", help="also check the pages of a built handbook")
    args = ap.parse_args(argv)
    credits = json.loads((ROOT / CREDITS).read_text(encoding="utf-8"))
    problems = check_footer() + check_credits(figure_files(), shown_figures(), credits)
    if args.built:
        problems += check_built(Path(args.built))
    for line in problems:
        print(line)
    print(f"doc licence lint: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
