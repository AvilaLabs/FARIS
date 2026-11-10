#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Serial, predeclared R3 whole-workflow replicates across activation grids.

This is an evidence collector. It does not calculate a new convergence rule or
scientific verdict. Each replicate contains one D1S run and bounded R2S runs at
100, 50, and 25 cm, followed by the existing pairwise comparison reports.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from datetime import datetime, timezone

import r3_scenario as sc

ROOT = Path(__file__).resolve().parents[3]
RISK_DIR = Path(__file__).resolve().parent
GRIDS_CM = (100, 50, 25)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def seed_map(replicates: int, seed_base: int) -> list[dict]:
    """Return disjoint D1S, R2S-neutron, and R2S-photon seeds per replicate/grid."""
    if replicates < 2:
        raise ValueError("at least two whole-workflow replicates are required")
    if seed_base < 1:
        raise ValueError("seed base must be a positive integer")
    entries = []
    used: set[int] = set()
    for replicate in range(replicates):
        offset = seed_base + replicate * 10
        d1s = offset
        if d1s in used:
            raise ValueError("seed mapping collides")
        used.add(d1s)
        r2s = []
        for grid_index, cell_cm in enumerate(GRIDS_CM):
            neutron = offset + 1 + grid_index * 2
            photon = neutron + 1  # r3_r2s.py uses seed + 1 for photon transport.
            if neutron in used or photon in used:
                raise ValueError("seed mapping collides")
            used.update((neutron, photon))
            r2s.append({"activation_cell_cm": cell_cm, "neutron": neutron, "photon": photon,
                        "cli_seed": neutron})
        entries.append({"replicate": replicate + 1, "d1s_seed": d1s, "r2s": r2s})
    return entries


def make_plan(*, model_dir: Path, cross_sections: Path, python: Path, out_dir: Path,
              replicates: int, seed_base: int, purpose: str) -> dict:
    model_dir = model_dir.resolve()
    cross_sections = cross_sections.resolve()
    python = python.resolve()
    out_dir = out_dir.resolve()
    for label, path in (("Python executable", python), ("cross-section index", cross_sections)):
        if not path.is_file():
            raise FileNotFoundError(f"{label} is missing: {path}")
    dependencies = sc.rm_m_geometry_dependencies(model_dir)
    scripts = {name: RISK_DIR / name for name in
               ("r3_campaign.py", "r3_d1s.py", "r3_r2s.py", "r3_compare.py",
                "r3_scenario.py", "r3_dose_statistics.py", "r3_shutdown_dose_export.py",
                "r3_bounded_microxs.py", "r3_activation_coverage.py", "rm_m_openmc.py")}
    script_hashes = {name: sha256(path) for name, path in scripts.items()}
    return {
        "schema": "faris-r3-whole-workflow-campaign/v1",
        "status": "PREDECLARED",
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "purpose": purpose,
        "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR",
        "protocol": sc.FULL_EMISSION_PROTOCOL,
        "convergence_dimensions": {
            "activation_mesh_cm": list(GRIDS_CM),
            "response_mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM),
                              "upper_right_cm": list(sc.MESH_UPPER_CM),
                              "dimension": list(sc.MESH_DIMENSION)},
            "note": "Activation-grid convergence evidence is separate from source-response mesh convergence; this runner does not evaluate either.",
        },
        "replicate_count": replicates,
        "seed_policy": "Within each replicate D1S and every R2S neutron and photon stage use unique seeds; replicate seed blocks are disjoint. The R2S CLI seed is neutron seed; photon seed is CLI seed + 1.",
        "replicates": seed_map(replicates, seed_base),
        "standard_settings": {
            "d1s": {"particles_per_batch": 5000, "max_batches": 100000, "budget_s": 7000.0},
            "r2s": {"neutron_particles_per_batch": 20000, "neutron_batches": 50,
                    "photon_particles_per_batch": 100000, "photon_max_batches": 5000,
                    "photon_budget_s_per_time": 3000.0, "activation_backend": "bounded",
                    "activation_groups": sc.ACTIVATION_GROUPS, "photon_indices": list(sc.PHOTON_TIME_INDICES)},
            "threads": 4,
        },
        "inputs": {"model_dir": str(model_dir), "model_dependencies": dependencies,
                   "cross_sections_xml": {"path": str(cross_sections), "sha256": sha256(cross_sections)},
                   "python": str(python), "implementation_sha256": script_hashes},
        "output_dir": str(out_dir),
        "comparison_note": "r3_compare.py is run unchanged for each replicate/grid pair. It reports unknown D1S/R2S covariance as inconclusive; no campaign-level uncertainty or pass gate is added.",
    }


def _write_json(path: Path, value: dict) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(path)


