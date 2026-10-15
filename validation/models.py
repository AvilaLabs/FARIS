# SPDX-License-Identifier: AGPL-3.0-only
"""The model matrix: verification and validation evidence for every model FARIS reports (VAL-001, VAL-006, VAL-037, VAL-053).

validation/matrix.json has one row per (quantity, benchmark). This module adds the other side: one row per
model or response FARIS reports, with its verification evidence (tests and controls, each with one evidence
class), its validation evidence (the matrix rows of the quantities it reports) and what is not covered.
It is generated, never edited by hand:

    python3 -m validation models [--out validation/model_matrix.json]   # write it, after the lint passes
    python3 -m validation models --check                               # lint and staleness, write nothing

The lint (`lint_models`) is what makes the matrix complete. It reads the Rust output catalogues
(`pub const NAME: &[Output]`) that classify every number FARIS reports, so a calculated output with no model
row, a catalogue no model claims, a quantity missing from the registry, or a test named as evidence that does
not exist is a finding. Staleness (`staleness`) compares the committed files with a regeneration from the tree:
a changed source file, case manifest, quantity registry or rule makes the affected rows STALE until the
matrices are regenerated, and the release gate refuses stale matrices.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

from . import matrix as matrix_mod
from .manifest import load_manifest, manifest_sha256
from .scoring import EVIDENCE_CLASSES

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
MODELS = HERE / "models.json"
MODEL_MATRIX = HERE / "model_matrix.json"
REGISTRY_SCHEMA = "faris.validation-models/1.0.0"
SCHEMA = "faris.validation-model-matrix/1.0.0"
KINDS = ("test", "control")
# Cells of a model row that may not be blank.
CELLS = ("model", "label", "page", "verification_status", "status", "reason", "next_step", "not_covered")

CATALOGUE = re.compile(r"^pub const (\w+): &\[Output\] = &\[(.*?)^\];", re.M | re.S)
ENTRY = re.compile(r'\b(calculated|not_calculated|by_score)\(\s*"([A-Za-z0-9_]+)"\s*(?:,\s*(Quantity::\w+|"[^"]*"))?')


def _snake(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_catalogues(root: Path) -> dict[str, dict]:
    """Every output catalogue in the Rust sources: {name: {file, outputs: [{name, kind, quantity or why}]}}."""
    found: dict[str, dict] = {}
    for path in sorted((root / "crates").glob("*/src/**/*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        for m in CATALOGUE.finditer(text):
            outputs = []
            for kind, name, arg in ENTRY.findall(m.group(2)):
                entry = {"name": name, "kind": kind}
                if kind == "calculated":
                    entry["quantity"] = _snake((arg or "Quantity::").split("::")[1])
                elif kind == "not_calculated":
                    entry["why"] = (arg or "").strip('"')
                outputs.append(entry)
            found[m.group(1)] = {"file": str(path.relative_to(root)), "outputs": outputs}
    return found


def load_models(root: Path = ROOT) -> dict:
    data = json.loads((root / "validation" / "models.json").read_text(encoding="utf-8"))
    if data.get("schema") != REGISTRY_SCHEMA:
        raise ValueError(f"models.json: schema must be {REGISTRY_SCHEMA!r}")
    return data


def _has_name(path: Path, name: str) -> bool:
    text = path.read_text(encoding="utf-8", errors="replace")
    return re.search(rf"\b(?:fn|def)\s+{re.escape(name)}\b", text) is not None


def lint_models(root: Path = ROOT, models: dict | None = None, registry: dict | None = None, catalogues: dict | None = None) -> list[str]:
    """Problems that leave a reported model or quantity without a row, or a row without real evidence."""
    models = models or load_models(root)
    registry = registry or matrix_mod.load_registry(root / "validation" / "quantities.json")
    catalogues = read_catalogues(root) if catalogues is None else catalogues
    quantity_ids = {q["id"] for q in registry["quantities"]}
    problems = []
    claimed: dict[str, str] = {}
    ids = [m.get("id") for m in models["models"]]
    for dup in sorted({i for i in ids if ids.count(i) > 1}):
        problems.append(f"model id {dup} appears twice")
    for m in models["models"]:
        mid = m.get("id", "?")
        for key in ("id", "label", "page", "sources", "verification", "not_covered"):
            if not m.get(key):
                problems.append(f"model {mid}: missing {key}")
        if not m.get("catalogues") and not str(m.get("reports", "")).strip():
            problems.append(f"model {mid}: names no output catalogue and does not say what it reports instead (reports)")
        for name in m.get("catalogues", []):
            if name not in catalogues:
                problems.append(f"model {mid}: output catalogue {name} is not in the Rust sources")
            elif name in claimed:
                problems.append(f"output catalogue {name} is claimed by both {claimed[name]} and {mid}")
            claimed[name] = mid
        has_by_score = any(o["kind"] == "by_score" for n in m.get("catalogues", []) for o in catalogues.get(n, {}).get("outputs", []))
        if has_by_score and not m.get("by_score_quantities"):
            problems.append(f"model {mid}: a catalogue has outputs whose quantity depends on the score; name them in by_score_quantities")
        if m.get("by_score_quantities") and not has_by_score:
            problems.append(f"model {mid}: by_score_quantities is set but no catalogue output depends on the score")
        for q in list(m.get("quantities", [])) + list(m.get("by_score_quantities", [])):
            if q not in quantity_ids:
                problems.append(f"model {mid}: quantity {q} is not in the quantity registry")
        for rel in list(m.get("sources", [])) + ([m["page"]] if m.get("page") else []):
            if not (root / rel).is_file():
                problems.append(f"model {mid}: {rel} does not exist")
        for i, ev in enumerate(m.get("verification", [])):
            where = f"model {mid} evidence {i + 1}"
            if ev.get("class") not in EVIDENCE_CLASSES:
                problems.append(f"{where}: class {ev.get('class')!r} is not exactly one of {', '.join(EVIDENCE_CLASSES)}")
            if ev.get("kind") not in KINDS:
                problems.append(f"{where}: kind must be one of {', '.join(KINDS)}")
            if not str(ev.get("what", "")).strip():
                problems.append(f"{where}: says nothing about what it checks")
            path = root / str(ev.get("file", ""))
            if not path.is_file():
                problems.append(f"{where}: {ev.get('file')} does not exist")
            elif ev.get("name") and not _has_name(path, ev["name"]):
                problems.append(f"{where}: {ev['name']} is not defined in {ev['file']}")
    for name, cat in sorted(catalogues.items()):
        if name not in claimed:
            problems.append(f"output catalogue {name} ({cat['file']}) reports numbers but no model row claims it")
        for out in cat["outputs"]:
            if out["kind"] == "calculated" and out["quantity"] not in quantity_ids:
                problems.append(f"{name}: output {out['name']} is a calculated {out['quantity']} that is not in the quantity registry")
    # Every quantity the Rust code can report has a registry row (the enum is the registry).
    enum_text = ""
    validation_rs = root / "crates" / "faris-model" / "src" / "validation.rs"
    if validation_rs.is_file():
        enum_text = validation_rs.read_text(encoding="utf-8")
        m = re.search(r"pub enum Quantity \{(.*?)\n\}", enum_text, re.S)
        if m:
            for variant in re.findall(r"^\s*([A-Z]\w*),", m.group(1), re.M):
                if _snake(variant) not in quantity_ids:
                    problems.append(f"the Rust Quantity {variant} has no row in the quantity registry")
    return problems


def _validation_of(quantities: list[str], matrix: dict) -> list[dict]:
    out = []
    for q in quantities:
        rows = [r for r in matrix["rows"] if r["quantity"] == q]
        out.append({"quantity": q, "status": _strongest_claim(r["status"] for r in rows) if rows else matrix_mod.NOT_VALIDATED,
                    "benchmarks": [r["benchmark_id"] for r in rows if r["benchmark_id"] != "none"]})
    return out


def _strongest_claim(statuses) -> str:
    """The state shown for a quantity: a failing benchmark first, then the strongest evidence (as in the Rust code)."""
    statuses = list(statuses)
    for s in ("validated but out of range", "validated within range", "code-to-code only"):
        if s in statuses:
            return s
    return matrix_mod.NOT_VALIDATED


def _weakest_claim(statuses) -> str:
    """The state of a model: a failing benchmark first, then the weakest evidence among its quantities."""
    statuses = set(statuses)
    for s in ("validated but out of range", matrix_mod.NOT_VALIDATED, "code-to-code only"):
        if s in statuses:
            return s
    return "validated within range"


def build_model_matrix(root: Path, matrix: dict, models: dict | None = None, catalogues: dict | None = None) -> dict:
    models = models or load_models(root)
    catalogues = read_catalogues(root) if catalogues is None else catalogues
    rows = []
    for m in models["models"]:
        outputs, quantities = [], list(m.get("quantities", []))
        for name in m.get("catalogues", []):
            cat = catalogues.get(name)
            for out in (cat["outputs"] if cat else []):
                outputs.append({"catalogue": name, **out})
                if out["kind"] == "calculated" and out["quantity"] not in quantities:
                    quantities.append(out["quantity"])
        for q in m.get("by_score_quantities", []):
            if q not in quantities:
                quantities.append(q)
        quantities.sort()
        validation = _validation_of(quantities, matrix)
        verification = [dict(ev) for ev in m["verification"]]
        classes = sorted({ev["class"] for ev in verification})
        states = {v["status"] for v in validation}
        if not quantities:
            status = matrix_mod.NOT_VALIDATED
            reason = f"this model reports {m.get('reports') or 'no quantity of the validation registry'}; no benchmark is registered for its response"
            next_step = "register a benchmark for this response in validation/cases and add a quantity for it to validation/quantities.json"
        else:
            status = _weakest_claim(states)
            if status == matrix_mod.NOT_VALIDATED:
                unv = [v["quantity"] for v in validation if v["status"] == matrix_mod.NOT_VALIDATED]
                reason = "the model reports quantities with no scored benchmark: " + ", ".join(unv)
            else:
                reason = "the state of the weakest quantity this model reports: " + ", ".join(f"{v['quantity']} {v['status']}" for v in validation)
            next_step = "score the registered benchmarks of these quantities (see the matrix rows) or register one"
        rows.append({
            "model": m["id"], "label": m["label"], "page": m["page"],
            "reported_outputs": outputs, "quantities": quantities,
            "verification": verification, "evidence_classes": classes,
            "verification_status": f"{len(verification)} test or control items, each with one evidence class ({', '.join(classes) or 'none'})",
            "validation": validation, "status": status, "reason": reason, "next_step": next_step,
            "not_covered": "; ".join(m["not_covered"]),
            "source_sha256": {rel: sha256_file(root / rel) for rel in m["sources"]},
        })
    return {"schema": SCHEMA,
            "note": "Generated by `python3 -m validation models`; do not edit by hand. Verification evidence is named in validation/models.json and checked to exist; validation status is derived from validation/matrix.json.",
            "inputs": input_hashes(root), "rows": rows}


def input_hashes(root: Path) -> dict:
    """Hashes of what the matrices were generated from; a change to any of them makes the matrices stale."""
    v = root / "validation"
    out = {"quantities.json": sha256_file(v / "quantities.json"), "models.json": sha256_file(v / "models.json"),
           "cases": {}}
    if (v / "rule_hashes.json").is_file():
        out["rule_hashes.json"] = sha256_file(v / "rule_hashes.json")
    for path in sorted((v / "cases").glob("*/manifest.json")):
        m = load_manifest(path)
        out["cases"][m["case_id"]] = manifest_sha256(m)
    return out


def check_model_matrix(mm: dict) -> list[str]:
    """Structure of a model matrix, as the release gate reads it from a report: no blank cell, one class per item."""
    if not isinstance(mm, dict) or mm.get("schema") != SCHEMA:
        return [f"model matrix schema must be {SCHEMA!r}"]
    rows = mm.get("rows")
    if not isinstance(rows, list) or not rows:
        return ["model matrix has no rows"]
    problems = []
    for i, row in enumerate(rows):
        label = f"model row {i + 1} ({row.get('model') if isinstance(row, dict) else '?'})"
        if not isinstance(row, dict):
            problems.append(f"{label}: not an object")
            continue
        for cell in CELLS:
            if not isinstance(row.get(cell), str) or not row[cell].strip():
                problems.append(f"{label}: blank cell {cell}")
        if row.get("status") not in matrix_mod.STATUSES:
            problems.append(f"{label}: status {row.get('status')!r} is not one of {', '.join(matrix_mod.STATUSES)}")
        for j, ev in enumerate(row.get("verification") or []):
            if ev.get("class") not in EVIDENCE_CLASSES:
                problems.append(f"{label}: evidence {j + 1} has no single valid evidence class")
    return problems


def staleness(root: Path, committed_models: dict, committed_matrix: dict) -> list[str]:
    """STALE findings: what changed since the committed matrices were generated (VAL-037)."""
    problems = []
    registry = matrix_mod.load_registry(root / "validation" / "quantities.json")
    from . import report  # imported here: report imports this module
    manifests = [load_manifest(p) for p in report.discover(root / "validation" / "cases")]
    suite = report.build_suite(manifests, {}, None, registry=registry)
    fresh_matrix = suite["matrix"]
    if fresh_matrix != committed_matrix:
        old = {(r["quantity"], r["benchmark_id"]): r for r in committed_matrix.get("rows", [])}
        new = {(r["quantity"], r["benchmark_id"]): r for r in fresh_matrix["rows"]}
        for key in sorted(set(old) | set(new)):
            if key not in old:
                problems.append(f"STALE matrix.json: row {key[0]}/{key[1]} is new in the tree")
            elif key not in new:
                problems.append(f"STALE matrix.json: row {key[0]}/{key[1]} no longer exists in the tree")
            elif old[key] != new[key]:
                cells = [c for c in new[key] if old[key].get(c) != new[key][c]]
                problems.append(f"STALE matrix.json: row {key[0]}/{key[1]} differs from the tree in {', '.join(cells)}")
        if not problems:
            problems.append("STALE matrix.json: differs from a regeneration from the tree")
    fresh = build_model_matrix(root, fresh_matrix)
    if fresh["inputs"] != committed_models.get("inputs"):
        old, new = committed_models.get("inputs") or {}, fresh["inputs"]
        for k in sorted(set(old) | set(new)):
            if old.get(k) != new.get(k):
                if k == "cases":
                    for c in sorted(set(old.get(k) or {}) | set(new.get(k) or {})):
                        if (old.get(k) or {}).get(c) != new[k].get(c):
                            problems.append(f"STALE model_matrix.json: case {c} changed since the matrices were generated")
                else:
                    problems.append(f"STALE model_matrix.json: {k} changed since the matrices were generated")
    old_rows = {r["model"]: r for r in committed_models.get("rows", [])}
    for row in fresh["rows"]:
        old_row = old_rows.get(row["model"])
        if old_row is None:
            problems.append(f"STALE model_matrix.json: model {row['model']} has no committed row")
        elif old_row.get("source_sha256") != row["source_sha256"]:
            changed = [p for p in row["source_sha256"] if (old_row.get("source_sha256") or {}).get(p) != row["source_sha256"][p]]
            problems.append(f"STALE model_matrix.json: model {row['model']}: source changed ({', '.join(changed)})")
        elif old_row != row:
            problems.append(f"STALE model_matrix.json: model {row['model']} differs from a regeneration")
    for mid in sorted(set(old_rows) - {r["model"] for r in fresh["rows"]}):
        problems.append(f"STALE model_matrix.json: committed model {mid} is no longer in models.json")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python3 -m validation models", description="write the model matrix")
    ap.add_argument("--out", type=Path, default=MODEL_MATRIX)
    ap.add_argument("--check", action="store_true", help="lint and check staleness; write nothing")
    args = ap.parse_args(argv)
    problems = lint_models(ROOT)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    matrix = json.loads(matrix_mod.MATRIX.read_text(encoding="utf-8"))
    if args.check:
        stale = staleness(ROOT, json.loads(args.out.read_text(encoding="utf-8")), matrix)
        print("\n".join(stale) or "the model matrix and the matrix are current")
        return 1 if stale else 0
    mm = build_model_matrix(ROOT, matrix)
    problems = check_model_matrix(mm)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    args.out.write_text(json.dumps(mm, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {len(mm['rows'])} models")
    return 0
