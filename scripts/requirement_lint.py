#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint the requirement tables in docs/requirements/ (requirements QA-001, QA-008).

Every table row must follow the conventions in docs/requirements/README.md: the
seven columns in order with no empty cell, a well-formed unique id in the file
that owns its prefix, a known phase, a Now cell that starts with a status word
and says what is missing when the row is not met, a Basis that carries a
documented mark, and a Verification that names a method. A Met row needs a
`Verifies:` test unless its verification is an audit, review or similar check.
A withdrawn row keeps its id, struck through, in both the id and the
Requirement cell.

Rules the tables still break are listed in ADVISORY: they are printed, and fail
the run only with --strict. Everything else fails the run.

QA-008 (ids never reused) is checked against history. --against REF compares
with the tables at a git ref, normally the previous release tag: an id there
must still be present, live or struck through, a struck id must not come back,
and a live id must keep its meaning. --history does the same against every
commit that touched the tables, so a removed id is found even when no tag
carried it.
"""
from __future__ import annotations

import argparse
import difflib
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import requirement_trace as rt

HEADER = ["ID", "Requirement", "Target", "Verification", "Basis", "Phase", "Now"]
ID_FORM = re.compile(r"[A-Z][A-Z0-9]*-\d{3}")
PHASE = r"F[1-7](?: \([^()|]+\))?"
PHASES = re.compile(rf"{PHASE}(?:, {PHASE})*")
# README: Met (with evidence), Partial (what exists, what is missing), No, Unmeasured.
# No alone is the documented form; the others carry a reason after a colon.
EXPLAINED = ("Met", "Partial", "Unmeasured")
# README: an "R5 PD-01" style cell points into the research notes and counts as a FARIS choice.
BASIS_MARKS = re.compile(r"\[V\b|\[U\b|FARIS choice|\[internal\b|\bR\d\b")
METHOD = re.compile(
    r"\b(test\w*|benchmark\w*|audit\w*|stud(?:y|ies)|CI|review\w*|inspection|check\w*|script\w*|suite|"
    r"analys[ie]s|measure\w*|trial\w*|demonstration|survey|run|runs|lint\w*|fuzz\w*|compar\w*|"
    r"harness|scan|trace|regression|fixture\w*|validat\w*|verif\w*|probe|monitor\w*|inventory|"
    r"experiment\w*|session|checklist|simulat\w*|replay|inject\w*|calculation|report|log|"
    r"statistics|dashboard|diff|profil\w*|timing|stopwatch|panel|usability|walkthrough|"
    r"evaluation|assessment|rehearsal|drill|reproduc\w*|replicat\w*|cross-?check\w*|"
    r"mutation|coverage|sweep|corpus|pair\w*|worked example|compile\w*|build\w*|scorecard)\b",
    re.IGNORECASE,
)
# A Met row without a test is accepted when it is decided by one of these checks.
NON_TEST = re.compile(r"\b(audit|review|inspection|scan|trace|check|CI|lint|document|study|benchmark|"
                      r"cargo-\w+|clippy)\b", re.IGNORECASE)
# Rules the tables break today and that cannot be fixed by a mechanical edit.
ADVISORY = {"now-explain", "no-pipe", "basis-mark", "method", "met-traced"}
RULES = ("header", "cells", "empty-cell", "id-form", "id-unique", "id-prefix", "phase", "now-status",
         "now-explain", "no-pipe", "basis-mark", "method", "met-traced", "struck-form",
         "id-removed", "id-revived", "id-reused")
REUSE_SIMILARITY = 0.4

Finding = tuple[str, str, str]


def lint_file(name: str, text: str) -> tuple[list[Finding], list[dict]]:
    """Per-row rules for one file; returns (findings, rows) with rows for the cross-file rules."""
    findings: list[Finding] = []
    rows: list[dict] = []
    lines = text.splitlines()
    for number, line in enumerate(lines, 1):
        if line.startswith("| ID |"):
            columns = [c.strip() for c in rt.CELL_SPLIT.split(line.strip())[1:-1]]
            if columns != HEADER:
                findings.append(("header", f"{name}:{number}", f"columns are {columns}, expected {HEADER}"))
    for number, struck, rid, cells in rt.table_rows(text):
        where = f"{name}:{number}"
        label = rid or (cells[0] if cells else "?")
        if not rid:
            findings.append(("id-form", where, f"first cell {label!r} is not an id like ABC-001, or ~~ABC-001~~ when withdrawn"))
        elif not ID_FORM.fullmatch(rid):
            findings.append(("id-form", where, f"{rid} is not a well-formed id"))
        if len(cells) != 7:
            findings.append(("cells", where, f"{label} has {len(cells)} cells, expected 7"))
            continue
        for column, cell in zip(HEADER, cells):
            if not cell:
                findings.append(("empty-cell", where, f"{label} has an empty {column} cell"))
        if "\\|" in lines[number - 1]:
            findings.append(("no-pipe", where, f"{label} has an escaped pipe inside a cell; reword so no cell contains one"))
        if struck and not re.fullmatch(r"~~.+~~", cells[1]):
            findings.append(("struck-form", where, f"{label} is withdrawn but its Requirement cell is not struck through"))
        if not struck and rid and cells[1].startswith("~~"):
            findings.append(("struck-form", where, f"{label} has a struck-through Requirement but a live id; strike the id too"))
        if cells[5] and not PHASES.fullmatch(cells[5]):
            findings.append(("phase", where, f"{label} Phase {cells[5]!r} is not F1-F7 (compound form: 'F1 (what), F7 (what)')"))
        if struck or not rid:
            continue
        status = cells[6].split(":")[0].strip()
        if status not in rt.STATUSES:
            findings.append(("now-status", where, f"{label} Now starts with {status!r}, expected one of {rt.STATUSES}"))
        elif status in EXPLAINED and not (":" in cells[6] and cells[6].split(":", 1)[1].strip()):
            findings.append(("now-explain", where, f"{label} Now is {status} with no reason after the colon; say what exists or is missing"))
        if cells[4] and not BASIS_MARKS.search(cells[4]):
            findings.append(("basis-mark", where, f"{label} Basis has no [V], [U], FARIS choice, [internal] or research-note mark"))
        if cells[3] and not METHOD.search(cells[3]):
            findings.append(("method", where, f"{label} Verification {cells[3][:50]!r} names no test, benchmark, audit or study"))
        rows.append({"id": rid, "where": where, "now": status, "verification": cells[3]})
    return findings, rows


def lint_tables(files: dict[str, str], trace: dict[str, list[str]]) -> list[Finding]:
    """All table rules over a set of files (name -> text) and the Verifies: trace."""
    findings: list[Finding] = []
    seen: dict[str, str] = {}
    prefix_file: dict[str, str] = {}
    for name in sorted(files):
        file_findings, rows = lint_file(name, files[name])
        findings += file_findings
        for _, _, rid, _ in rt.table_rows(files[name]):
            if not rid:
                continue
            if rid in seen:
                findings.append(("id-unique", name, f"{rid} appears in {seen[rid]} and {name}" if seen[rid] != name
                                 else f"{rid} appears twice in {name}"))
            seen.setdefault(rid, name)
            prefix = rid.rsplit("-", 1)[0]
            owner = prefix_file.setdefault(prefix, name)
            if owner != name:
                findings.append(("id-prefix", name, f"{rid} is in {name} but prefix {prefix} belongs to {owner}"))
        for row in rows:
            if row["now"] == "Met" and row["id"] not in trace and not NON_TEST.search(row["verification"]):
                findings.append(("met-traced", row["where"],
                                 f"{row['id']} is Met but no test carries 'Verifies: {row['id']}' and its verification is not an audit or review"))
    return findings


def table_ids(files: dict[str, str]) -> dict[str, tuple[bool, str]]:
    """id -> (withdrawn, requirement text) for every row of a set of files."""
    out: dict[str, tuple[bool, str]] = {}
    for text in files.values():
        for _, struck, rid, cells in rt.table_rows(text):
            if rid and len(cells) > 1:
                out[rid] = (struck, cells[1].strip("~ "))
    return out


def lint_history(old: dict[str, tuple[bool, str]], new: dict[str, tuple[bool, str]], label: str) -> list[Finding]:
    """QA-008: compare the ids of an earlier state with the current tables."""
    findings: list[Finding] = []
    for rid, (was_struck, was_text) in sorted(old.items()):
        if rid not in new:
            findings.append(("id-removed", label, f"{rid} existed at {label} and is gone; keep it as a struck-through tombstone"))
            continue
        now_struck, now_text = new[rid]
        if was_struck and not now_struck:
            findings.append(("id-revived", label, f"{rid} was withdrawn at {label} and is live again; take a new id"))
        elif not was_struck and not now_struck:
            ratio = difflib.SequenceMatcher(None, was_text, now_text).ratio()
            if ratio < REUSE_SIMILARITY:
                findings.append(("id-reused", label, f"{rid} now states a different requirement than at {label} "
                                                     f"(similarity {ratio:.2f}); withdraw it and take a new id"))
    return findings


def git(*args: str) -> str:
    return subprocess.run(["git", "-C", str(rt.ROOT), *args], check=True, capture_output=True, text=True).stdout


def files_at(ref: str) -> dict[str, str]:
    names = git("ls-tree", "--name-only", ref, "docs/requirements/").split()
    return {Path(n).name: git("show", f"{ref}:{n}") for n in names if re.fullmatch(r"[0-9][0-9]-.*\.md", Path(n).name)}


def ids_ever_added() -> set[str]:
    """Every id that appeared as a table row in any commit that touched the tables."""
    out = git("log", "--format=", "-p", "--no-color", "--", ":(glob)docs/requirements/[0-9][0-9]-*.md")
    pattern = re.compile(r"^[+-]\| (?:~~)?([A-Z][A-Z0-9]*-\d{3})(?:~~)? \|", re.MULTILINE)
    return set(pattern.findall(out))


def current_files() -> dict[str, str]:
    return {p.name: p.read_text(encoding="utf-8") for p in sorted(rt.REQUIREMENTS.glob("[0-9][0-9]-*.md"))}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--strict", action="store_true", help="advisory rules also fail the run")
    parser.add_argument("--against", metavar="REF",
                        help="compare ids with the tables at a git ref (QA-008), e.g. the previous release tag")
    parser.add_argument("--history", action="store_true",
                        help="check that no id ever added to the tables has been dropped (QA-008)")
    args = parser.parse_args()
    files = current_files()
    trace, _ = rt.read_markers()
    findings = lint_tables(files, trace)
    new = table_ids(files)
    if args.against:
        findings += lint_history(table_ids(files_at(args.against)), new, args.against)
    if args.history:
        findings += [("id-removed", "history", f"{rid} was added in an earlier commit and is gone; keep it as a struck-through tombstone")
                     for rid in sorted(ids_ever_added() - set(new))]
    errors = [f for f in findings if f[0] not in ADVISORY]
    advisory = [f for f in findings if f[0] in ADVISORY]
    for rule, where, message in errors:
        print(f"error [{rule}] {where}: {message}", file=sys.stderr)
    for rule, where, message in advisory:
        print(f"advisory [{rule}] {where}: {message}", file=sys.stderr)
    counts = ", ".join(f"{r} {sum(1 for f in advisory if f[0] == r)}" for r in RULES if any(f[0] == r for f in advisory))
    print(f"{len(new)} requirements linted: {len(errors)} errors, {len(advisory)} advisory" + (f" ({counts})" if counts else ""),
          file=sys.stderr)
    return 1 if errors or (args.strict and advisory) else 0


if __name__ == "__main__":
    sys.exit(main())
