#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Merge audit: every fixed S1 to S3 defect links the commit that fixed it to a test (requirement QA-085).

    python3 scripts/merge_audit.py [--root DIR] [--log FILE]

For each defect in docs/defects/faris-defects.json with status "fixed" and severity S1, S2 or S3 the audit
requires that
- `fixed_in` names a commit of this repository's history (a hash or any ref git resolves to a commit), and
- the test named by `regression_test` ("path::name") exists in that commit: the file is in the commit's tree and
  contains the name. A test that arrived only after the fix therefore fails the audit.

A fix that predates the policy may record `retrospective_test`: the commit that added its test afterwards. The
audit then requires the test at that commit instead, so a late test is visible in the log and not mistaken for a
failing-first one.

scripts/defect_log.py checks the same link against the working tree; this script checks it at the fix commit
and needs the full history (the CI job `history` fetches it). That the test failed before the fix is the
reviewer's check, required by the pull request template; a script cannot decide it.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOG = Path("docs") / "defects" / "faris-defects.json"
AUDITED = ("S1", "S2", "S3")


def git(root: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True, check=False)


def audit_defect(d: dict, root: Path) -> list[str]:
    name = d.get("id", "(no id)")
    ref = str(d.get("fixed_in") or "").strip()
    path, _, function = str(d.get("regression_test") or "").partition("::")
    if not ref:
        return [f"{name}: fixed but no fixed_in commit"]
    if not path.strip() or not function.strip():
        return [f"{name}: regression_test is not 'path::test_name'"]
    resolved = git(root, "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}")
    if resolved.returncode != 0:
        return [f"{name}: fixed_in {ref!r} is not a commit in this history; name the commit that fixed it"]
    late = str(d.get("retrospective_test") or "").strip()
    if late:
        later = git(root, "rev-parse", "--verify", "--quiet", f"{late}^{{commit}}")
        if later.returncode != 0:
            return [f"{name}: retrospective_test {late!r} is not a commit in this history"]
        ref, resolved = late, later
    commit = resolved.stdout.strip()
    shown = git(root, "show", f"{commit}:{path.strip()}")
    if shown.returncode != 0:
        return [f"{name}: {path.strip()} does not exist at fix commit {ref}; the fix merged without its test"]
    if not re.search(rf"\b{re.escape(function.strip())}\b", shown.stdout):
        return [f"{name}: {function.strip()} is not in {path.strip()} at fix commit {ref}; the fix merged without its test"]
    return []


def audit(defects: list[dict], root: Path) -> list[str]:
    problems = []
    for d in defects:
        if isinstance(d, dict) and d.get("status") == "fixed" and d.get("severity") in AUDITED:
            problems.extend(audit_defect(d, root))
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--log", type=Path)
    args = ap.parse_args(argv)
    defects = json.loads((args.log or args.root / LOG).read_text(encoding="utf-8"))["defects"]
    problems = audit(defects, args.root)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        print(f"merge audit: FAIL, {len(problems)} fixes without a test at their commit", file=sys.stderr)
        return 1
    count = sum(1 for d in defects if d.get("status") == "fixed" and d.get("severity") in AUDITED)
    print(f"merge audit: PASS, {count} fixed S1 to S3 defects each have their test at the fix commit")
    return 0


if __name__ == "__main__":
    sys.exit(main())
