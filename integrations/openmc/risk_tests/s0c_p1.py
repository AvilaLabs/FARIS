#!/usr/bin/env python3
"""S0c Q1: window resolution on the P1 slab (inboard radial build of RM-M as layers normal to x). OpenMC 0.15.3, double-down env.

    s0c_p1.py layers
    s0c_p1.py mgxs     --run-dir D --out R.json
    s0c_p1.py analog   --run-dir D --out R.json [--budget 600 --stop-r 0.05]
    s0c_p1.py generate --spacing 5 --mgxs D/mgxs.h5 --run-dir D --out R.json
    s0c_p1.py produce  --label L --windows W.h5 --run-dir D --out R.json --seed S [--floor] [--budget 120]

P1: layers normal to x, 100 cm x 100 cm in y and z with reflective sides, vacuum at both x ends, 200 cm long. The plasma-side
layer (vacuum, 47 cm) carries a uniform isotropic 14.1 MeV source. Response (a') is the neutron flux above 0.1 MeV in the TF
inboard-leg layer, in cm per source neutron. Windows come from random ray (stochastic-slab MGXS, CASMO-8 plus a 0.1 MeV edge,
FW-CADIS adjoint on (a')) on a regular x mesh with one cell in y and z; the mesh is both the source-region mesh and the window
mesh. Production applies `max_split` 1000 and surface and collision checkpoints. T is the time from process start to the
statepoint used (start-up counts), as in S0b.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_configs as cfg  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402
import s0c_common as S  # noqa: E402

STRUCTURE = "CASMO-8+0.1MeV"
RR = dict(rays=20000, batches=60, inactive=30, distance_inactive_cm=500.0, distance_active_cm=2500.0, source_shape="flat",
          volume_estimator="hybrid", diagonal_stabilization_rho=1.0, sample_method="prng", mgxs_nparticles=20000)
HALF = S.SLAB_WIDTH_CM / 2.0
TALLY = "a_prime"
MAX_BATCHES = 20000   # the statepoint list is written into the XML, so the cap stays small


def build_geometry(openmc, rr: bool):
    """(geometry, materials, cells by layer name). Void layers are vacuum in the continuous-energy model and H-1 at 0.001 g/cm3
    ('filler') in the random-ray model, as in S0b."""
    layers = S.p1_layers()
    tags = {layer["tag"] for layer in layers if layer["tag"] != "void"}
    mats = RM.build_materials(openmc, tags | ({"filler"} if rr else set()))
    edges = S.layer_edges(layers)
    xs = []
    for i, e in enumerate(edges):
        bc = "vacuum" if i in (0, len(edges) - 1) else "transmission"
        xs.append(openmc.XPlane(x0=e, boundary_type=bc, surface_id=100 + i))
    y0, y1 = openmc.YPlane(y0=-HALF, boundary_type="reflective", surface_id=200), openmc.YPlane(y0=HALF, boundary_type="reflective", surface_id=201)
    z0, z1 = openmc.ZPlane(z0=-HALF, boundary_type="reflective", surface_id=202), openmc.ZPlane(z0=HALF, boundary_type="reflective", surface_id=203)
    cells = {}
    for i, layer in enumerate(layers):
        fill = mats.get(layer["tag"]) if layer["tag"] != "void" else (mats["filler"] if rr else None)
        region = +xs[i] & -xs[i + 1] & +y0 & -y1 & +z0 & -z1
        cells[layer["name"]] = openmc.Cell(cell_id=i + 1, name=layer["name"], fill=fill, region=region)
    geometry = openmc.Geometry(list(cells.values()))
    return geometry, mats, cells


def source_box(layers):
    p = layers[0]
    return [p["x0_cm"], -HALF, -HALF], [p["x1_cm"], HALF, HALF]


def ce_model(openmc, args_seed: int, particles: int, batches: int, statepoint_every: int):
    geometry, mats, cells = build_geometry(openmc, rr=False)
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(list(mats.values())))
    model.materials.cross_sections = str(RM.FARIS_XS)
    layers = S.p1_layers()
    lo, hi = source_box(layers)
    s = model.settings
    s.run_mode = "fixed source"
    s.particles, s.batches, s.seed = particles, batches, args_seed
    s.source = [openmc.IndependentSource(space=openmc.stats.Box(lo, hi), angle=openmc.stats.Isotropic(),
                                         energy=openmc.stats.Discrete([S.SOURCE_ENERGY_EV], [1.0]), particle="neutron", strength=1.0)]
    s.photon_transport = False
    s.statepoint = {"batches": sorted(set(range(statepoint_every, batches + 1, statepoint_every)))} if statepoint_every else {}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 1000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    t = openmc.Tally(name=TALLY)
    t.filters = [openmc.CellFilter([cells["tf_inboard_leg"]]), openmc.EnergyFilter([S.FIRST_WALL_FAST_CUT_EV, 2.0e7])]
    t.scores = ["flux"]
    model.tallies = openmc.Tallies([t])
    return model, cells


def read_a(openmc, path: Path) -> dict:
    with openmc.StatePoint(str(path)) as sp:
        t = sp.get_tally(name=TALLY)
        m, sd = float(t.mean.ravel()[0]), float(t.std_dev.ravel()[0])
        return {"batches": int(sp.current_batch), "mean": m, "std_error": sd, "relative_error": sd / m if m > 0 else None}


def sp_number(p: Path) -> int:
    return int(p.stem.split(".")[1])


def run_timed(openmc, run_dir: Path, threads: int, budget_s: float, stop_r: float | None, poll_s: float, stop_min_batches: int = 20) -> dict:
    """Run openmc, polling the newest statepoint; stop by process group at the budget or when R(a') <= stop_r."""
    exe = Path(sys.executable).parent / "openmc"
    env = dict(os.environ, OMP_NUM_THREADS=str(threads))
    t0 = time.time()
    stopped_by = None
    with (run_dir / "openmc.out").open("w", encoding="utf-8") as log:
        proc = subprocess.Popen([str(exe), "--threads", str(threads)], cwd=run_dir, stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
        rc = None
        while rc is None:
            try:
                rc = proc.wait(timeout=poll_s)
                break
            except subprocess.TimeoutExpired:
                pass
            elapsed = time.time() - t0
            files = sorted(run_dir.glob("statepoint.*.h5"), key=sp_number)
            for p in files[:-3]:
                p.unlink(missing_ok=True)
            fresh = [p for p in files[-3:] if time.time() - p.stat().st_mtime > 0.5]
            if stop_r is not None and fresh:
                try:
                    rec = read_a(openmc, fresh[-1])
                    if rec["relative_error"] is not None and rec["batches"] >= stop_min_batches and rec["relative_error"] <= stop_r:
                        stopped_by = "R_reached"
                except OSError:
                    pass
            if stopped_by is None and elapsed >= budget_s:
                stopped_by = "budget"
            if stopped_by is not None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    rc = proc.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    rc = proc.wait()
    return {"returncode": rc, "stopped_by": stopped_by or "completed", "wall_seconds": time.time() - t0, "t_start": t0, "executable": str(exe)}


def finish_run(openmc, run_dir: Path, run: dict, particles: int, record: dict) -> None:
    files = sorted(run_dir.glob("statepoint.*.h5"), key=sp_number)
    resp = None
    for candidate in reversed(files):
        try:
            resp = read_a(openmc, candidate)
            resp["statepoint"] = candidate.name
            resp["statepoint_sha256"] = RM.sha256(candidate)
            resp["seconds_to_statepoint"] = candidate.stat().st_mtime - run["t_start"]
            break
        except OSError:
            continue
    if resp is None:
        record["error"] = "no readable statepoint"
        return
    resp["histories"] = resp["batches"] * particles
    resp["histories_per_second"] = resp["histories"] / resp["seconds_to_statepoint"]
    record["response"] = resp
    record["fom_a"] = C.fom_with_sigma(resp["relative_error"], resp["seconds_to_statepoint"], resp["batches"]) if resp["batches"] >= 2 else None
    for p in files:
        if p.name != resp["statepoint"]:
            p.unlink(missing_ok=True)
    record["lost"] = RM.count_lost_particles(run_dir)


def write(record: dict, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")


def cmd_layers(args) -> int:
    layers = S.p1_layers()
    print(f"{'layer':<22}{'material tag':<20}{'thickness cm':>13}{'x0':>8}{'x1':>8}")
    for layer in layers:
        print(f"{layer['name']:<22}{layer['tag']:<20}{layer['thickness_cm']:>13.1f}{layer['x0_cm']:>8.1f}{layer['x1_cm']:>8.1f}")
    for n in (25.0, 10.0, 5.0, 2.5):
        print(f"spacing {n}: {S.mesh_dimension(S.SLAB_TOTAL_CM, n)} cells, {S.source_region_count(S.layer_edges(layers), n)} source regions")
    return 0


def cmd_mgxs(args) -> int:
    import openmc
    import openmc.mgxs

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    geometry, mats, cells = build_geometry(openmc, rr=True)
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(list(mats.values())))
    model.materials.cross_sections = str(RM.FARIS_XS)
    model.settings.run_mode = "fixed source"
    model.settings.seed = S.SEEDS["mgxs"]
    path = run_dir / "mgxs.h5"
    t0 = time.time()
    model.convert_to_multigroup(method="stochastic_slab", groups=openmc.mgxs.EnergyGroups(cfg.STRUCTURES[STRUCTURE]), nparticles=RR["mgxs_nparticles"],
                                overwrite_mgxs_library=False, mgxs_path=str(path), correction="P0")
    rec = {"kind": "s0c-p1-mgxs", "seconds": time.time() - t0, "mgxs": str(path), "mgxs_sha256": RM.sha256(path), "seed": S.SEEDS["mgxs"],
           "group_edges_ev": cfg.STRUCTURES[STRUCTURE], "openmc_version": openmc.__version__, "library": RM.library_identity(),
           "peak_rss_mb": max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0}
    write(rec, args.out)
    return 0


def cmd_analog(args) -> int:
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    model, _ = ce_model(openmc, S.SEEDS["analog"], args.particles, MAX_BATCHES, 1)
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "s0c-p1-analog", "seed": S.SEEDS["analog"], "threads": args.threads, "particles_per_batch": args.particles,
           "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(), "stop_r": args.stop_r, "budget_s": args.budget,
           "openmc_version": openmc.__version__, "library": RM.library_identity()}
    run = run_timed(openmc, run_dir, args.threads, args.budget, args.stop_r, args.poll)
    rec["run"], rec["loadavg_end"] = run, C.read_loadavg()
    finish_run(openmc, run_dir, run, args.particles, rec)
    write(rec, args.out)
    print(json.dumps({k: rec.get(k) for k in ("response", "fom_a", "run")}, indent=1, default=str))
    return 0 if "response" in rec else 1


def cmd_generate(args) -> int:
    import numpy as np
    import openmc
    import openmc.mgxs

    spacing = args.spacing
    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    layers = S.p1_layers()
    n = S.mesh_dimension(S.SLAB_TOTAL_CM, spacing)
    expected_sr = S.source_region_count(S.layer_edges(layers), spacing)
    geometry, mats, cells = build_geometry(openmc, rr=True)
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(list(mats.values())))
    model.materials.cross_sections = str(RM.FARIS_XS)
    model.settings.run_mode = "fixed source"
    model.settings.seed = S.SEEDS["mgxs"]
    model.convert_to_multigroup(method="stochastic_slab", groups=openmc.mgxs.EnergyGroups(cfg.STRUCTURES[STRUCTURE]), nparticles=RR["mgxs_nparticles"],
                                overwrite_mgxs_library=False, mgxs_path=str(args.mgxs), correction="P0")
    edges = cfg.STRUCTURES[STRUCTURE]
    resp_edges = cfg.fast_response_edges(STRUCTURE)
    mesh = openmc.RegularMesh(name="ww-mesh")
    mesh.lower_left, mesh.upper_right, mesh.dimension = [0.0, -HALF, -HALF], [S.SLAB_TOTAL_CM, HALF, HALF], (n, 1, 1)
    s = model.settings
    s.run_mode = "fixed source"
    s.photon_transport = False
    s.batches, s.inactive, s.particles = RR["batches"], RR["inactive"], RR["rays"]
    s.seed = S.SEEDS["gen"][spacing]
    s.statepoint = {}
    s.source = [openmc.IndependentSource(energy=openmc.stats.Discrete([S.SOURCE_ENERGY_EV], [1.0]), particle="neutron",
                                         constraints={"domains": [cells[S.PLASMA_LAYER_NAME]]}, strength=1.0)]
    lo, hi = [0.0, -HALF, -HALF], [S.SLAB_TOTAL_CM, HALF, HALF]
    s.random_ray = {"distance_inactive": RR["distance_inactive_cm"], "distance_active": RR["distance_active_cm"],
                    "ray_source": openmc.IndependentSource(space=openmc.stats.Box(lo, hi)), "volume_estimator": RR["volume_estimator"],
                    "source_shape": RR["source_shape"], "adjoint": False, "sample_method": RR["sample_method"],
                    "diagonal_stabilization_rho": RR["diagonal_stabilization_rho"], "source_region_meshes": [(mesh, [model.geometry.root_universe])]}
    s.weight_window_generators = [openmc.WeightWindowGenerator(mesh=mesh, energy_bounds=list(edges), particle_type="neutron", method="fw_cadis",
                                                               max_realizations=RR["batches"] - RR["inactive"])]
    a = openmc.Tally(name="response_a")
    a.filters = [openmc.CellFilter([cells["tf_inboard_leg"]]), openmc.EnergyFilter(resp_edges)]
    a.scores = ["flux"]
    model.tallies = openmc.Tallies([a])
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "s0c-p1-generate", "spacing_cm": spacing, "mesh_dimension": [n, 1, 1], "seed": s.seed, "threads": args.threads,
           "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(), "parameters": RR, "group_edges_ev": edges,
           "adjoint_response_energy_edges_ev": resp_edges, "mgxs_sha256": RM.sha256(args.mgxs), "expected_source_regions": expected_sr,
           "openmc_version": openmc.__version__, "library": RM.library_identity()}
    run = RM.run_openmc_cli(run_dir, args.threads, "openmc_rr.out")
    rec["run"], rec["loadavg_end"] = run, C.read_loadavg()
    rec["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    log = (run_dir / "openmc_rr.out").read_text(encoding="utf-8", errors="replace")
    rec["random_ray_log_stats"] = C.parse_random_ray_log(log)
    if run["returncode"] != 0:
        rec["error"] = f"random-ray run exited {run['returncode']}"
        write(rec, args.out)
        return 1
    wfile = sorted(run_dir.glob("weight_windows*.h5"))
    if not wfile:
        rec["error"] = "no weight_windows.h5"
        write(rec, args.out)
        return 1
    rec["weight_windows_file"] = str(wfile[0])
    rec["weight_windows_sha256"] = RM.sha256(wfile[0])
    wws = openmc.hdf5_to_wws(str(wfile[0]))
    lower = np.asarray(wws[0].lower_ww_bounds).ravel().tolist()
    rec["windows"] = {"energy_bins": len(wws[0].energy_bounds) - 1, "mesh_dimension": list(map(int, wws[0].mesh.dimension)),
                      "cells": len(lower), "survival_ratio": wws[0].survival_ratio, "max_split": wws[0].max_split, **S.window_range(lower)}
    forward = rec["random_ray_log_stats"].get("forward") or {}
    rec["source_regions"] = {"expected": expected_sr, "reported_forward": forward.get("source_regions")}
    write(rec, args.out)
    print(json.dumps({k: rec.get(k) for k in ("spacing_cm", "run", "peak_rss_mb_children", "source_regions", "windows")}, indent=1, default=str))
    return 0


def cmd_produce(args) -> int:
    import numpy as np
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    model, cells = ce_model(openmc, args.seed, args.particles, MAX_BATCHES, args.statepoint_every)
    wws = openmc.hdf5_to_wws(str(args.windows))
    applied = {"file": str(args.windows), "sha256": RM.sha256(args.windows), "floor": bool(args.floor)}
    for ww in wws:
        ww.mesh.id = 9000
        ww.max_split = S.MAX_SPLIT
        if args.floor:
            shape = np.asarray(ww.lower_ww_bounds).shape
            fl = S.apply_floor(np.asarray(ww.lower_ww_bounds).ravel().tolist(), np.asarray(ww.upper_ww_bounds).ravel().tolist(), S.FLOOR_REL)
            ww.lower_ww_bounds = np.asarray(fl["lower"]).reshape(shape)
            ww.upper_ww_bounds = np.asarray(fl["upper"]).reshape(shape)
            applied["floor_result"] = {k: fl[k] for k in ("floor_value", "raised", "non_positive", "cells")}
            applied["range_after_floor"] = S.window_range(fl["lower"])
    model.settings.weight_windows = wws
    model.settings.weight_windows_on = True
    model.settings.weight_window_checkpoints = {"surface": True, "collision": True}
    applied["max_split_used"] = S.MAX_SPLIT
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "s0c-p1-produce", "label": args.label, "seed": args.seed, "threads": args.threads, "particles_per_batch": args.particles,
           "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(), "budget_s": args.budget, "windows": applied,
           "checkpoints_surface_and_collision": True, "openmc_version": openmc.__version__, "library": RM.library_identity()}
    run = run_timed(openmc, run_dir, args.threads, args.budget, None, args.poll)
    rec["run"], rec["loadavg_end"] = run, C.read_loadavg()
    finish_run(openmc, run_dir, run, args.particles, rec)
    write(rec, args.out)
    print(json.dumps({k: rec.get(k) for k in ("label", "response", "fom_a")}, indent=1, default=str))
    return 0 if "response" in rec else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("layers")
    for name in ("mgxs", "analog", "generate", "produce"):
        p = sub.add_parser(name)
        p.add_argument("--run-dir", required=True, type=Path)
        p.add_argument("--out", required=True, type=Path)
        p.add_argument("--threads", type=int, default=4)
        if name in ("analog", "produce"):
            p.add_argument("--particles", type=int, default=2000)
            p.add_argument("--poll", type=float, default=1.0)
        if name == "analog":
            p.add_argument("--budget", type=float, default=S.ANALOG_BUDGET_S)
            p.add_argument("--stop-r", type=float, default=S.ANALOG_R_STOP)
        if name == "generate":
            p.add_argument("--spacing", type=float, required=True, choices=S.SPACINGS_CM)
            p.add_argument("--mgxs", type=Path, required=True)
        if name == "produce":
            p.add_argument("--label", required=True)
            p.add_argument("--windows", type=Path, required=True)
            p.add_argument("--seed", type=int, required=True)
            p.add_argument("--floor", action="store_true")
            p.add_argument("--budget", type=float, default=S.PRODUCTION_S)
            p.add_argument("--statepoint-every", type=int, default=2)
    args = ap.parse_args()
    return {"layers": cmd_layers, "mgxs": cmd_mgxs, "analog": cmd_analog, "generate": cmd_generate, "produce": cmd_produce}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
