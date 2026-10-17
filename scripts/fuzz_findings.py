#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Check that every fuzz finding has a committed reproducer (requirement SEC-022).

    python3 scripts/fuzz_findings.py [--log FILE] [--root DIR] [--release]

The log is docs/fuzz-findings.json: {"findings": [...]}. One entry per crash, hang or out-of-memory finding of
a fuzz target, whether it came from libFuzzer, the smoke driver or a report from outside. Each entry has:

    id           FUZZ-NNN, unique
    target       a file name (without .rs) in fuzz/fuzz_targets
    found        YYYY-MM-DD
    status       open or fixed
    summary      one line: what the input did
    reproducer   "path/to/file::test_name": an ordinary test in the crate that owns the parser, with the
                 minimised input in it; the file must exist and contain the name. Required when fixed.
    fixed_in     commit or version; required when fixed

The reproducer rule: a finding is fixed only with a committed reproducer test (docs/FUZZING.md). An open finding
is allowed between releases; with --release (and in the release gate) any open finding fails, which is "a
reproducer within one release". The log is empty while no target has produced a finding. One more check keeps
it honest: more files in fuzz/artifacts/ than findings fail (the directory is ignored by git, so every finding
the fuzzer wrote there has to be accounted for by an entry).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from defect_log import test_problems, text  # noqa: E402

ROOT = HERE.parent
LOG = Path("docs") / "fuzz-findings.json"
TARGETS = Path("fuzz") / "fuzz_targets"
ID = re.compile(r"^FUZZ-\d{3,}$")


def lint(data, root: Path = ROOT, release: bool = False) -> list[str]:
    findings = data.get("findings") if isinstance(data, dict) else None
    if not isinstance(findings, list):
        return ["the log has no 'findings' list"]
    targets = {p.stem for p in (root / TARGETS).glob("*.rs")}
    out, seen = [], set()
    for f in findings:
        if not isinstance(f, dict):
            out.append("an entry is not an object")
            continue
        name = f.get("id") if text(f.get("id")) else "(no id)"

        def bad(message: str) -> None:
            out.append(f"{name}: {message}")

        if not (text(f.get("id")) and ID.match(f["id"])):
            bad("id is not FUZZ-NNN")
        if name in seen:
            bad("id is used twice")
        seen.add(name)
        if f.get("target") not in targets:
            bad(f"target {f.get('target')!r} is not a file in {TARGETS.as_posix()}")
        try:
            date.fromisoformat(f["found"])
        except (KeyError, TypeError, ValueError):
            bad("found is not a YYYY-MM-DD date")
        if not text(f.get("summary")):
            bad("no summary")
        status = f.get("status")
        if status not in ("open", "fixed"):
            bad(f"status {status!r} is not open or fixed")
        elif status == "fixed":
            if not text(f.get("fixed_in")):
                bad("fixed but no fixed_in")
            out.extend(f"{name}: {p}" for p in test_problems(f.get("reproducer"), root))
        elif release:
            bad("open at a release: it needs a committed reproducer and a fix first")
    artifacts = root / "fuzz" / "artifacts"
    if artifacts.is_dir():
        files = [p for p in artifacts.rglob("*") if p.is_file()]
        if len(files) > len(findings):
            out.append(f"{len(files)} files in fuzz/artifacts but only {len(findings)} findings are logged")
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    ap.add_argument("--log", type=Path, help=f"findings log (default: {LOG} under --root)")
    ap.add_argument("--release", action="store_true", help="also fail on an open finding")
    args = ap.parse_args(argv)
    path = args.log or args.root / LOG
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"fuzz findings {path}: cannot be read ({e})", file=sys.stderr)
        return 1
    problems = lint(data, args.root, args.release)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        print(f"{len(problems)} problems in the fuzz findings log", file=sys.stderr)
        return 1
    print(f"{len(data['findings'])} fuzz findings logged; each fixed one has a committed reproducer test")
    return 0


if __name__ == "__main__":
    sys.exit(main())
