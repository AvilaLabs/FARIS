#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Validate and descriptively summarize existing whole-workflow R3 artifacts.

No campaign-level uncertainty or verdict is inferred. Every point retains its
replicate, stage seeds, and activation grid identity; unknown method covariance
remains inconclusive as stated by the source comparison.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys

import r3_scenario as sc

GRIDS_CM = (100, 50, 25)
COOLING_LABELS = tuple(sc.COOLING_LABELS.values())


def _json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read valid JSON artifact {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"artifact must contain a JSON object: {path}")
    return value


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _safe_artifact(root: Path, rel: object, digest: object, label: str) -> Path:
    if not isinstance(rel, str) or not isinstance(digest, str) or len(digest) != 64:
        raise ValueError(f"malformed {label} path or SHA-256 in campaign state")
    path = (root / rel).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"{label} path escapes campaign directory")
    if not path.is_file() or _sha256(path) != digest:
        raise ValueError(f"missing or hash-mismatched {label}: {rel}")
    return path


def _expected(plan: dict) -> dict[tuple[str, int, int | None], dict]:
    if plan.get("schema") != "faris-r3-whole-workflow-campaign/v1":
        raise ValueError("unsupported or malformed campaign plan schema")
    reps = plan.get("replicates")
    count = plan.get("replicate_count")
    if not isinstance(count, int) or count < 2 or not isinstance(reps, list) or len(reps) != count:
        raise ValueError("campaign plan must declare at least two complete replicates")
    expected: dict[tuple[str, int, int | None], dict] = {}
    seen = set()
    for index, rep in enumerate(reps, 1):
        if not isinstance(rep, dict) or rep.get("replicate") != index:
            raise ValueError("replicate identities in plan must be ordered and contiguous from 1")
        dseed = rep.get("d1s_seed")
        grids = rep.get("r2s")
        if not isinstance(dseed, int) or not isinstance(grids, list) or [g.get("activation_cell_cm") for g in grids if isinstance(g, dict)] != list(GRIDS_CM):
            raise ValueError(f"malformed stage/grid plan for replicate {index}")
        seeds = [dseed]
        for grid in grids:
            for key in ("neutron", "photon", "cli_seed"):
                if not isinstance(grid.get(key), int):
                    raise ValueError(f"missing {key} seed for replicate {index}, grid {grid.get('activation_cell_cm')}")
            if grid["cli_seed"] != grid["neutron"] or grid["photon"] != grid["neutron"] + 1:
                raise ValueError("R2S stage seeds do not match the declared CLI/photon seed relationship")
            seeds.extend((grid["neutron"], grid["photon"]))
        if len(set(seeds)) != len(seeds) or seen.intersection(seeds):
            raise ValueError("plan contains colliding seeds across stages or replicates")
        seen.update(seeds)
        expected[("d1s", index, None)] = {"seed": dseed}
        for grid in grids:
            cell = grid["activation_cell_cm"]
            expected[("r2s", index, cell)] = {"neutron": grid["neutron"], "photon": grid["photon"]}
            expected[("compare", index, cell)] = {}
    return expected


