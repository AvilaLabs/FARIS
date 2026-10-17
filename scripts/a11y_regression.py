#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Accessibility regression check between releases (requirement A11Y-055).

    python3 scripts/a11y_regression.py snapshot --tests FILE --out FILE [--root DIR]
    python3 scripts/a11y_regression.py compare  --baseline FILE --current FILE

The automated accessibility checks are the tests that carry a `Verifies: A11Y-nnn` marker for A11Y-001 to
A11Y-046. `snapshot` derives one verdict per requirement from the test evidence of a release (the JSON
{"commit": SHA, "checks": [{"name": ..., "verdict": "PASS"}]} the release gate reads): PASS when every
marked test of the ID has a PASS check, FAIL when one has another verdict, NOT_EVALUATED when no marked
test has a check. A snapshot is {"commit": SHA, "results": {"A11Y-001": "PASS", ...}} and is kept with the
release record. `compare` finds the new failures between the previous release's snapshot and this one: an ID
that is not PASS now and was PASS before, or that is FAIL now and was not FAIL before. An ID that was not PASS
before and still is not PASS is a known gap and is listed, not counted. Exit status 1 on any new failure.
scripts/release_gate.py runs the same comparison as the condition "no new accessibility failures".
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import requirement_trace as rt  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FIRST, LAST = 1, 46
PASS, FAIL, NOT_EVALUATED = "PASS", "FAIL", "NOT_EVALUATED"


def a11y_ids() -> list[str]:
    return [f"A11Y-{n:03d}" for n in range(FIRST, LAST + 1)]


def marked_tests(root: Path) -> dict[str, set[str]]:
    """A11Y-001..046 -> names of the tests whose Verifies: marker names them (as in the requirement trace)."""
    wanted = set(a11y_ids())
    found: dict[str, set[str]] = {}
    for top in rt.SOURCE_DIRS:
        for path in sorted((root / top).rglob("*")):
            if path.suffix not in rt.SOURCE_SUFFIXES or "target" in path.parts or "__pycache__" in path.parts:
                continue
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            fn_pattern = rt.RUST_FN if path.suffix == ".rs" else rt.PY_FN
            for index, line in enumerate(lines):
                marker = rt.MARKER.search(line)
                ids = wanted.intersection(rt.ID.findall(marker.group(1))) if marker else set()
                if not ids:
                    continue
                name = next((m.group(1) for following in lines[index:index + 12] if (m := fn_pattern.match(following))), None)
                if name:
                    for rid in ids:
                        found.setdefault(rid, set()).add(name)
    return found


def _verdict_of(name: str, checks: list[dict]) -> str | None:
    hits = [c.get("verdict") for c in checks if isinstance(c, dict) and isinstance(c.get("name"), str)
            and re.split(r"::|\.|/", c["name"].strip())[-1] == name]
    if not hits:
        return None
    return PASS if all(h == PASS for h in hits) else FAIL


def snapshot(tests: dict, root: Path = ROOT) -> dict:
    checks = tests.get("checks")
    if not isinstance(checks, list) or not checks:
        raise ValueError("the test evidence lists no checks")
    marked = marked_tests(root)
    results = {}
    for rid in a11y_ids():
        verdicts = [v for v in (_verdict_of(n, checks) for n in sorted(marked.get(rid, ()))) if v]
        results[rid] = NOT_EVALUATED if not verdicts else PASS if all(v == PASS for v in verdicts) else FAIL
    return {"commit": tests.get("commit"), "results": results}


def compare(baseline: dict, current: dict) -> tuple[list[str], list[str]]:
    """(new failures, known gaps) between two snapshots."""
    for what, snap in (("baseline", baseline), ("current", current)):
        if not isinstance(snap, dict) or not isinstance(snap.get("results"), dict):
            raise ValueError(f"the {what} snapshot has no results")
    before, now = baseline["results"], current["results"]
    new, known = [], []
    for rid in a11y_ids():
        b, c = before.get(rid, NOT_EVALUATED), now.get(rid, NOT_EVALUATED)
        if c != PASS and (b == PASS or (c == FAIL and b != FAIL)):
            new.append(f"{rid}: {b} -> {c}")
        elif c != PASS:
            known.append(f"{rid}: {c}")
    return new, known


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="accessibility regressions between releases")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("snapshot")
    s.add_argument("--tests", type=Path, required=True)
    s.add_argument("--out", type=Path, required=True)
    s.add_argument("--root", type=Path, default=ROOT)
    c = sub.add_parser("compare")
    c.add_argument("--baseline", type=Path, required=True)
    c.add_argument("--current", type=Path, required=True)
    args = ap.parse_args(argv)
    try:
        if args.cmd == "snapshot":
            snap = snapshot(json.loads(args.tests.read_text(encoding="utf-8")), args.root)
            args.out.write_text(json.dumps(snap, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            counts = {v: sum(1 for x in snap["results"].values() if x == v) for v in (PASS, FAIL, NOT_EVALUATED)}
            print(f"snapshot: {counts}")
            return 0
        new, known = compare(*(json.loads(p.read_text(encoding="utf-8")) for p in (args.baseline, args.current)))
    except (ValueError, OSError) as e:
        print(f"a11y regression: {e}", file=sys.stderr)
        return 1
    for line in new:
        print(f"NEW FAILURE {line}")
    print(f"a11y regression: {len(new)} new failure(s), {len(known)} known gap(s) not PASS in the current release")
    return 1 if new else 0


if __name__ == "__main__":
    sys.exit(main())
