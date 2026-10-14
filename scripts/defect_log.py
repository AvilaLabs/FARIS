#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint of FARIS's own defect log (requirement QA-080).

    python3 scripts/defect_log.py [--log FILE] [--root DIR]

The log is docs/defects/faris-defects.json: {"defects": [...]}. It is also the evidence file the release gate
reads with `--defects` (scripts/release_gate.py, QA-083), so no separate evidence is written. Each entry has:

    id                 FARIS-NNN, unique
    title              one line
    severity           S1 wrong physics answer without warning; S2 wrong answer with a warning, or loss of
                       user work or data; S3 function fails or misleads but the user is told or a workaround
                       exists; S4 cosmetic (definitions in docs/defects/README.md)
    status             open, fixed or wont-fix
    found              YYYY-MM-DD
    affected_versions  released versions (X.Y.Z), or ["unreleased"] if it never reached a release
    fixed_in           commit or version; required when fixed
    disposition        what was wrong and what was done
    regression_test    fixed: "path/to/file::test_name", the file must exist and contain the name;
                       wont-fix: the reason; open: null is allowed
    waiver             optional, as the release gate defines it (open S2 only)

Every problem is listed; exit status 1 if there is any. Upstream OpenMC drafts in docs/defects/*.md are not
part of this log.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOG = Path("docs") / "defects" / "faris-defects.json"
SEVERITIES = ("S1", "S2", "S3", "S4")
STATUSES = ("open", "fixed", "wont-fix")
ID = re.compile(r"^FARIS-\d{3,}$")
VERSION = re.compile(r"^\d+\.\d+\.\d+$")


def text(value) -> bool:
    return isinstance(value, str) and bool(value.strip())


def lint_entry(d, root: Path) -> list[str]:
    """The problems of one entry, each prefixed with its id."""
    if not isinstance(d, dict):
        return ["an entry is not an object"]
    name = d.get("id") if text(d.get("id")) else "(no id)"
    out = []

    def bad(message: str) -> None:
        out.append(f"{name}: {message}")

    if not (text(d.get("id")) and ID.match(d["id"])):
        bad("id is not FARIS-NNN")
    if not text(d.get("title")):
        bad("no title")
    if d.get("severity") not in SEVERITIES:
        bad(f"severity {d.get('severity')!r} is not one of S1 to S4")
    status = d.get("status")
    if status not in STATUSES:
        bad(f"status {status!r} is not open, fixed or wont-fix")
    try:
        date.fromisoformat(d["found"])
    except (KeyError, TypeError, ValueError):
        bad("found is not a YYYY-MM-DD date")
    versions = d.get("affected_versions")
    if not (isinstance(versions, list) and versions and all(
            isinstance(v, str) and (v == "unreleased" or VERSION.match(v)) for v in versions)):
        bad('affected_versions must be a list of released versions (X.Y.Z) or ["unreleased"]')
    if not text(d.get("disposition")):
        bad("no disposition")
    if status == "fixed" and not text(d.get("fixed_in")):
        bad("fixed but no fixed_in")
    test = d.get("regression_test")
    if status == "fixed":
        out.extend(f"{name}: {p}" for p in test_problems(test, root))
    elif status == "wont-fix" and not text(test):
        bad("wont-fix but regression_test gives no reason")
    if d.get("waiver") is not None and not (status == "open" and d.get("severity") == "S2"):
        bad("a waiver applies only to an open S2 defect")
    return out


def test_problems(test, root: Path) -> list[str]:
    if not text(test):
        return ["fixed but regression_test is empty: name the test that now prevents recurrence"]
    path, sep, function = test.partition("::")
    if not sep or not path.strip() or not function.strip():
        return [f"regression_test {test!r} is not 'path::test_name'"]
    file = root / path.strip()
    if not file.is_file():
        return [f"regression_test file {path} does not exist"]
    if not re.search(rf"\b{re.escape(function.strip())}\b", file.read_text(encoding="utf-8", errors="replace")):
        return [f"regression_test {function} does not occur in {path}"]
    return []


def lint(data, root: Path = ROOT) -> list[str]:
    defects = data.get("defects") if isinstance(data, dict) else None
    if not isinstance(defects, list):
        return ["the log has no 'defects' list"]
    problems = []
    seen = set()
    for d in defects:
        problems.extend(lint_entry(d, root))
        ident = d.get("id") if isinstance(d, dict) else None
        if ident in seen:
            problems.append(f"{ident}: id is used twice")
        seen.add(ident)
    return problems


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    ap.add_argument("--log", type=Path, help=f"defect log (default: {LOG} under --root)")
    args = ap.parse_args(argv)
    path = args.log or args.root / LOG
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"defect log {path}: cannot be read ({e})", file=sys.stderr)
        return 1
    problems = lint(data, args.root)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        print(f"{len(problems)} problems in the defect log", file=sys.stderr)
        return 1
    print(f"{len(data['defects'])} defects logged; every entry has severity, status, affected versions, "
          "disposition and a regression test")
    return 0


if __name__ == "__main__":
    sys.exit(main())