def run_campaign(plan: dict, *, runner=subprocess.run) -> dict:
    out = Path(plan["output_dir"])
    if out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {out}")
    if sc.rm_m_geometry_dependencies(Path(plan["inputs"]["model_dir"])) != plan["inputs"]["model_dependencies"]:
        raise ValueError("RM-M model dependencies changed after campaign predeclaration")
    xs = Path(plan["inputs"]["cross_sections_xml"]["path"])
    if not xs.is_file() or sha256(xs) != plan["inputs"]["cross_sections_xml"]["sha256"]:
        raise ValueError("cross-section index changed after campaign predeclaration")
    for name, digest in plan["inputs"]["implementation_sha256"].items():
        path = RISK_DIR / name
        if not path.is_file() or sha256(path) != digest:
            raise ValueError(f"campaign implementation changed after predeclaration: {path}")
    out.mkdir(parents=True)
    plan_path = out / "campaign-plan.json"
    with plan_path.open("x", encoding="utf-8") as stream:
        json.dump(plan, stream, indent=2, sort_keys=True)
        stream.write("\n")
    python = Path(plan["inputs"]["python"])
    env = os.environ.copy()
    env["PYTHONPATH"] = str(RISK_DIR) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    env["FARIS_CROSS_SECTIONS"] = plan["inputs"]["cross_sections_xml"]["path"]
    env["OMP_NUM_THREADS"] = "4"
    tasks = []
    d1s_script = str(RISK_DIR / "r3_d1s.py")
    r2s_script = str(RISK_DIR / "r3_r2s.py")
    compare_script = str(RISK_DIR / "r3_compare.py")
    model = plan["inputs"]["model_dir"]
    for rep in plan["replicates"]:
        rep_name = f"rep-{rep['replicate']:02d}"
        d1s_dir = out / rep_name / "d1s"
        tasks.append({"kind": "d1s", "replicate": rep["replicate"], "out_dir": d1s_dir,
                      "command": [str(python), d1s_script, "--model-dir", model, "--out-dir", str(d1s_dir),
                                  "--threads", "4", "--seed", str(rep["d1s_seed"])]})
        for grid in rep["r2s"]:
            grid_dir = out / rep_name / f"r2s-{grid['activation_cell_cm']}cm"
            tasks.append({"kind": "r2s", "replicate": rep["replicate"], "activation_cell_cm": grid["activation_cell_cm"],
                          "out_dir": grid_dir,
                          "command": [str(python), r2s_script, "--model-dir", model, "--out-dir", str(grid_dir),
                                      "--activation-cell-cm", str(grid["activation_cell_cm"]),
                                      "--activation-backend", "bounded", "--threads", "4", "--seed", str(grid["cli_seed"])]})
            comparison = out / rep_name / f"comparison-{grid['activation_cell_cm']}cm.json"
            tasks.append({"kind": "compare", "replicate": rep["replicate"], "activation_cell_cm": grid["activation_cell_cm"],
                          "out_file": comparison,
                          "command": [str(python), compare_script, "--r2s", str(grid_dir), "--d1s", str(d1s_dir),
                                      "--out", str(comparison)]})
    state = {"schema": "faris-r3-whole-workflow-campaign-state/v1", "status": "RUNNING",
             "campaign_plan_sha256": sha256(plan_path),
             "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR", "tasks": []}
    state_path = out / "campaign-state.json"
    _write_json(state_path, state)
    logs = out / "logs"
    logs.mkdir()
    for index, task in enumerate(tasks, 1):
        if not xs.is_file() or sha256(xs) != plan["inputs"]["cross_sections_xml"]["sha256"]:
            state["status"] = "STOPPED_ON_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = "cross-section index changed after campaign predeclaration"
            _write_json(state_path, state)
            return state
        for name, digest in plan["inputs"]["implementation_sha256"].items():
            path = RISK_DIR / name
            if not path.is_file() or sha256(path) != digest:
                state["status"] = "STOPPED_ON_IMPLEMENTATION_CHANGE"
                state["failed_task_index"] = index
                state["error"] = f"campaign implementation changed after predeclaration: {path}"
                _write_json(state_path, state)
                return state
        log = logs / f"{index:03d}-{task['kind']}-rep{task['replicate']:02d}.log"
        with log.open("wb") as stream:
            completed = runner(task["command"], cwd=str(ROOT), env=env, stdout=stream,
                               stderr=subprocess.STDOUT, check=False)
        row = {key: value for key, value in task.items() if key not in ("command", "out_dir", "out_file")}
        row.update({"command": task["command"], "log": str(log.relative_to(out)), "returncode": completed.returncode})
        if task["kind"] in ("d1s", "r2s"):
            record = task["out_dir"] / ("d1s_record.json" if task["kind"] == "d1s" else "r2s_record.json")
            row["record_path"] = str(record.relative_to(out))
            row["record_sha256"] = sha256(record) if record.is_file() else None
        else:
            row["comparison_path"] = str(task["out_file"].relative_to(out))
            row["comparison_sha256"] = sha256(task["out_file"]) if task["out_file"].is_file() else None
        state["tasks"].append(row)
        if completed.returncode != 0:
            state["status"] = "STOPPED_ON_COMMAND_FAILURE"
            state["failed_task_index"] = index
            _write_json(state_path, state)
            return state
        if (task["kind"] in ("d1s", "r2s") and row["record_sha256"] is None) or \
                (task["kind"] == "compare" and row["comparison_sha256"] is None):
            state["status"] = "STOPPED_ON_MISSING_OUTPUT"
            state["failed_task_index"] = index
            _write_json(state_path, state)
            return state
        _write_json(state_path, state)
    state["status"] = "ALL_COMMANDS_EXITED_ZERO"
    _write_json(state_path, state)
    return state


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--cross-sections", required=True, type=Path)
    ap.add_argument("--python", required=True, type=Path, help="pinned OpenMC 0.15.3 Python executable")
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--replicates", required=True, type=int)
    ap.add_argument("--seed-base", required=True, type=int)
    ap.add_argument("--purpose", required=True, help="for example conditional-method-control")
    args = ap.parse_args(argv)
    if args.out_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {args.out_dir}")
    plan = make_plan(model_dir=args.model_dir, cross_sections=args.cross_sections, python=args.python,
                     out_dir=args.out_dir, replicates=args.replicates, seed_base=args.seed_base,
                     purpose=args.purpose)
    result = run_campaign(plan)
    print(json.dumps({"status": result["status"], "scientific_verdict": result["scientific_verdict"],
                      "task_count": len(result["tasks"]), "output_dir": str(args.out_dir.resolve())}, indent=2))
    return 0 if result["status"] == "ALL_COMMANDS_EXITED_ZERO" else 1


if __name__ == "__main__":
    raise SystemExit(main())
