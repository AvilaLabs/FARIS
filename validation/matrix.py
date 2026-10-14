# SPDX-License-Identifier: AGPL-3.0-only
"""The V&V matrix and its blank-cell check (requirements VAL-050, VAL-051).

One row per (quantity, benchmark). A quantity with no registered benchmark gets one row whose
benchmark is "none" and whose status is "not validated", with the reason and the next step. The
matrix is generated from validation/quantities.json and the case manifests, never edited by hand:

    python3 -m validation matrix --out validation/matrix.json [--runs DIR --identity FILE]

`check_matrix` is what the release gate (scripts/release_gate.py) calls.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

from .manifest import load_manifest
from .scoring import DEFAULT_BOOTSTRAP_SEED, Identity, ValidationError

HERE = Path(__file__).resolve().parent
QUANTITIES = HERE / "quantities.json"
MATRIX = HERE / "matrix.json"
SCHEMA = "faris.validation-matrix/1.0.0"
REGISTRY_SCHEMA = "faris.validation-quantities/1.0.0"
NOT_VALIDATED = "not validated"
STATUSES = (NOT_VALIDATED, "code-to-code only", "validated within range", "validated but out of range")
# Every cell of a row. None of them may be blank.
CELLS = ("quantity", "benchmark_id", "evidence_class", "status", "reason", "next_step",
         "bias", "uncertainty", "qualified_range", "last_run", "not_covered")


def load_registry(path: Path = QUANTITIES) -> dict:
    registry = json.loads(path.read_text(encoding="utf-8"))
    if registry.get("schema") != REGISTRY_SCHEMA:
        raise ValidationError(f"{path.name}: schema must be {REGISTRY_SCHEMA!r}")
    return registry


def _range_text(r: dict) -> str:
    params = "; ".join(f"{p['name']} {p['min']:g} to {p['max']:g} {p['unit']}" for p in r.get("parameters", [])) or "no scanned parameters"
    return (f"materials: {', '.join(r['materials'])}. Geometry: {r['geometry_class']}. Spectrum: {r['spectrum_class']}. "
            f"Parameters: {params}. Cooling time: {r['cooling_time']}")


def range_sha256(qualified_range: dict) -> str:
    """Hash of a case's qualified range (VAL-070); a changed range changes the matrix row."""
    return hashlib.sha256(json.dumps(qualified_range, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")).hexdigest()


def _fmt(x: float) -> str:
    return f"{x:.4g}"


def build_matrix(registry: dict, cases: list[dict]) -> dict:
    """`cases` are the scored case results of a suite (validation.report.build_suite)."""
    by_id = {c["case_id"]: c for c in cases}
    rows = []
    for q in registry["quantities"]:
        if not q["benchmarks"]:
            rows.append({
                "quantity": q["id"], "benchmark_id": "none", "evidence_class": "none", "status": NOT_VALIDATED,
                "reason": "no benchmark is registered for this quantity",
                "next_step": "register a benchmark case in validation/cases and name it in validation/quantities.json",
                "bias": "not evaluated: no benchmark", "uncertainty": "not evaluated: no benchmark",
                "qualified_range": "none: no benchmark", "range_sha256": "none: no benchmark", "last_run": "never run: no benchmark",
                "not_covered": "everything: no benchmark covers this quantity"})
            continue
        for b in q["benchmarks"]:
            case = by_id.get(b["case_id"])
            if case is None or b["response_class"] not in case["classes"]:
                raise ValidationError(f"quantity {q['id']}: benchmark {b['case_id']}/{b['response_class']} is not a registered case response")
            cls = case["classes"][b["response_class"]]
            counts = cls["verdict_counts"]
            scored = counts.get("PASS", 0) + counts.get("FAIL", 0)
            if scored == 0:
                status, reason = NOT_VALIDATED, "the benchmark is registered but no current FARIS run has a PASS or FAIL verdict on it"
                next_step = cls.get("next_step") or "run the case and resolve the blocked or missing detectors"
            elif case["evidence_class"] == "code-to-code":
                status, reason = "code-to-code only", "agreement with another code is verification evidence, not validation (VAL-041)"
                next_step = "compare with an experiment"
            elif counts.get("FAIL", 0) == 0:
                status, reason = "validated within range", f"{scored} detectors scored, none FAIL"
                next_step = "extend the qualified range or add a benchmark outside it"
            else:
                status, reason = "validated but out of range", f"{counts['FAIL']} of {scored} scored detectors FAIL"
                next_step = "read the failing detectors in the validation report and find why"
            bias = cls.get("bias")
            if bias and bias["status"] == "estimated":
                bias_cell = f"calculated: mean log C/E {_fmt(bias['bias'])}, 95 % bootstrap interval [{_fmt(bias['ci95'][0])}, {_fmt(bias['ci95'][1])}]"
                unc_cell = f"calculated: bootstrap interval above; compatibility rule k = {case['k']:g}"
            else:
                why = (bias or {}).get("why") or cls.get("why") or "no bias estimate"
                bias_cell = f"not evaluated: {why}"
                unc_cell = f"not evaluated: {why}"
            run = case.get("run")
            if run is None:
                last_run = "never run: no FARIS run record for this case"
            else:
                last_run = run.get("run_date") or "run date not recorded in the run record"
            rows.append({
                "quantity": q["id"], "benchmark_id": f"{b['case_id']}/{b['response_class']}", "evidence_class": case["evidence_class"],
                "status": status, "reason": f"{reason}. Scope: {b['scope']}", "next_step": next_step,
                "bias": bias_cell, "uncertainty": unc_cell, "qualified_range": _range_text(case["qualified_range"]), "range_sha256": range_sha256(case["qualified_range"]),
                "last_run": last_run, "not_covered": "; ".join(case["not_covered"])})
    return {"schema": SCHEMA, "note": "Generated by `python3 -m validation matrix`; do not edit by hand. Bias and uncertainty are calculated from a run record or marked not evaluated with why.",
            "quantities": [q["id"] for q in registry["quantities"] if q["kind"] == "calculated"], "rows": rows}


def check_matrix(matrix: dict, registry: dict | None = None) -> list[str]:
    """VAL-051: the problems that block a release. Empty means the matrix has no blank row."""
    problems = []
    if not isinstance(matrix, dict) or matrix.get("schema") != SCHEMA:
        return [f"matrix schema must be {SCHEMA!r}"]
    rows = matrix.get("rows")
    if not isinstance(rows, list) or not rows:
        return ["matrix has no rows"]
    seen = set()
    for i, row in enumerate(rows):
        label = f"row {i + 1}" + (f" ({row.get('quantity')}, {row.get('benchmark_id')})" if isinstance(row, dict) else "")
        if not isinstance(row, dict):
            problems.append(f"{label}: not an object")
            continue
        for cell in CELLS:
            value = row.get(cell)
            if not isinstance(value, str) or not value.strip():
                problems.append(f"{label}: blank cell {cell}")
        if row.get("status") not in STATUSES:
            problems.append(f"{label}: status {row.get('status')!r} is not one of {', '.join(STATUSES)}")
        if row.get("benchmark_id") == "none" and row.get("status") != NOT_VALIDATED:
            problems.append(f"{label}: a quantity with no benchmark must be labelled {NOT_VALIDATED!r}")
        seen.add(row.get("quantity"))
    wanted = list(matrix.get("quantities") or [])
    if registry is not None:
        wanted = [q["id"] for q in registry["quantities"] if q["kind"] == "calculated"]
        if sorted(wanted) != sorted(matrix.get("quantities") or []):
            problems.append("matrix quantities differ from the quantity registry")
    for q in wanted:
        if q not in seen:
            problems.append(f"quantity {q} has no matrix row")
    return problems


def main(argv: list[str] | None = None) -> int:
    from . import report  # imported here: report imports this module too
    ap = argparse.ArgumentParser(prog="python3 -m validation matrix", description="write the V&V matrix")
    ap.add_argument("--out", type=Path, default=MATRIX)
    ap.add_argument("--cases", type=Path, default=report.CASES_DIR)
    ap.add_argument("--runs", type=Path)
    ap.add_argument("--identity", type=Path)
    ap.add_argument("--seed", type=int, default=DEFAULT_BOOTSTRAP_SEED)
    args = ap.parse_args(argv)
    manifests = [load_manifest(p) for p in report.discover(args.cases)]
    records = report.load_records(manifests, args.runs)
    current = Identity.from_dict(json.loads(args.identity.read_text(encoding="utf-8"))) if args.identity else None
    suite = report.build_suite(manifests, records, current, args.seed, load_registry())
    problems = check_matrix(suite["matrix"], load_registry())
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    args.out.write_text(json.dumps(suite["matrix"], indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {len(suite['matrix']['rows'])} rows")
    return 0