def validate_campaign(campaign_dir: Path) -> tuple[dict, dict, dict]:
    root = campaign_dir.resolve()
    plan_path, state_path = root / "campaign-plan.json", root / "campaign-state.json"
    plan, state = _json(plan_path), _json(state_path)
    expected = _expected(plan)
    if state.get("schema") != "faris-r3-whole-workflow-campaign-state/v1":
        raise ValueError("unsupported or malformed campaign state schema")
    if state.get("campaign_plan_sha256") != _sha256(plan_path):
        raise ValueError("campaign state does not identify this frozen plan")
    tasks = state.get("tasks")
    if not isinstance(tasks, list):
        raise ValueError("campaign state tasks must be a list")
    seen = set()
    artifacts = {}
    for row in tasks:
        if not isinstance(row, dict):
            raise ValueError("malformed task row in campaign state")
        kind, rep, cell = row.get("kind"), row.get("replicate"), row.get("activation_cell_cm")
        key = (kind, rep, cell if kind != "d1s" else None)
        if key not in expected or key in seen:
            raise ValueError(f"unexpected or duplicate task identity in campaign state: {key}")
        seen.add(key)
        if row.get("status") != "COMPLETED" or row.get("returncode") != 0:
            continue
        if kind == "d1s":
            path = _safe_artifact(root, row.get("record_path"), row.get("record_sha256"), "D1S record")
            record = _json(path)
            want = expected[key]["seed"]
            if record.get("kind") != "r3-d1s" or record.get("seed") != want:
                raise ValueError(f"D1S record identity/seed mismatch for replicate {rep}")
            artifacts[key] = (path, record)
        elif kind == "r2s":
            path = _safe_artifact(root, row.get("record_path"), row.get("record_sha256"), "R2S record")
            record = _json(path)
            settings = record.get("settings", {})
            if record.get("kind") != "r3-r2s" or settings.get("activation_cell_cm") != cell or settings.get("seeds") != [expected[key]["neutron"], expected[key]["photon"]]:
                raise ValueError(f"R2S record identity/grid/seed mismatch for replicate {rep}, grid {cell}")
            artifacts[key] = (path, record)
        else:
            path = _safe_artifact(root, row.get("comparison_path"), row.get("comparison_sha256"), "comparison")
            comp = _json(path)
            if comp.get("schema") != "faris-r3/v1":
                raise ValueError(f"unsupported comparison artifact for replicate {rep}, grid {cell}")
            want_d1s = next(r["d1s_seed"] for r in plan["replicates"] if r["replicate"] == rep)
            want_grid = next(g for r in plan["replicates"] if r["replicate"] == rep
                             for g in r["r2s"] if g["activation_cell_cm"] == cell)
            runs = comp.get("runs", {})
            got_d1s = runs.get("d1s", {}).get("seed")
            r2s_run = runs.get("r2s", {})
            got_settings = r2s_run.get("settings", {})
            if got_d1s != want_d1s or got_settings.get("activation_cell_cm") != cell or \
                    got_settings.get("seeds") != [want_grid["neutron"], want_grid["photon"]]:
                raise ValueError(f"comparison artifact identity/grid/seed mismatch for replicate {rep}, grid {cell}")
            artifacts[key] = (path, comp)
    if state.get("status") == "ALL_COMMANDS_EXITED_ZERO" and seen != set(expected):
        raise ValueError("completed campaign state is missing planned replicate/stage/grid tasks")
    # Missing rows and noncompleted task rows are preserved as missing evidence.
    return plan, state, {"expected": expected, "seen": seen, "artifacts": artifacts}


