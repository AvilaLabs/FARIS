#!/usr/bin/env python3
"""UX-079 open-pipeline baseline: the two RM-M conversions (docs/notes/CAD_TO_ANSWER_BASELINE.md, "New runs").

    cad_to_answer_baseline.py run --out ~/.cache/avila-night/cad-to-answer-baseline

B2 first (the FARIS CAD helper's `convert` with its defaults), then B1 (cad_to_dagmc 0.14.3 as documented: add_stp_file and
export_dagmc_h5m_file at their defaults except the tolerances, 0.1 cm chord and 0.1 rad). Both run in
~/.cache/avila-night/envs/c2d-0143 under MemoryMax=6G on cores 0-3, each starting on a quiet machine (1-minute load below 2.0
and the FARIS build lock free). Limits: B1 45 min, B2 25 min, 70 min of machine time in total (waiting for a quiet machine is
not counted; a run that hits its limit is reported as "more than" the limit). Each h5m that is written is then loaded in
OpenMC 0.15.3 with DAGMC and run for 10,000 analog histories of the RM-M source; it is valid only if 0 particles are lost.

Internal subcommands (run by the driver inside the right environment): `b1-worker`, `validity`.
Both routes tag each solid with its material tag from rm_m_roles.json (void solids: "void"), so one OpenMC model serves both.
The module imports only the standard library at top level, so the CAD environment can run it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import sys
import time

HERE = Path(__file__).resolve().parent
HOME = Path.home()
REPO = HERE.parents[2]
MODEL = HOME / ".cache/avila-night/cad-risk-tests/models/rm-m"
STEP_NAME, ROLES_NAME = "rm_m.step", "rm_m_roles.json"
C2D_PY = HOME / ".cache/avila-night/envs/c2d-0143/bin/python"
PY0153 = HOME / "micromamba/envs/openmc0153dd/bin/python"
FARIS_CAD = REPO / "integrations/cad/faris_cad.py"
OUT_DEFAULT = HOME / ".cache/avila-night/cad-to-answer-baseline"

B1_LIMIT_S = 45 * 60.0
B2_LIMIT_S = 25 * 60.0
TOTAL_CAP_S = 70 * 60.0
VALIDITY_S = 600.0           # per validity run: its timeout
VALIDITY_RESERVE_S = 300.0   # kept per validity run still to come when sizing a conversion's limit (protocol: about 2 min each)
MIN_CONVERSION_S = 300.0     # a conversion with less than this left under the cap is not started
VALIDITY_HISTORIES = 10000
VALIDITY_SEED = 90150001
SHAPE_UNIT_M = 1.0e-3        # RM-M's STEP is millimetres (the CAD was built in mm); faris_cad turns this into 0.1 cm per unit
CHORD_CM, ANGULAR_RAD = 0.1, 0.1
SCALE_FACTOR = 0.1           # mm to cm, passed to add_stp_file as in the protocol


# ---------------------------------------------------------------------------
# Pure helpers (tested)

def solid_tags(roles: list[dict]) -> list[str]:
    """One material tag per solid in STEP order (the roles file lists the solids in that order); void solids keep "void"."""
    return [r["tag"] for r in roles]


def tag_entries(roles: list[dict]) -> list[dict]:
    """The tags file faris_cad.py convert reads: [{"step_index": n, "tag": "<tag>"}]."""
    return [{"step_index": i, "tag": t} for i, t in enumerate(solid_tags(roles))]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def effective_limit(route: str, used_s: float, validities_left: int) -> float:
    """A conversion's wall limit: its own limit, but never beyond the total cap less what the validity runs still need."""
    own = {"B1": B1_LIMIT_S, "B2": B2_LIMIT_S}[route]
    return max(0.0, min(own, TOTAL_CAP_S - used_s - validities_left * VALIDITY_RESERVE_S))


def can_start(limit_s: float) -> bool:
    return limit_s >= MIN_CONVERSION_S


def wall_report(wall_s: float, timed_out: bool, limit_s: float) -> str:
    """Reported time: the measured seconds, or "more than <limit>" when the run hit its limit."""
    return f"more than {limit_s:.0f} s" if timed_out else f"{wall_s:.0f} s"


def validity_verdict(returncode, lost: dict | None) -> dict:
    """Valid only if the 10,000-history run completed and lost 0 particles (no particle restart files)."""
    if returncode != 0 or lost is None:
        return {"valid": False, "reason": f"OpenMC run did not complete (exit {returncode})"}
    n = lost.get("restart_files")
    return {"valid": n == 0, "lost_particle_files": n,
            "reason": None if n == 0 else f"{n} particle restart files: particles were lost"}


