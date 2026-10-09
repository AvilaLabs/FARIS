#!/usr/bin/env python3
"""One R6 step on one OpenMC version. Run by r6_version.py under the memory cap.

    r6_worker.py generate --model-dir D --run-dir R --out step.json     (MGXS, random-ray FW-CADIS, window file, short windowed run)
    r6_worker.py analog   --model-dir D --run-dir R --out step.json     (one analog run, TF fast flux)
    r6_worker.py compare-windows A.h5 B.h5 --out cmp.json               (needs numpy and h5py only)

generate is the R1 generation step for response (a), the neutron flux above
0.1 MeV integrated over all TF coil cells: continuous-energy MGXS generation
(stochastic slab, P0 correction, as fwcadis_spike.py), a random-ray forward and
adjoint solve on the DAGMC model with the adjoint source on response (a), and
the weight-window file. Each sub-step is recorded with its outcome and the
workarounds it needed. Parameters are fixed here and are not tuned for R6.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import resource
import sys
import time
import traceback

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402

# Workarounds OpenMC 0.15.3 needs on a DAGMC model (feasibility run, 2026-10-09). Recorded with every step that uses them.
WORKAROUNDS_0153 = [
    "source_region_meshes takes openmc.Universe(universe_id=dagmc_universe.id), not the DAGMCUniverse",
    "every DAGMC volume has a real material: void is tagged 'filler' (H-1, 0.001 g/cm3)",
    "random-ray source is a discrete-energy IndependentSource constrained to the plasma cell",
    "bounding surface has an explicit surface_id",
]

# Eight energy groups, eV ascending. 0.1 MeV is an edge (the fast-flux cut); 13.5-20 MeV isolates the 14.06 MeV source.
GROUP_EDGES_EV = {
    1: [1.0e-5, 2.0e7],
    8: [1.0e-5, 1.0, 1.0e3, 1.0e5, 5.0e5, 2.0e6, 6.0e6, 1.35e7, 2.0e7],
}
SOURCE_ENERGY_EV = 14.06e6
RAY_BOX_CM = ([-650.0, -650.0, -390.0], [650.0, 650.0, 390.0])

DEFAULTS = {
    "groups": 8, "mgxs_method": "stochastic_slab", "mgxs_correction": "P0", "mgxs_nparticles": 20000,
    "rays": 20000, "batches": 60, "inactive": 30,
    "distance_inactive_cm": 500.0, "distance_active_cm": 2500.0,
    "source_shape": "flat", "volume_estimator": "hybrid", "diagonal_stabilization_rho": 1.0, "sample_method": "prng",
    "ww_cell_cm": 25.0, "seed": 81150099,
}


def step(record: dict, name: str, fn, workarounds: list[str] | None = None):
    """Run fn() as a recorded step: ok, seconds, workarounds, error."""
    t0 = time.time()
    entry = {"ok": False, "workarounds": list(workarounds or []), "seconds": None, "error": None}
    record["steps"][name] = entry
    try:
        value = fn()
        entry["ok"] = True
        return value
    except Exception:
        entry["error"] = traceback.format_exc(limit=6)
        return None
    finally:
        entry["seconds"] = time.time() - t0


def finish(record: dict, out: Path) -> int:
    record["peak_rss_mb_self"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    record["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    return 0 if all(s["ok"] for s in record["steps"].values()) else 1


def generate(args) -> int:
    import openmc
    import openmc.mgxs

    params = dict(DEFAULTS)
    params.update({k: v for k, v in (("rays", args.rays), ("batches", args.batches), ("inactive", args.inactive), ("groups", args.groups)) if v is not None})
    edges = GROUP_EDGES_EV[params["groups"]]
    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    record = {"kind": "r6-generate", "openmc_version": openmc.__version__, "parameters": params, "group_edges_ev": edges,
              "threads": args.threads, "steps": {}, "library": RM.library_identity()}
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    h5m = args.model_dir / "rm_m_rr.h5m"
    record["h5m"] = {"file": str(h5m), "sha256": RM.sha256(h5m)}

    state = {}

    def build():
        model, dag = RM.build_model(openmc, h5m, roles, rr=True)
        state["model"], state["dag"] = model, dag
        source = openmc.IndependentSource(energy=openmc.stats.Discrete([SOURCE_ENERGY_EV], [1.0]), particle="neutron",
                                          constraints={"domains": [openmc.Cell(cell_id=ids["plasma"])]}, strength=1.0)
        model.settings.source = [source]
        model.settings.run_mode = "fixed source"
        model.settings.batches = params["batches"]
        model.settings.particles = params["rays"]
        model.tallies = openmc.Tallies([RM.tf_fast_flux_tallies(openmc, ids["tf"])])
        model.tallies[0].filters[1] = openmc.EnergyFilter([1.0e5, edges[-1]])

    step(record, "build_model", build, workarounds=WORKAROUNDS_0153[1:2] + WORKAROUNDS_0153[3:4])
    if not record["steps"]["build_model"]["ok"]:
        return finish(record, args.out)
    model, dag = state["model"], state["dag"]

    mgxs_path = run_dir / "mgxs.h5"

    def mgxs():
        def convert():
            model.convert_to_multigroup(method=params["mgxs_method"], groups=openmc.mgxs.EnergyGroups(edges),
                                        nparticles=params["mgxs_nparticles"], overwrite_mgxs_library=False,
                                        mgxs_path=str(mgxs_path), correction=params["mgxs_correction"])

        try:
            convert()
        except RuntimeError as exc:
            # 0.16.0 builds the stochastic-slab generation model without the library path of the user's materials
            if "cross_sections" not in str(exc):
                raise
            entry = record["steps"]["mgxs"]
            entry["workarounds"].append("openmc.config['cross_sections'] set to the audited library, because the stochastic-slab "
                                        "generation model does not inherit model.materials.cross_sections")
            entry["first_attempt_error"] = str(exc)[:300]
            openmc.config["cross_sections"] = str(RM.FARIS_XS)
            convert()
        if not mgxs_path.is_file():
            raise RuntimeError("MGXS generation wrote no library")
        record["mgxs_sha256"] = RM.sha256(mgxs_path)

    step(record, "mgxs", mgxs)
    if not record["steps"]["mgxs"]["ok"]:
        return finish(record, args.out)

    ww_mesh = openmc.RegularMesh(name="ww-mesh")
    lower, upper = RAY_BOX_CM
    cell = params["ww_cell_cm"]
    ww_mesh.lower_left = lower
    ww_mesh.upper_right = upper
    ww_mesh.dimension = tuple(int(round((u - l) / cell)) for l, u in zip(lower, upper))
    record["ww_mesh"] = {"lower_left": lower, "upper_right": upper, "dimension": list(ww_mesh.dimension), "cell_cm": cell}

    def configure_and_run():
        s = model.settings
        s.energy_mode = "multi-group"
        s.photon_transport = False
        s.batches, s.inactive, s.particles = params["batches"], params["inactive"], params["rays"]
        s.seed = params["seed"]
        s.statepoint = {}
        s.random_ray = {
            "distance_inactive": params["distance_inactive_cm"], "distance_active": params["distance_active_cm"],
            "ray_source": openmc.IndependentSource(space=openmc.stats.Box(lower, upper)),
            "volume_estimator": params["volume_estimator"], "source_shape": params["source_shape"], "adjoint": False,
            "sample_method": params["sample_method"], "diagonal_stabilization_rho": params["diagonal_stabilization_rho"],
            "source_region_meshes": [(ww_mesh, [openmc.Universe(universe_id=dag.id)])],
        }
        s.weight_window_generators = [openmc.WeightWindowGenerator(
            mesh=ww_mesh, energy_bounds=list(edges), particle_type="neutron", method="fw_cadis",
            max_realizations=params["batches"] - params["inactive"])]
        model.export_to_model_xml(str(run_dir))
        run = RM.run_openmc_cli(run_dir, args.threads, "openmc_rr.out")
        record["random_ray_run"] = run
        if run["returncode"] != 0:
            raise RuntimeError(f"openmc random-ray run exited {run['returncode']}; see openmc_rr.out")
        files = sorted(run_dir.glob("weight_windows*.h5"))
        if not files:
            raise RuntimeError("random-ray run wrote no weight_windows.h5")
        record["weight_windows_file"] = str(files[0])
        record["weight_windows_sha256"] = RM.sha256(files[0])

    step(record, "random_ray_fw_cadis", configure_and_run, workarounds=[WORKAROUNDS_0153[0], WORKAROUNDS_0153[2]])
    log = run_dir / "openmc_rr.out"
    if log.is_file():
        text = log.read_text(encoding="utf-8", errors="replace")
        record["random_ray_log_warnings"] = [ln.strip() for ln in text.splitlines() if "WARNING" in ln.upper()][:20]
    if not record["steps"]["random_ray_fw_cadis"]["ok"]:
        return finish(record, args.out)

    def windows_load():
        wws = openmc.hdf5_to_wws(record["weight_windows_file"])
        record["weight_window_sets"] = len(wws)
        record["weight_window_energy_bins"] = [len(w.energy_bounds) - 1 for w in wws]

    step(record, "windows_file", windows_load)

    def windowed():
        result = RM.run_analog(openmc, args.model_dir, args.run_dir.parent / (args.run_dir.name + "_windowed"),
                               args.windowed_histories, args.windowed_batches, args.windowed_seed, args.threads,
                               weight_windows_file=Path(record["weight_windows_file"]))
        record["windowed_run"] = result
        if result["run"]["returncode"] != 0:
            raise RuntimeError("windowed run failed")

    step(record, "windowed_run", windowed)
    return finish(record, args.out)


def analog(args) -> int:
    import openmc

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    record = {"kind": "r6-analog", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "library": RM.library_identity()}

    def run():
        result = RM.run_analog(openmc, args.model_dir, args.run_dir, args.histories, args.batches, args.seed, args.threads)
        record["analog_run"] = result
        if result["run"]["returncode"] != 0:
            raise RuntimeError("analog run failed")

    step(record, "analog_run", run)
    return finish(record, args.out)


def compare_windows(args) -> int:
    import h5py
    import numpy as np

    def load(path):
        out = {}
        with h5py.File(path, "r") as f:
            group = f["weight_windows"]
            for name in sorted(group):
                out[name] = {k: group[name][k][()] for k in ("lower_ww_bounds", "upper_ww_bounds") if k in group[name]}
        return out

    a, b = load(args.files[0]), load(args.files[1])
    record = {"files": [str(p) for p in args.files], "sets": {}}
    for name in sorted(set(a) & set(b)):
        la, lb = np.asarray(a[name]["lower_ww_bounds"], float).ravel(), np.asarray(b[name]["lower_ww_bounds"], float).ravel()
        both = (la > 0) & (lb > 0)
        ratio = lb[both] / la[both] if la.shape == lb.shape else np.array([])
        record["sets"][name] = {
            "cells": int(la.size), "same_shape": bool(la.shape == lb.shape), "cells_positive_in_both": int(both.sum()),
            "cells_positive_only_a": int(((la > 0) & ~(lb > 0)).sum()) if la.shape == lb.shape else None,
            "cells_positive_only_b": int(((lb > 0) & ~(la > 0)).sum()) if la.shape == lb.shape else None,
            "ratio_b_over_a": ({"median": float(np.median(ratio)), "p05": float(np.percentile(ratio, 5)),
                                "p95": float(np.percentile(ratio, 95))} if ratio.size else None),
        }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("generate", "analog"):
        p = sub.add_parser(name)
        p.add_argument("--model-dir", required=True, type=Path)
        p.add_argument("--run-dir", required=True, type=Path)
        p.add_argument("--out", required=True, type=Path)
        p.add_argument("--threads", type=int, default=4)
        if name == "generate":
            p.add_argument("--groups", type=int, choices=sorted(GROUP_EDGES_EV))
            p.add_argument("--rays", type=int)
            p.add_argument("--batches", type=int)
            p.add_argument("--inactive", type=int)
            p.add_argument("--windowed-histories", type=int, default=200_000)
            p.add_argument("--windowed-batches", type=int, default=20)
            p.add_argument("--windowed-seed", type=int, default=20261010)
        else:
            p.add_argument("--histories", type=int, default=1_000_000)
            p.add_argument("--batches", type=int, default=100)
            p.add_argument("--seed", type=int, default=20261009)
    p = sub.add_parser("compare-windows")
    p.add_argument("files", nargs=2, type=Path)
    p.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()
    return {"generate": generate, "analog": analog, "compare-windows": compare_windows}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
