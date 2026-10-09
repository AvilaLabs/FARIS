#!/usr/bin/env python3
"""RM-M model acceptance: the three checks that must pass before any risk test.

Run with an OpenMC interpreter (0.15.3, ~/.venvs/w003env) under the memory cap:

    PYTHONPATH=<dir with openmc_plasma_source and NeSST> accept_rm_m.py --model-dir DIR --out acceptance.json

Checks (docs/notes/CAD_TRANSPORT_RISK_TESTS.md, Model acceptance):
  1. per-solid faceted volume within 0.5 % of the CAD volume (GEO-030);
  2. lost particles <= 1e-6 per history over 1e6 histories (GEO-025);
  3. 100 % of sampled source sites inside the plasma volume (SRC-018).
The CAD volumes come from the build (rm_m_roles.json); the faceted volumes are
read from the h5m file itself.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h5m_volumes  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402


def check_volumes(model_dir: Path, h5m_name: str) -> dict:
    roles = RM.load_roles(model_dir)
    facets = h5m_volumes.facet_volumes(str(model_dir / h5m_name))
    rows = []
    for r in roles:
        rows.append({"name": r["name"], "cad_volume_cm3": r["cad_volume_cm3"],
                     "faceted_volume_cm3": facets[r["volume_id"]]["faceted_volume"], "volume_id": r["volume_id"],
                     "triangles": facets[r["volume_id"]]["triangles"]})
    result = checks.volume_comparison(rows)
    result["h5m"] = h5m_name
    result["h5m_sha256"] = RM.sha256(model_dir / h5m_name)
    result["total_triangles"] = sum(r["triangles"] for r in rows)
    return result


def sample_sites(openmc, model_dir: Path, run_dir: Path, constrained: bool, n: int, seed: int, threads: int, h5m_name: str) -> dict:
    """Sample source sites with the real OpenMC source machinery and locate each in the DAGMC model."""
    import numpy as np
    from matplotlib.path import Path as MplPath

    run_dir.mkdir(parents=True, exist_ok=True)
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, model_dir / h5m_name, roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, ids["plasma"] if constrained else None)
    model.settings.run_mode = "fixed source"
    model.settings.batches = 1
    model.settings.particles = 1000
    model.settings.source = [source]
    model.settings.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    model.export_to_model_xml(str(run_dir))
    cwd = os.getcwd()
    os.chdir(run_dir)
    import openmc.lib

    cell_of = {}
    outside_any = 0
    try:
        openmc.lib.init(["--threads", str(threads)])
        t0 = time.time()
        sites = openmc.lib.sample_external_source(n, prn_seed=seed)
        sample_seconds = time.time() - t0
        points = np.array([s.r for s in sites])
        t0 = time.time()
        for r in points:
            try:
                cell, _ = openmc.lib.find_cell(tuple(float(v) for v in r))
                cell_of[cell.id] = cell_of.get(cell.id, 0) + 1
            except Exception:
                outside_any += 1
        locate_seconds = time.time() - t0
    finally:
        openmc.lib.finalize()
        os.chdir(cwd)
    inside = cell_of.get(ids["plasma"], 0)
    # Independent check against the analytic plasma boundary polygon (no DAGMC involved).
    boundary = MplPath(np.array([spec.plasma_point(2.0 * math.pi * i / 2000) for i in range(2000)]))
    rz = np.column_stack([np.hypot(points[:, 0], points[:, 1]), points[:, 2]])
    analytic_inside = int(boundary.contains_points(rz).sum())
    result = checks.source_site_check(inside, n)
    result.update({
        "constrained_to_plasma_cell": constrained, "seed": seed,
        "sites_by_volume_id": {str(k): v for k, v in sorted(cell_of.items())},
        "sites_not_in_any_cell": outside_any,
        "analytic_polygon_inside": analytic_inside,
        "analytic_fraction_inside": analytic_inside / n,
        "unscaled_source_rate_n_s": unscaled,
        "sample_seconds": sample_seconds, "locate_seconds": locate_seconds,
    })
    return result


def check_lost_particles(openmc, model_dir: Path, run_dir: Path, histories: int, batches: int, threads: int, seed: int, h5m_name: str, reuse: bool = False) -> dict:
    if reuse:
        # evaluate a finished run's output without repeating it (used when only the post-processing was fixed)
        result = {"run_dir": str(run_dir), "openmc_version": openmc.__version__, "histories": histories, "batches": batches,
                  "seed": seed, "threads": threads, "reused_existing_run": True, "run": {"returncode": 0},
                  "lost": RM.count_lost_particles(run_dir)}
        result["tf_fast_flux"] = RM.read_tf_flux(openmc, run_dir, batches)
    else:
        if run_dir.exists():
            for p in run_dir.glob("*"):
                p.unlink()
        result = RM.run_analog(openmc, model_dir, run_dir, histories, batches, seed, threads, h5m_name=h5m_name)
    if result["run"]["returncode"] != 0:
        return {"pass": False, "error": f"openmc exited {result['run']['returncode']}", "run": result}
    lost = result["lost"]["restart_files"]
    rule = checks.lost_particle_check(lost, histories)
    rule["lost_log_lines"] = result["lost"]["log_lines_mentioning_lost"]
    rule["run"] = result
    return rule


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--h5m", default="rm_m.h5m")
    ap.add_argument("--out", required=True, type=Path, help="acceptance result JSON")
    ap.add_argument("--run-dir", required=True, type=Path, help="scratch directory for the OpenMC runs")
    ap.add_argument("--histories", type=int, default=1_000_000)
    ap.add_argument("--batches", type=int, default=100)
    ap.add_argument("--sites", type=int, default=200_000)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20261009)
    ap.add_argument("--reuse-lost-run", action="store_true", help="evaluate the existing lost-particle run directory instead of running it again")
    ap.add_argument("--skip-lost-particle-run", action="store_true")
    args = ap.parse_args()
    import openmc

    t0 = time.time()
    out = {"schema": "faris-rm-m-acceptance/v1", "openmc_version": openmc.__version__,
           "library": RM.library_identity(), "model_dir": str(args.model_dir)}
    out["volumes"] = check_volumes(args.model_dir, args.h5m)
    print("volumes:", out["volumes"]["pass"], out["volumes"]["worst"], flush=True)
    out["source_sites_unconstrained"] = sample_sites(openmc, args.model_dir, args.run_dir / "sites_free", False, args.sites, args.seed, args.threads, args.h5m)
    print("sites (free):", out["source_sites_unconstrained"]["fraction_inside"], flush=True)
    out["source_sites"] = sample_sites(openmc, args.model_dir, args.run_dir / "sites", True, args.sites, args.seed, args.threads, args.h5m)
    print("sites (plasma-cell constraint):", out["source_sites"]["fraction_inside"], flush=True)
    if not args.skip_lost_particle_run:
        out["lost_particles"] = check_lost_particles(openmc, args.model_dir, args.run_dir / "lost", args.histories, args.batches, args.threads, args.seed, args.h5m, args.reuse_lost_run)
        print("lost:", out["lost_particles"].get("lost"), "of", out["lost_particles"].get("histories"), flush=True)
    required = ["volumes", "source_sites"] + ([] if args.skip_lost_particle_run else ["lost_particles"])
    out["accepted"] = all(out[k]["pass"] for k in required)
    out["wall_seconds"] = time.time() - t0
    out["peak_rss_mb_self"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    out["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print("accepted:", out["accepted"])
    return 0 if out["accepted"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