def volume_mapping(roles: list[dict], facets: dict, rtol: float = 1.0e-3) -> dict:
    """Do the h5m's volume ids and faceted volumes match the roles file (the ids the OpenMC model uses)? Reported, not gating."""
    want = {int(r["volume_id"]): float(r["cad_volume_cm3"]) for r in roles}
    got = {int(v): float(f["faceted_volume"]) for v, f in facets.items()}
    off = sorted(v for v in want if v not in got or abs(got[v] / want[v] - 1.0) > rtol)
    return {"ids_match": set(want) == set(got), "volumes_within_rtol": not off, "rtol": rtol, "ids_off": off[:30]}


def conversion_record(route: str, command: list[str], job: dict, limit_s: float, h5m: Path | None, extra: dict) -> dict:
    """The per-route record: wall time, load, limit and outcome (hashes and counts are added by the caller)."""
    timed_out = job["returncode"] == "timeout"
    rec = {"route": route, "command": command, "limit_s": limit_s, "returncode": job["returncode"], "wall_s": job["wall_s"],
           "timed_out": timed_out, "wall_report": wall_report(job["wall_s"], timed_out, limit_s),
           "loadavg_start": job.get("loadavg_start"), "loadavg_end": job.get("loadavg_end"),
           "contended": job.get("contended"), "wait": job.get("wait"), **extra}
    rec["h5m_written"] = bool(h5m is not None and h5m.is_file() and job["returncode"] == 0)
    return rec


# ---------------------------------------------------------------------------
# Workers

def peak_rss_mb() -> float:
    own = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    kids = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    return max(own, kids) / 1024.0


