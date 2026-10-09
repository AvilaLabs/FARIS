#!/usr/bin/env python3
"""One R1 step on OpenMC 0.15.3. Run by r1_driver.py under the memory cap.

    r1_worker.py generate --config C2 --model-dir D --run-dir R --out step.json
        MGXS, then a random-ray forward and adjoint FW-CADIS solve; writes weight_windows.h5.
    r1_worker.py produce  --config C2 --windows W.h5 --model-dir D --run-dir R --out step.json
        Continuous-energy run with those windows for a fixed wall budget (default 1800 s).
    r1_worker.py analog   --model-dir D --run-dir R --out step.json
        The same run with no windows for 7200 s.

Every tally is per source neutron (source strength 1): "cm per source neutron". The OpenMC
"flux" score is a track length and is not divided by volume.

The adjoint source. OpenMC 0.15.3 builds the FW-CADIS adjoint source from the tallies in the
model: it is the inverse of the forward random-ray estimate in the tallied regions and energy
bins. Objective (a) is one tally over the TF coil cells. Objective (a)+(b) adds a second tally,
the (b) cylindrical mesh restricted to the TF material, so each response is weighted by the inverse
of its own forward estimate. The weights are not user settable in 0.15.3.
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
import r6_worker as W  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402

GEN_DEFAULTS = dict(W.DEFAULTS)


def peak_mesh(openmc):
    g = cfg.peak_mesh_grids()
    return openmc.CylindricalMesh(r_grid=g["r_grid_cm"], z_grid=g["z_grid_cm"], phi_grid=[p * 3.141592653589793 / 180.0 for p in g["phi_grid_deg"]],
                                  mesh_id=9001, name="peak-b"), g


def tf_material(model):
    return next(m for m in model.materials if m.name == "tf_coil")


def response_b_tally(openmc, model, energy_edges):
    mesh, _ = peak_mesh(openmc)
    t = openmc.Tally(name="peak_b")
    t.filters = [openmc.MeshFilter(mesh), openmc.MaterialFilter([tf_material(model)]), openmc.EnergyFilter(energy_edges)]
    t.scores = ["flux"]
    return t


def response_a_material_tally(openmc, model, name="tf_fast_a"):
    """Response (a) as one bin: the TF material is only in the 18 coils, so this integrates over all of them."""
    t = openmc.Tally(name=name)
    t.filters = [openmc.MaterialFilter([tf_material(model)]), openmc.EnergyFilter([cfg.FAST_CUT_EV, 2.0e7])]
    t.scores = ["flux"]
    return t


def generate(args) -> int:
    import openmc
    import openmc.mgxs

    conf = cfg.CONFIGS[args.config]
    params = dict(GEN_DEFAULTS)
    for key in ("rays", "batches", "inactive"):
        if getattr(args, key) is not None:
            params[key] = getattr(args, key)
    edges = cfg.STRUCTURES[conf["structure"]]
    resp_edges = cfg.fast_response_edges(conf["structure"])
    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    gen_seed = cfg.GENERATION_SEEDS[args.config]
    record = {"kind": "r1-generate", "config": args.config, "definition": conf, "openmc_version": openmc.__version__,
              "parameters": params, "group_edges_ev": edges, "adjoint_response_energy_edges_ev": resp_edges,
              "generation_seed": gen_seed, "mgxs_seed": cfg.MGXS_SEED, "threads": args.threads, "steps": {},
              "library": RM.library_identity()}
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    h5m = args.model_dir / "rm_m_rr.h5m"
    record["h5m"] = {"file": str(h5m), "sha256": RM.sha256(h5m)}
    state = {}

    def build():
        model, dag = RM.build_model(openmc, h5m, roles, rr=True)
        state["model"], state["dag"] = model, dag
        model.settings.source = [openmc.IndependentSource(
            energy=openmc.stats.Discrete([W.SOURCE_ENERGY_EV], [1.0]), particle="neutron",
            constraints={"domains": [openmc.Cell(cell_id=ids["plasma"])]}, strength=1.0)]
        model.settings.run_mode = "fixed source"
        model.settings.batches = params["batches"]
        model.settings.particles = params["rays"]
        a = RM.tf_fast_flux_tallies(openmc, ids["tf"], name="response_a")
        a.filters[1] = openmc.EnergyFilter(resp_edges)
        tallies = [a]
        if cfg.objective_includes_b(args.config):
            tallies.append(response_b_tally(openmc, model, resp_edges))
        model.tallies = openmc.Tallies(tallies)

    W.step(record, "build_model", build, workarounds=W.WORKAROUNDS_0153[1:2] + W.WORKAROUNDS_0153[3:4])
    if not record["steps"]["build_model"]["ok"]:
        return W.finish(record, args.out)
    model, dag = state["model"], state["dag"]
    mgxs_path = run_dir / "mgxs.h5"

    def mgxs():
        model.settings.seed = cfg.MGXS_SEED
        model.convert_to_multigroup(method=params["mgxs_method"], groups=openmc.mgxs.EnergyGroups(edges),
                                    nparticles=params["mgxs_nparticles"], overwrite_mgxs_library=False,
                                    mgxs_path=str(mgxs_path), correction=params["mgxs_correction"])
        if not mgxs_path.is_file():
            raise RuntimeError("MGXS generation wrote no library")
        record["mgxs_sha256"] = RM.sha256(mgxs_path)

    W.step(record, "mgxs", mgxs)
    if not record["steps"]["mgxs"]["ok"]:
        return W.finish(record, args.out)

    ww_mesh = openmc.RegularMesh(name="ww-mesh")
    lower, upper = W.RAY_BOX_CM
    cell = params["ww_cell_cm"]
    ww_mesh.lower_left, ww_mesh.upper_right = lower, upper
    ww_mesh.dimension = tuple(int(round((u - l) / cell)) for l, u in zip(lower, upper))
    record["ww_mesh"] = {"lower_left": lower, "upper_right": upper, "dimension": list(ww_mesh.dimension), "cell_cm": cell}

    def configure_and_run():
        s = model.settings
        s.energy_mode = "multi-group"
        s.photon_transport = False
        s.batches, s.inactive, s.particles = params["batches"], params["inactive"], params["rays"]
        s.seed = gen_seed
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

    W.step(record, "random_ray_fw_cadis", configure_and_run,
           workarounds=[W.WORKAROUNDS_0153[0], W.WORKAROUNDS_0153[2]])
    log = run_dir / "openmc_rr.out"
    if log.is_file():
        text = log.read_text(encoding="utf-8", errors="replace")
        record["random_ray_log_warnings"] = sorted({ln.strip() for ln in text.splitlines() if "WARNING" in ln.upper()})[:20]
    if record["steps"]["random_ray_fw_cadis"]["ok"]:
        summarise_windows(openmc, record)
    return W.finish(record, args.out)


def summarise_windows(openmc, record):
    import numpy as np

    wws = openmc.hdf5_to_wws(record["weight_windows_file"])
    lo, up = wws[0].lower_ww_bounds, wws[0].upper_ww_bounds
    pos = lo > 0
    record["windows"] = {"sets": len(wws), "energy_bins": len(wws[0].energy_bounds) - 1, "mesh_dimension": list(map(int, wws[0].mesh.dimension)),
                         "fraction_positive": float(pos.mean()), "upper_over_lower": float(np.median(up[pos] / lo[pos])),
                         "survival_ratio": wws[0].survival_ratio, "max_split": wws[0].max_split, "lower_max": float(lo.max()),
                         "lower_min_positive": float(lo[pos].min())}


def run_budgeted(run_dir: Path, threads: int, budget_s: float, log_name: str = "openmc.out") -> dict:
    """Run openmc for at most budget_s of wall time; stop it by process group when the budget ends."""
    exe = Path(sys.executable).parent / "openmc"
    env = dict(os.environ, OMP_NUM_THREADS=str(threads))
    t0 = time.time()
    timed_out = False
    with (run_dir / log_name).open("w", encoding="utf-8") as log:
        proc = subprocess.Popen([str(exe), "--threads", str(threads)], cwd=run_dir, stdout=log, stderr=subprocess.STDOUT,
                                env=env, start_new_session=True)
        try:
            rc = proc.wait(timeout=budget_s)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                rc = proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                rc = proc.wait()
    return {"returncode": rc, "ran_to_budget": timed_out, "wall_seconds": time.time() - t0, "t_start": t0, "executable": str(exe),
            "peak_rss_mb": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0}


def read_production(openmc, run_dir: Path, t_start: float, particles: int) -> dict:
    """Read the last statepoint: response (a) and the (b) peak voxel. T is the time at which that statepoint was written."""
    import numpy as np

    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    if not files:
        raise RuntimeError("no statepoint written inside the budget")
    last = files[-1]
    seconds = last.stat().st_mtime - t_start
    out = {"statepoint": last.name, "batches": int(last.stem.split(".")[1]), "histories": int(last.stem.split(".")[1]) * particles,
           "seconds_to_statepoint": seconds}
    g = cfg.peak_mesh_grids()
    with openmc.StatePoint(str(last)) as sp:
        a = sp.get_tally(name="tf_fast_a")
        mean, sd = float(a.mean.ravel()[0]), float(a.std_dev.ravel()[0])
        out["a"] = {"mean": mean, "std_error": sd, "relative_error": cfg.relative_error(mean, sd), "unit": checks.UNIT_PER_SOURCE}
        b = sp.get_tally(name="peak_b")
        nr, nphi, nz = len(g["r_grid_cm"]) - 1, len(g["phi_grid_deg"]) - 1, len(g["z_grid_cm"]) - 1
        shape = b.mean.shape
        flat_mean, flat_sd = b.mean.ravel(), b.std_dev.ravel()
        out["b_nonzero_voxels"] = int((flat_mean > 0).sum())
        if (flat_mean > 0).any():
            i = int(np.argmax(flat_mean))
            # OpenMC structured-mesh bins run r fastest, then phi, then z
            ir, rem = i % nr, i // nr
            ip, iz = rem % nphi, rem // nphi
            out["b"] = {"peak_value": float(flat_mean[i]), "std_error": float(flat_sd[i]),
                        "relative_error": cfg.relative_error(float(flat_mean[i]), float(flat_sd[i])), "unit": checks.UNIT_PER_SOURCE,
                        "voxel": {"index": [ir, ip, iz], "r_cm": [g["r_grid_cm"][ir], g["r_grid_cm"][ir + 1]],
                                  "phi_deg": [g["phi_grid_deg"][ip], g["phi_grid_deg"][ip + 1]],
                                  "z_cm": [g["z_grid_cm"][iz], g["z_grid_cm"][iz + 1]], "coil": ip // 2 if ip % 2 == 0 else None}}
            out["b_tally_shape"] = list(shape)
        else:
            out["b"] = None
    return out


def production(args, windows: Path | None, label: str, budget: float, seed: int) -> dict:
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    roles = RM.load_roles(args.model_dir)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, RM.role_ids(roles)["plasma"], rate=1.0)
    s = model.settings
    particles = args.particles
    s.run_mode = "fixed source"
    s.batches = args.max_batches
    s.particles = particles
    s.seed = seed
    s.source = [source]
    s.photon_transport = False
    s.statepoint = {"batches": list(range(args.statepoint_every, args.max_batches + 1, args.statepoint_every))}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    applied = None
    if windows is not None:
        wws = openmc.hdf5_to_wws(str(windows))
        conf = cfg.CONFIGS[label] if label in cfg.CONFIGS else {}
        applied = {"file": str(windows), "sha256": RM.sha256(windows)}
        for ww in wws:
            ww.mesh.id = 9000  # the file's mesh id can collide with the source mesh (both id 1)
            if conf.get("upper_lower_ratio"):
                ww.upper_ww_bounds = ww.lower_ww_bounds * conf["upper_lower_ratio"]
            if conf.get("survival_ratio"):
                ww.survival_ratio = conf["survival_ratio"]
        applied.update({"upper_over_lower_set": conf.get("upper_lower_ratio"), "survival_ratio_set": conf.get("survival_ratio"),
                        "parameters": "WeightWindows.upper_ww_bounds = lower_ww_bounds * ratio; WeightWindows.survival_ratio"
                        if conf.get("upper_lower_ratio") else "as generated (survival_ratio %s, upper/lower 5)" % wws[0].survival_ratio,
                        "survival_ratio": wws[0].survival_ratio, "max_split": wws[0].max_split})
        s.weight_windows = wws
        s.weight_windows_on = True
    model.tallies = openmc.Tallies([response_a_material_tally(openmc, model), response_b_tally(openmc, model, [cfg.FAST_CUT_EV, 2.0e7])])
    model.export_to_model_xml(str(run_dir))
    run = run_budgeted(run_dir, args.threads, budget)
    result = {"label": label, "seed": seed, "budget_seconds": budget, "particles_per_batch": particles, "source_strength": 1.0,
              "windows": applied, "run": run, "threads": args.threads}
    result["lost_log_lines"] = RM.count_lost_particles(run_dir)
    result["response"] = read_production(openmc, run_dir, run["t_start"], particles)
    # keep only the last statepoint
    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    for p in files[:-1]:
        p.unlink()
    return result


def produce(args) -> int:
    import openmc

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    record = {"kind": "r1-produce", "config": args.config, "definition": cfg.CONFIGS[args.config], "openmc_version": openmc.__version__,
              "threads": args.threads, "steps": {}, "library": RM.library_identity()}

    def run():
        record["production"] = production(args, args.windows, args.config, args.budget, cfg.PRODUCTION_SEEDS[args.config])

    W.step(record, "production", run)
    return W.finish(record, args.out)


def analog(args) -> int:
    import openmc

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    record = {"kind": "r1-analog", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "library": RM.library_identity()}

    def run():
        record["production"] = production(args, None, "analog", args.budget, cfg.ANALOG_SEED)

    W.step(record, "analog", run)
    return W.finish(record, args.out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("generate", "produce", "analog"):
        p = sub.add_parser(name)
        p.add_argument("--model-dir", required=True, type=Path)
        p.add_argument("--run-dir", required=True, type=Path)
        p.add_argument("--out", required=True, type=Path)
        p.add_argument("--threads", type=int, default=4)
        if name != "analog":
            p.add_argument("--config", required=True, choices=sorted(cfg.CONFIGS))
        if name == "generate":
            p.add_argument("--rays", type=int)
            p.add_argument("--batches", type=int)
            p.add_argument("--inactive", type=int)
        else:
            p.add_argument("--budget", type=float, default=cfg.PRODUCTION_BUDGET_S if name == "produce" else cfg.ANALOG_BUDGET_S)
            p.add_argument("--particles", type=int, default=10000)
            p.add_argument("--max-batches", type=int, default=20000)
            p.add_argument("--statepoint-every", type=int, default=5)
        if name == "produce":
            p.add_argument("--windows", required=True, type=Path)
    args = ap.parse_args()
    return {"generate": generate, "produce": produce, "analog": analog}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
