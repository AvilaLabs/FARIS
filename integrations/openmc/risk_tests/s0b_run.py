#!/usr/bin/env python3
"""S0b continuous-energy runs on RM-M (D1, D2, D3 production, D4, TF spectrum). OpenMC 0.15.3, either build.

    s0b_run.py run --model-dir D --run-dir R --out record.json --label L --seed S [--histories N | --budget SECONDS]
                   [--stop-r 0.05] [--windows W.h5] [--max-split 1000] [--checkpoints] [--particles 10000] [--threads 4]

One fixed-source neutron run with the tokamak source at strength 1 (tallies are "per source neutron"). Response (a) is the
TF fast flux (E > 0.1 MeV) over the TF coil material in cm per source neutron; the TBR is the H3 production summed over every
lithium-bearing material. The run is monitored: the newest statepoint is read about every `--poll` seconds, and the run is
stopped (by process group) when response (a) reaches `--stop-r` or the wall budget ends. T is the time from process start to
the modification time of the statepoint used, as in S0, so start-up and window loading count.

The record carries the inputs, the build (executable, tracer line from the log), seed, threads, CPU affinity, load average at
start and end, wall time, peak RSS of the children, lost particles, timing lines from the log, and the raw tallies with sigma.
Statepoints are thinned while the run goes: the newest are kept, and one at least every `--keep-spacing` seconds.
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
import r1_worker  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402


def sp_number(path: Path) -> int:
    return int(path.stem.split(".")[1])


def read_statepoint(openmc, path: Path) -> dict:
    """Response (a) and TBR from one statepoint (mean, std_error, relative error) and its batch count."""
    with openmc.StatePoint(str(path)) as sp:
        a = sp.get_tally(name="tf_fast_a")
        am, asd = float(a.mean.ravel()[0]), float(a.std_dev.ravel()[0])
        t = sp.get_tally(name="tbr")
        tm = float(t.mean.sum())
        ts = float((t.std_dev ** 2).sum() ** 0.5)
        batches = int(sp.n_batches) if hasattr(sp, "n_batches") else sp_number(path)
    return {"batches": batches,
            "a": {"mean": am, "std_error": asd, "relative_error": asd / am if am > 0 else None, "unit": checks.UNIT_PER_SOURCE},
            "tbr": {"mean": tm, "std_error": ts, "relative_error": ts / tm if tm > 0 else None, "unit": "tritons per source neutron"}}


def thin_statepoints(run_dir: Path, last_kept_mtime: list, spacing_s: float, keep_newest: int = 3) -> None:
    """Delete old statepoints but keep the newest few and one every `spacing_s`."""
    files = sorted(run_dir.glob("statepoint.*.h5"), key=sp_number)
    for p in files[:-keep_newest]:
        m = p.stat().st_mtime
        if last_kept_mtime[0] is None or m - last_kept_mtime[0] >= spacing_s:
            last_kept_mtime[0] = m
            continue
        p.unlink()


def run_monitored(openmc, run_dir: Path, threads: int, budget_s: float, stop_r: float | None, poll_s: float, keep_spacing: float,
                  stop_min_batches: int = 20) -> dict:
    exe = Path(sys.executable).parent / "openmc"
    env = dict(os.environ, OMP_NUM_THREADS=str(threads))
    t0 = time.time()
    stopped_by = None
    series = []
    last_kept = [None]
    seen = None
    with (run_dir / "openmc.out").open("w", encoding="utf-8") as log:
        proc = subprocess.Popen([str(exe), "--threads", str(threads)], cwd=run_dir, stdout=log, stderr=subprocess.STDOUT,
                                env=env, start_new_session=True)
        rc = None
        while rc is None:
            try:
                rc = proc.wait(timeout=poll_s)
                break
            except subprocess.TimeoutExpired:
                pass
            elapsed = time.time() - t0
            files = sorted(run_dir.glob("statepoint.*.h5"), key=sp_number)
            fresh = [p for p in files if time.time() - p.stat().st_mtime > 2.0]
            if fresh and fresh[-1] != seen:
                seen = fresh[-1]
                try:
                    rec = read_statepoint(openmc, seen)
                    rec["seconds_to_statepoint"] = seen.stat().st_mtime - t0
                    series.append(rec)
                    with (run_dir / "monitor.jsonl").open("a", encoding="utf-8") as m:
                        m.write(json.dumps({"elapsed": elapsed, **rec}) + "\n")
                    if stop_r is not None and rec["a"]["relative_error"] is not None and rec["batches"] >= stop_min_batches \
                            and rec["a"]["relative_error"] <= stop_r:
                        stopped_by = "R_reached"
                except OSError:
                    pass
            thin_statepoints(run_dir, last_kept, keep_spacing)
            if stopped_by is None and elapsed >= budget_s:
                stopped_by = "budget"
            if stopped_by is not None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    rc = proc.wait(timeout=60)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    rc = proc.wait()
    return {"returncode": rc, "stopped_by": stopped_by or "completed", "wall_seconds": time.time() - t0, "t_start": t0,
            "executable": str(exe), "monitor_points": len(series)}


def build_production(openmc, args, run_dir: Path):
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = model.settings
    s.run_mode = "fixed source"
    if args.histories:
        s.particles = args.particles
        s.batches = max(1, args.histories // args.particles)
    else:
        s.particles = args.particles
        s.batches = args.max_batches
    s.seed = args.seed
    s.source = [source]
    s.photon_transport = False
    last = s.batches
    if args.statepoint_every > 0:
        s.statepoint = {"batches": sorted(set(list(range(args.statepoint_every, last + 1, args.statepoint_every)) + [last]))}
    else:
        s.statepoint = {"batches": [last]}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    applied = None
    if args.windows is not None:
        wws = openmc.hdf5_to_wws(str(args.windows))
        applied = {"file": str(args.windows), "sha256": RM.sha256(args.windows), "max_split_in_file": wws[0].max_split,
                   "survival_ratio": wws[0].survival_ratio}
        for ww in wws:
            ww.mesh.id = 9000  # the file's mesh id can collide with the source mesh
            if args.max_split is not None:
                ww.max_split = args.max_split
        applied["max_split_used"] = wws[0].max_split
        s.weight_windows = wws
        s.weight_windows_on = True
    if args.checkpoints:
        s.weight_window_checkpoints = {"surface": True, "collision": True}
    li_mats = [m for m in model.materials if {"Li6", "Li7"} & set(m.get_nuclides())]
    tbr = openmc.Tally(name="tbr")
    tbr.filters = [openmc.MaterialFilter(li_mats)]
    tbr.scores = ["H3-production"]
    tallies = [r1_worker.response_a_material_tally(openmc, model), tbr]
    if args.spectrum_bins:
        # D5a source: the TF coil flux per source neutron in log-spaced bins from 0.1 MeV to 20 MeV
        edges = [r1_worker.cfg.FAST_CUT_EV * (2.0e7 / r1_worker.cfg.FAST_CUT_EV) ** (i / args.spectrum_bins) for i in range(args.spectrum_bins + 1)]
        spec_tally = openmc.Tally(name="tf_spectrum")
        spec_tally.filters = [openmc.MaterialFilter([r1_worker.tf_material(model)]), openmc.EnergyFilter(edges)]
        spec_tally.scores = ["flux"]
        tallies.append(spec_tally)
    model.tallies = openmc.Tallies(tallies)
    model.export_to_model_xml(str(run_dir))
    return applied, unscaled, [m.name for m in li_mats]


def run(args) -> int:
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in list(run_dir.glob("statepoint.*.h5")) + list(run_dir.glob("particle_*.h5")):
        old.unlink()
    (run_dir / "monitor.jsonl").unlink(missing_ok=True)
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    applied, unscaled, li = build_production(openmc, args, run_dir)
    record = {"kind": "s0b-run", "label": args.label, "openmc_version": openmc.__version__, "python": sys.executable,
              "threads": args.threads, "affinity_cpus": sorted(os.sched_getaffinity(0)), "seed": args.seed,
              "particles_per_batch": args.particles, "histories_requested": args.histories, "budget_s": args.budget,
              "stop_r": args.stop_r, "source_strength": 1.0, "windows": applied, "checkpoints_surface_and_collision": bool(args.checkpoints),
              "max_split_override": args.max_split, "lithium_materials": li, "library": RM.library_identity(),
              "loadavg_start": C.read_loadavg()}
    run_info = run_monitored(openmc, run_dir, args.threads, args.budget, args.stop_r, args.poll, args.keep_spacing)
    record["loadavg_end"] = C.read_loadavg()
    record["run"] = run_info
    record["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    log = (run_dir / "openmc.out").read_text(encoding="utf-8", errors="replace")
    record["log"] = C.parse_transport_log(log)
    record["lost"] = RM.count_lost_particles(run_dir)
    files = sorted(run_dir.glob("statepoint.*.h5"), key=sp_number)
    if not files:
        record["error"] = "no statepoint written inside the budget"
    else:
        last = files[-1]
        # a statepoint cut off mid-write is skipped in favour of the previous one
        for candidate in reversed(files):
            try:
                resp = read_statepoint(openmc, candidate)
                last = candidate
                break
            except OSError:
                continue
        resp["statepoint"] = last.name
        resp["statepoint_sha256"] = RM.sha256(last)
        resp["seconds_to_statepoint"] = last.stat().st_mtime - run_info["t_start"]
        resp["histories"] = resp["batches"] * args.particles
        resp["histories_per_second"] = resp["histories"] / resp["seconds_to_statepoint"] if resp["seconds_to_statepoint"] > 0 else None
        if args.spectrum_bins:
            with openmc.StatePoint(str(last)) as sp:
                t = sp.get_tally(name="tf_spectrum")
                bins = list(t.filters[1].bins)
                resp["spectrum"] = {"edges_ev": [bins[0][0]] + [b[1] for b in bins], "mean": t.mean.ravel().tolist(),
                                    "std_error": t.std_dev.ravel().tolist(), "unit": checks.UNIT_PER_SOURCE}
        record["response"] = resp
        record["fom_a"] = C.fom_with_sigma(resp["a"]["relative_error"], resp["seconds_to_statepoint"], resp["batches"])
        for p in files:
            if p != last:
                p.unlink()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: record.get(k) for k in ("label", "response", "fom_a", "lost", "log")}, indent=1, default=str))
    return 0 if "response" in record else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--model-dir", required=True, type=Path)
    p.add_argument("--run-dir", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--label", required=True)
    p.add_argument("--seed", required=True, type=int)
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--particles", type=int, default=10000)
    p.add_argument("--histories", type=int, default=0, help="fixed number of histories (otherwise run to --budget or --stop-r)")
    p.add_argument("--max-batches", type=int, default=200000)
    p.add_argument("--budget", type=float, default=1200.0)
    p.add_argument("--stop-r", type=float, default=None)
    p.add_argument("--statepoint-every", type=int, default=5)
    p.add_argument("--poll", type=float, default=20.0)
    p.add_argument("--keep-spacing", type=float, default=300.0)
    p.add_argument("--windows", type=Path)
    p.add_argument("--max-split", type=int)
    p.add_argument("--spectrum-bins", type=int, default=0, help="add a log-spaced TF flux spectrum tally (0.1 to 20 MeV)")
    p.add_argument("--checkpoints", action="store_true", help="weight-window checkpoints at surface crossings and collisions")
    args = ap.parse_args()
    if args.histories:
        args.budget = max(args.budget, 1.0e9)
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
