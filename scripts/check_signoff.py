#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Every commit carries a Signed-off-by line for its author, maintainers included (requirement LEG-005).

    python3 scripts/check_signoff.py REV_RANGE [--root DIR] [--since YYYY-MM-DD]

REV_RANGE is a git revision range such as BASE..HEAD, or a single commit (checked alone). Merge commits are
skipped. A commit passes when its message has a `Signed-off-by:` line whose address is the author's, the Developer
Certificate of Origin sign-off that CONTRIBUTING.md asks for. Commits authored before --since (the day the
policy took effect, default 2026-10-18) are not checked: history is not rewritten. The CI job `sign-off` runs this
over every pull request and every push.
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POLICY_START = "2026-10-18"
RECORD = "\x1e"
FIELD = "\x1f"


def commits(root: Path, rev_range: str) -> list[dict]:
    args = ["git", "-C", str(root), "log", "--no-merges", f"--format={RECORD}%H{FIELD}%ae{FIELD}%as{FIELD}%B"]
    args += [rev_range] if ".." in rev_range else ["-1", rev_range]
    out = subprocess.run(args, capture_output=True, text=True, check=True).stdout
    found = []
    for record in out.split(RECORD):
        if record.strip():
            sha, email, day, body = record.split(FIELD, 3)
            found.append({"sha": sha.strip(), "email": email.strip(), "date": day.strip(), "body": body})
    return found


def signed(commit: dict) -> bool:
    wanted = f"<{commit['email']}>".lower()
    return any(re.match(r"(?i)signed-off-by:\s", line) and wanted in line.lower() for line in commit["body"].splitlines())


def problems(found: list[dict], since: str = POLICY_START) -> list[str]:
    limit = date.fromisoformat(since)
    return [f"{c['sha'][:12]} by {c['email']} has no Signed-off-by line for that address; amend it with git commit -s --amend"
            for c in found if date.fromisoformat(c["date"]) >= limit and not signed(c)]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("rev_range")
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--since", default=POLICY_START)
    args = ap.parse_args(argv)
    found = commits(args.root, args.rev_range)
    bad = problems(found, args.since)
    for line in bad:
        print(f"check_signoff: {line}", file=sys.stderr)
    print(f"check_signoff: {len(found)} commits examined, {len(bad)} without sign-off")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
