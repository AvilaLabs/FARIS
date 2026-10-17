#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Fix-time check for advisories in the sign-in client stack (requirement SEC-039).

    python3 scripts/advisory_log.py [--log FILE] [--root DIR] [--today YYYY-MM-DD]

The log is docs/advisories/signin-advisories.json:

    {"packages": ["avila-account", "ureq", ...],
     "advisories": [{"id": "RUSTSEC-...", "package": "ureq", "severity": "critical", "published": "YYYY-MM-DD",
                     "status": "fixed", "fixed_on": "YYYY-MM-DD", "fixed_in": "0.2.1", "note": "..."}]}

`packages` is the sign-in stack: the account client and the HTTP and TLS crates it uses. Each must be in
Cargo.lock, so the list cannot rot when a crate is replaced. `scripts/check_audit.py` finds the advisories in
CI (SEC-031); an advisory that touches one of these packages is recorded here, with the day it was published
and the day a FARIS release carried the fix. The time allowed from publication to the release is 7 days for a
critical advisory and 30 days for a high one (provisional, R5 SC-20). The check fails for:

- a fixed advisory whose release came after the limit (late);
- an open advisory that is past its limit as of today (overdue);
- a missing field, an unknown severity or package, a repeated id, a fix before publication.

Medium and low advisories are recorded but have no limit here; they follow the ordinary audit waivers.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOG = Path("docs") / "advisories" / "signin-advisories.json"
LIMIT_DAYS = {"critical": 7, "high": 30}
SEVERITIES = ("critical", "high", "medium", "low")


def day(value):
    try:
        return date.fromisoformat(value)
    except (TypeError, ValueError):
        return None


def lockfile_packages(root: Path) -> set[str]:
    path = root / "Cargo.lock"
    if not path.is_file():
        return set()
    return set(re.findall(r'^name = "([^"]+)"', path.read_text(encoding="utf-8"), flags=re.M))


def lint(data, root: Path = ROOT, today: date | None = None) -> list[str]:
    today = today or date.today()
    if not isinstance(data, dict) or not isinstance(data.get("packages"), list) or not isinstance(data.get("advisories"), list):
        return ["the log needs a 'packages' list and an 'advisories' list"]
    out = []
    packages = data["packages"]
    if not packages:
        out.append("the sign-in package list is empty")
    locked = lockfile_packages(root)
    for p in packages:
        if p not in locked:
            out.append(f"package {p} is not in Cargo.lock: update the list")
    seen = set()
    for a in data["advisories"]:
        name = a.get("id") if isinstance(a, dict) and isinstance(a.get("id"), str) and a["id"].strip() else "(no id)"
        if not isinstance(a, dict):
            out.append("an advisory is not an object")
            continue

        def bad(message: str) -> None:
            out.append(f"{name}: {message}")

        if name in seen:
            bad("id is used twice")
        seen.add(name)
        if a.get("package") not in packages:
            bad(f"package {a.get('package')!r} is not in the sign-in package list")
        severity = a.get("severity")
        if severity not in SEVERITIES:
            bad(f"severity {severity!r} is not one of {', '.join(SEVERITIES)}")
        published = day(a.get("published"))
        if published is None:
            bad("published is not a YYYY-MM-DD date")
            continue
        status = a.get("status")
        if status not in ("open", "fixed"):
            bad(f"status {status!r} is not open or fixed")
            continue
        limit = LIMIT_DAYS.get(severity)
        deadline = published + timedelta(days=limit) if limit else None
        if status == "fixed":
            fixed = day(a.get("fixed_on"))
            if fixed is None or not (isinstance(a.get("fixed_in"), str) and a["fixed_in"].strip()):
                bad("fixed needs fixed_on (YYYY-MM-DD) and fixed_in (the release)")
            elif fixed < published:
                bad("fixed_on is before published")
            elif deadline and fixed > deadline:
                bad(f"fixed {(fixed - published).days} days after publication; {severity} allows {limit}")
        elif deadline and today > deadline:
            bad(f"open {(today - published).days} days after publication; {severity} allows {limit}")
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    ap.add_argument("--log", type=Path, help=f"advisory log (default: {LOG} under --root)")
    ap.add_argument("--today", help="the date to judge open advisories by (default: today)")
    args = ap.parse_args(argv)
    path = args.log or args.root / LOG
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"advisory log {path}: cannot be read ({e})", file=sys.stderr)
        return 1
    today = day(args.today) if args.today else None
    if args.today and today is None:
        print("--today is not a YYYY-MM-DD date", file=sys.stderr)
        return 1
    problems = lint(data, args.root, today)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        print(f"{len(problems)} problems in the advisory log", file=sys.stderr)
        return 1
    print(f"{len(data['advisories'])} sign-in advisories logged over {len(data['packages'])} packages; "
          "none is late or overdue")
    return 0


if __name__ == "__main__":
    sys.exit(main())
