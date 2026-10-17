#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The per-release quality scorecard: at most 15 measures, each one that gates a release (requirement QA-007).

    python3 scripts/release_scorecard.py [--gate FILE] [--out FILE]

Every measure has a formula, a threshold and a data source, and none is empty. The measures are the release
gate's own conditions (scripts/release_gate.py) that decide a release, chosen so the card stays short: a metric
that does not gate a release is not on it. With --gate FILE, the output of `release_gate.py --archive FILE`, each
row also shows that condition's verdict for the release (PASS, WAIVED, FAIL, or "not run" when the gate output
has no line for it); without it every verdict reads "not run". The card is written to --out or printed, and is
meant to be stored with the release record. The requirement-status scorecard (docs/requirements/SCORECARD.md,
scripts/requirement_scorecard.py) is a different, per-commit document.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

MAX_MEASURES = 15
FIELDS = ("name", "formula", "threshold", "source", "ident")

# name, formula, threshold, data source, the release-gate condition id that decides it.
MEASURES = (
    {"name": "Release-gate requirements Met", "formula": "requirement ids in the NEXT_RELEASE_PLAN release-gate list whose Now cell is Met, divided by the list length",
     "threshold": "100 %", "source": "docs/NEXT_RELEASE_PLAN.md and docs/requirements/*.md", "ident": "NEXT_RELEASE_PLAN"},
    {"name": "Requirement-to-test trace", "formula": "release-gate ids with a Verifies: marker, plus markers naming an unknown id",
     "threshold": "every id traced, 0 unknown markers", "source": "Verifies: markers in the source tree", "ident": "QA-002"},
    {"name": "Met requirements with a test marker", "formula": "requirements marked Met that no test names",
     "threshold": "0", "source": "requirement tables and Verifies: markers", "ident": "QA-071"},
    {"name": "Test suite", "formula": "checks with a verdict other than PASS in the test evidence of this commit",
     "threshold": "0 failing, at least 1 check, evidence for this commit", "source": "--tests evidence JSON", "ident": "QA-070"},
    {"name": "Performance gates", "formula": "performance checks with a verdict other than PASS",
     "threshold": "0 failing, evidence for this commit", "source": "--perf evidence JSON", "ident": "PERF-051"},
    {"name": "V&V matrix completeness", "formula": "blank cells in validation/matrix.json; quantities without a benchmark not labelled not validated",
     "threshold": "0", "source": "validation/matrix.json", "ident": "VAL-051"},
    {"name": "Model coverage in the V&V matrix", "formula": "reported models and outputs without a row in validation/model_matrix.json",
     "threshold": "0", "source": "validation/model_matrix.json and the model registry", "ident": "VAL-001"},
    {"name": "V&V verdicts derived from case records", "formula": "matrix cells that differ from their derivation from the case records",
     "threshold": "0", "source": "V&V report and case records", "ident": "VAL-006"},
    {"name": "Open serious defects", "formula": "open S1 defects, plus open S2 defects without a valid waiver",
     "threshold": "0", "source": "docs/defects/faris-defects.json", "ident": "QA-083"},
    {"name": "Fuzzing", "formula": "fuzz targets without a clean libfuzzer run of at least 3600 s; crashes",
     "threshold": "0 of each", "source": "--fuzz evidence JSON (scripts/fuzz_all.sh)", "ident": "QA-033"},
    {"name": "Reproducible build", "formula": "release artifacts whose SHA-256 differs between two builds",
     "threshold": "0", "source": "--repro evidence JSON", "ident": "QA-063"},
    {"name": "Documentation tests", "formula": "handbook samples and tutorials with a verdict other than PASS",
     "threshold": "0 failing, evidence for this commit", "source": "--doc-tests evidence JSON (scripts/doc_samples.py)", "ident": "QA-016"},
    {"name": "Gate waivers", "formula": "waivers lacking owner, reason or expiry, expired, or expiring beyond 90 days",
     "threshold": "0", "source": "docs/release-waivers.json", "ident": "QA-072"},
    {"name": "Dependency licences", "formula": "dependencies whose licence is not on the AGPL-3.0-compatible allowlist",
     "threshold": "0", "source": "cargo metadata and scripts/licence_policy.json", "ident": "LEG-003"},
    {"name": "Security advisories", "formula": "RustSec advisories against Cargo.lock without a valid, unexpired waiver",
     "threshold": "0", "source": "scripts/check_audit.py and scripts/audit_allowlist.json", "ident": "SEC-031"},
)

LINE = re.compile(r"^(PASS|FAIL|WAIVED)\s+.*\(([^()]+)\):")


def problems() -> list[str]:
    """Rules of the card itself: the count, empty fields, unique rows and known gate conditions."""
    import release_gate

    known = {c[1] for c in release_gate.CONDITIONS} | {c[1] for c in release_gate.REPO_CHECKS}
    found = []
    if len(MEASURES) > MAX_MEASURES:
        found.append(f"{len(MEASURES)} measures; a release card has at most {MAX_MEASURES}")
    seen = set()
    for m in MEASURES:
        found += [f"{m.get('name', '?')}: empty {f}" for f in FIELDS if not str(m.get(f, "")).strip()]
        if "|" in "".join(str(m.get(f, "")) for f in FIELDS):
            found.append(f"{m.get('name')}: a field contains a pipe character")
        if m.get("ident") not in known:
            found.append(f"{m.get('name')}: {m.get('ident')} is not a condition of the release gate")
        if m.get("ident") in seen:
            found.append(f"{m.get('name')}: {m.get('ident')} is used by two measures")
        seen.add(m.get("ident"))
    return found


def verdicts(gate_text: str) -> dict[str, str]:
    out = {}
    for line in gate_text.splitlines():
        match = LINE.match(line)
        if match:
            out[match.group(2)] = match.group(1)
    return out


def render(gate_text: str | None = None) -> str:
    result = verdicts(gate_text) if gate_text is not None else {}
    lines = [
        "# Release quality scorecard", "",
        f"{len(MEASURES)} measures, each of which gates the release. Generated by scripts/release_scorecard.py"
        " from the release gate's conditions (scripts/release_gate.py); the verdict is the gate's own.", "",
        "| Measure | Formula | Threshold | Source | Verdict |", "| --- | --- | --- | --- | --- |",
    ]
    for m in MEASURES:
        lines.append(f"| {m['name']} | {m['formula']} | {m['threshold']} | {m['source']} | {result.get(m['ident'], 'not run')} |")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--gate", type=Path, help="the release gate's output (release_gate.py --archive FILE)")
    ap.add_argument("--out", type=Path)
    args = ap.parse_args(argv)
    found = problems()
    for line in found:
        print(f"release_scorecard: {line}", file=sys.stderr)
    if found:
        return 1
    text = render(args.gate.read_text(encoding="utf-8") if args.gate else None)
    if args.out:
        args.out.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