def b1_worker(args) -> int:
    """B1, as documented: CadToDagmc().add_stp_file(...), export_dagmc_h5m_file(...) at defaults but the tolerances."""
    import cad_to_dagmc
    from importlib.metadata import version

    roles = json.loads((args.model_dir / ROLES_NAME).read_text(encoding="utf-8"))["solids"]
    tags = solid_tags(roles)
    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    h5m = out_dir / "model.h5m"
    t0 = time.monotonic()
    model = cad_to_dagmc.CadToDagmc()
    n = model.add_stp_file(str(args.model_dir / STEP_NAME), scale_factor=SCALE_FACTOR, material_tags=tags)
    t_read = time.monotonic() - t0
    model.export_dagmc_h5m_file(filename=str(h5m), tolerance=CHORD_CM, angular_tolerance=ANGULAR_RAD)
    total = time.monotonic() - t0
    record = {"route": "B1", "volumes_added": n, "tags": len(tags), "timing_seconds": {"read_step": t_read, "total": total},
              "peak_rss_mb": peak_rss_mb(), "h5m": {"file": h5m.name, "bytes": h5m.stat().st_size},
              "settings": {"scale_factor": SCALE_FACTOR, "tolerance": CHORD_CM, "angular_tolerance": ANGULAR_RAD,
                           "imprint": "default (True)", "overlap_scan": "default (on)"},
              "versions": {"cad_to_dagmc": version("cad_to_dagmc"), "mesher": version("cad_to_dagmc_mesher"),
                           "cadquery": version("cadquery")}}
    (out_dir / "b1-worker.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(record, indent=1, sort_keys=True))
    return 0


def validity(args) -> int:
    """Load the h5m in OpenMC 0.15.3 with DAGMC and run 10,000 analog histories of the RM-M source; count lost particles."""
    sys.path.insert(0, str(HERE))
    import openmc
    import h5m_volumes
    import rm_m_openmc as RM

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    facets = h5m_volumes.facet_volumes(str(args.h5m))
    mapping = volume_mapping(roles, facets)
    out = {"h5m": str(args.h5m), "openmc_version": openmc.__version__, "histories": VALIDITY_HISTORIES,
           "volumes": len(facets), "triangles": int(sum(f["triangles"] for f in facets.values())), "volume_mapping": mapping}
    model, _ = RM.build_model(openmc, args.h5m, roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = model.settings
    s.run_mode = "fixed source"
    s.particles, s.batches, s.seed = VALIDITY_HISTORIES, 1, VALIDITY_SEED
    s.source = [source]
    s.photon_transport = False
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    model.export_to_model_xml(str(run_dir))
    t0 = time.time()
    run = RM.run_openmc_cli(run_dir, args.threads)
    lost = RM.count_lost_particles(run_dir) if run["returncode"] == 0 else None
    out.update(run=run, lost=lost, wall_s=time.time() - t0, **validity_verdict(run["returncode"], lost))
    args.out.write_text(json.dumps(out, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=1, sort_keys=True, default=str))
    return 0 if out["valid"] else 1


# ---------------------------------------------------------------------------
# Driver

def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}


def convert_and_check(route: str, out: Path, used: list[float], validities_left: int, results: dict) -> None:
    import s0e_run as R

    d = out / route.lower()
    d.mkdir(parents=True, exist_ok=True)
    limit = effective_limit(route, used[0], validities_left)
    if not can_start(limit):
        results[route] = {"route": route, "note": f"NOT_EVALUATED: {limit:.0f} s left under the {TOTAL_CAP_S:.0f} s cap"}
        return
    if route == "B2":
        roles = json.loads((MODEL / ROLES_NAME).read_text(encoding="utf-8"))["solids"]
        (d / "tags.json").write_text(json.dumps(tag_entries(roles)) + "\n", encoding="utf-8")
        command = [str(C2D_PY), str(FARIS_CAD), "convert", str(MODEL / STEP_NAME), "--shape-unit-m", f"{SHAPE_UNIT_M:e}",
                   "--tags", str(d / "tags.json"), "--out-dir", str(d), "--report", str(d / "conversion.json")]
    else:
        command = [str(C2D_PY), str(Path(__file__).resolve()), "b1-worker", "--model-dir", str(MODEL), "--out-dir", str(d)]
    job = R.job(command, d / "convert.log", limit, wait=True, pylib=False)
    used[0] += job["wall_s"]
    h5m = d / "model.h5m"
    report = read_json(d / ("conversion.json" if route == "B2" else "b1-worker.json"))
    extra = {"tool_report": report, "peak_rss_mb": report.get("peak_rss_mb"), "effective_limit_s": limit}
    rec = conversion_record(route, command, job, limit, h5m, extra)
    if rec["h5m_written"]:
        rec["h5m"] = {"path": str(h5m), "bytes": h5m.stat().st_size, "sha256": sha256(h5m)}
    results[route] = rec
    if not rec["h5m_written"]:
        rec["validity"] = {"valid": None, "reason": "NOT_EVALUATED: no h5m was written (" + rec["wall_report"] + ")"}
        return
    vlimit = min(VALIDITY_S, max(60.0, TOTAL_CAP_S - used[0]))
    vjob = R.job([str(PY0153), str(Path(__file__).resolve()), "validity", "--model-dir", str(MODEL), "--h5m", str(h5m),
                  "--run-dir", str(d / "validity"), "--out", str(d / "validity.json")],
                 d / "validity.log", vlimit, wait=True)
    used[0] += vjob["wall_s"]
    v = read_json(d / "validity.json")
    rec["validity"] = {**v, "job": vjob} if v else {"valid": False, "reason": f"validity run failed (exit {vjob['returncode']})",
                                                    "job": vjob}
    rec["triangles"] = v.get("triangles")


def driver(args) -> int:
    sys.path.insert(0, str(HERE))
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    results: dict = {"protocol": "docs/notes/CAD_TO_ANSWER_BASELINE.md", "start_epoch": time.time(), "limits_s":
                     {"B1": B1_LIMIT_S, "B2": B2_LIMIT_S, "total": TOTAL_CAP_S}}
    used = [0.0]
    for route, left in (("B2", 2), ("B1", 1)):
        convert_and_check(route, out, used, left, results)
        results["machine_time_s"] = used[0]
        (out / "results.json").write_text(json.dumps(results, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: (v.get("wall_report"), (v.get("validity") or {}).get("valid")) if isinstance(v, dict) and "route" in v
                      else v for k, v in results.items() if k in ("B1", "B2", "machine_time_s")}, indent=1, default=str))
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="B2, validity, B1, validity under the 70 min cap")
    r.add_argument("--out", type=Path, default=OUT_DEFAULT)
    w = sub.add_parser("b1-worker", help="(internal) B1 conversion in the CAD environment")
    w.add_argument("--model-dir", required=True, type=Path)
    w.add_argument("--out-dir", required=True, type=Path)
    v = sub.add_parser("validity", help="(internal) 10,000 analog histories of an h5m in OpenMC 0.15.3")
    v.add_argument("--model-dir", required=True, type=Path)
    v.add_argument("--h5m", required=True, type=Path)
    v.add_argument("--run-dir", required=True, type=Path)
    v.add_argument("--out", required=True, type=Path)
    v.add_argument("--threads", type=int, default=4)
    args = ap.parse_args(argv)
    return {"run": driver, "b1-worker": b1_worker, "validity": validity}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
