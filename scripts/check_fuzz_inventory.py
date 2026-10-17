#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The parser inventory against the fuzz targets (requirement SEC-020).

    python3 scripts/check_fuzz_inventory.py

The inventory is the table in docs/FUZZING.md ("What is covered"): one row per fuzz target, naming what it reads and
the entry points it calls. This check fails when
- a row names a target that has no file in fuzz/fuzz_targets, or a target file has no row;
- a target file does not call an entry function of fuzz/entries/src/lib.rs, or that function is missing or not
  listed in ENTRIES (the list the scripts and the seed test run);
- an entry point written as a path (`faris_model::design::parse_design`) names a last segment that does not appear in
  fuzz/entries/src/lib.rs, so the table cannot claim a parser the entries do not call;
- the page has no list of what is not covered, with a reason for each.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PAGE = Path("docs/FUZZING.md")
ENTRIES = Path("fuzz/entries/src/lib.rs")
TARGETS = Path("fuzz/fuzz_targets")
ROW = re.compile(r"^\|\s*`([a-z_0-9]+)`\s*\|(.*?)\|(.*)\|\s*$")
PATH_ITEM = re.compile(r"`([A-Za-z_]\w*(?:::\w+)+)`")


def table_rows(page: str) -> dict[str, str]:
    """Target name -> its entry-point cell."""
    start = page.index("## What is covered")
    end = page.find("\n## ", start + 1)
    rows = {}
    for line in page[start:end if end > 0 else None].splitlines():
        m = ROW.match(line)
        if m:
            rows[m.group(1)] = m.group(3)
    return rows


def listed_functions(entries: str) -> set[str]:
    start = entries.index("pub const ENTRIES")
    block = entries[start:entries.index("];", start)]
    return {fn for _, fn in re.findall(r'\(\s*"(\w+)"\s*,\s*(\w+)\s*\)', block)}


def check(root: Path = ROOT) -> list[str]:
    page = (root / PAGE).read_text(encoding="utf-8")
    entries = (root / ENTRIES).read_text(encoding="utf-8")
    rows = table_rows(page)
    problems = []
    files = {p.stem: p.read_text(encoding="utf-8") for p in sorted((root / TARGETS).glob("*.rs"))}
    functions = listed_functions(entries)
    for name in sorted(set(rows) - set(files)):
        problems.append(f"{PAGE} lists the target {name}, which has no file in {TARGETS}")
    for name in sorted(set(files) - set(rows)):
        problems.append(f"{TARGETS}/{name}.rs is a fuzz target with no row in {PAGE}")
    for name, text in sorted(files.items()):
        called = re.findall(r"faris_fuzz_entries::(\w+)\(", text)
        if not called:
            problems.append(f"{TARGETS}/{name}.rs calls no function of faris_fuzz_entries")
        for fn in called:
            if not re.search(rf"^pub fn {fn}\(", entries, re.M):
                problems.append(f"{TARGETS}/{name}.rs calls {fn}, which {ENTRIES} does not define")
            elif fn not in functions:
                problems.append(f"{fn} is not listed in ENTRIES of {ENTRIES}")
    for name, cell in sorted(rows.items()):
        for item in PATH_ITEM.findall(cell):
            last = item.rsplit("::", 1)[1]
            if not re.search(rf"\b{re.escape(last)}\b", entries):
                problems.append(f"{PAGE}: target {name} claims {item}, but {ENTRIES} never calls {last}")
    if "Not covered by a cargo-fuzz target, and why" not in page:
        problems.append(f"{PAGE} has no list of what is not covered by a cargo-fuzz target and why")
    return problems


def main() -> int:
    problems = check()
    for p in problems:
        print(p)
    rows = table_rows((ROOT / PAGE).read_text(encoding="utf-8"))
    print(f"fuzz inventory: {len(rows)} targets, {len(problems)} problem(s)", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