def summarize(campaign_dir: Path) -> dict:
    plan, state, validated = validate_campaign(campaign_dir)
    expected, seen, artifacts = (validated[k] for k in ("expected", "seen", "artifacts"))
    root = campaign_dir.resolve()
    points = []
    for rep in range(1, plan["replicate_count"] + 1):
        dkey = ("d1s", rep, None)
        drec = artifacts.get(dkey, (None, None))[1]
        for cell in GRIDS_CM:
            rkey, ckey = ("r2s", rep, cell), ("compare", rep, cell)
            rrec = artifacts.get(rkey, (None, None))[1]
            comp = artifacts.get(ckey, (None, None))[1]
            seed_plan = next(r for r in plan["replicates"] if r["replicate"] == rep)
            grid_plan = next(g for g in seed_plan["r2s"] if g["activation_cell_cm"] == cell)
            complete = bool(drec and rrec and comp)
            for label in COOLING_LABELS:
                entry = (comp or {}).get("cooling_times", {}).get(label)
                if not complete or not isinstance(entry, dict):
                    points.append({"replicate": rep, "activation_cell_cm": cell,
                                   "d1s_seed": seed_plan["d1s_seed"], "r2s_neutron_seed": grid_plan["neutron"],
                                   "r2s_photon_seed": grid_plan["photon"], "cooling_time": label,
                                   "status": "MISSING_OR_FAILED", "missing_tasks": [kind for kind, key in
                                       (("d1s", dkey), ("r2s", rkey), ("compare", ckey)) if key not in artifacts]})
                    continue
                points.append({"replicate": rep, "activation_cell_cm": cell,
                               "d1s_seed": seed_plan["d1s_seed"], "r2s_neutron_seed": grid_plan["neutron"],
                               "r2s_photon_seed": grid_plan["photon"], "cooling_time": label,
                               "status": "AVAILABLE", "ratio_d1s_over_r2s": entry.get("ratio_d1s_over_r2s"),
                               "central_ratio_in_band": entry.get("central_ratio_in_band"),
                               "ratio_uncertainty": entry.get("ratio_uncertainty"),
                               "source_verdict": (comp or {}).get("verdict"),
                               "comparison_path": str(artifacts[ckey][0].relative_to(root))})
    groups = []
    for cell in GRIDS_CM:
        for label in COOLING_LABELS:
            selected = [p for p in points if p["activation_cell_cm"] == cell and p["cooling_time"] == label]
            values = [{"replicate": p["replicate"], "value": p["ratio_d1s_over_r2s"]} for p in selected
                      if p["status"] == "AVAILABLE" and isinstance(p["ratio_d1s_over_r2s"], (int, float))
                      and math.isfinite(p["ratio_d1s_over_r2s"])]
            numeric = [item["value"] for item in values]
            groups.append({"activation_cell_cm": cell, "cooling_time": label,
                           "available_replicates": len(numeric), "planned_replicates": plan["replicate_count"],
                           "ratio_values_by_replicate": values,
                           "descriptive_ratio_summary": ({"mean": statistics.mean(numeric),
                               "median": statistics.median(numeric), "min": min(numeric), "max": max(numeric),
                               "sample_sd": statistics.stdev(numeric) if len(numeric) > 1 else None}
                               if numeric else None),
                           "central_in_band_count": sum(p.get("central_ratio_in_band") is True for p in selected),
                           "central_out_of_band_count": sum(p.get("central_ratio_in_band") is False for p in selected),
                           "inconclusive_or_missing_count": sum(p["status"] != "AVAILABLE" or
                               p.get("central_ratio_in_band") is None or "unknown" in str(p.get("ratio_uncertainty", "")).lower()
                               for p in selected)})
    cross_grid = []
    for rep in range(1, plan["replicate_count"] + 1):
        for label in COOLING_LABELS:
            ordered = []
            by_grid = {}
            for cell in GRIDS_CM:
                point = next(p for p in points if p["replicate"] == rep and
                             p["activation_cell_cm"] == cell and p["cooling_time"] == label)
                value = point.get("ratio_d1s_over_r2s")
                finite = (point["status"] == "AVAILABLE" and isinstance(value, (int, float))
                          and math.isfinite(value))
                row = {"activation_cell_cm": cell, "status": point["status"],
                       "ratio_d1s_over_r2s": value if finite else None}
                ordered.append(row)
                by_grid[cell] = value if finite else None
            differences = []
            for coarse, fine in ((100, 50), (50, 25)):
                a, b = by_grid[fine], by_grid[coarse]
                differences.append({"from_grid_cm": coarse, "to_grid_cm": fine,
                                    "difference_fine_minus_coarse": a - b if a is not None and b is not None else None,
                                    "status": "AVAILABLE" if a is not None and b is not None else "MISSING_OR_FAILED"})
            cross_grid.append({"replicate": rep, "cooling_time": label,
                               "ordered_grid_values": ordered,
                               "adjacent_grid_differences": differences})
    return {"schema": "faris-r3-campaign-summary/v1", "campaign_status": state.get("status"),
            "scientific_verdict": "INCONCLUSIVE: summary adds no uncertainty model or acceptance threshold; source comparisons retain their verdicts",
            "plan_path": "campaign-plan.json", "replicate_count": plan["replicate_count"],
            "activation_grids_cm": list(GRIDS_CM), "points": points, "cross_replicate_by_grid": groups,
            "cross_grid_by_replicate": cross_grid,
            "task_inventory": [{"kind": k, "replicate": r, "activation_cell_cm": c,
                                "present_in_state": (k, r, c) in seen}
                               for k, r, c in expected]}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("campaign_dir", type=Path)
    ap.add_argument("--out", type=Path, help="write summary JSON here (default: stdout)")
    args = ap.parse_args(argv)
    result = summarize(args.campaign_dir)
    rendered = json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n"
    if args.out:
        args.out.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
