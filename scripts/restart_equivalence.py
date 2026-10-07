#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Compare the restart continuation method with the full one on real ACTINV runs.

Takes the `decay` folder of an existing maintenance-coupling run (the continuation specs and their provenance files),
the decay cache holding the full method's points files for those specs, an ACTINV binary and its data directory. It
builds the trunk and cooling specs with the driver's own functions, runs them (into its own cache under --work-dir) and
compares, at every cooling-grid point, the restart curve with the cached full curve for heat and, where present, dose:

    relative difference = |a - b| / max(|a|, |b|, 1e-9 * max |value| over that curve)

The output JSON gives, per curve (component, installation, shutdown_s), the largest relative difference for heat and
dose and the trunk's n_states_below_floor and heat_bound_from_below_floor_W_per_g at that shutdown, the overall worst,
the tolerance and pass or fail. Exit status: 0 within tolerance, 1 outside it, 2 bad input or a tool failure.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCHEMA = "faris-restart-equivalence/v1"
DEFAULT_TOLERANCE = 1e-6
FLOOR_FRACTION = 1e-9


def load_driver():
    spec = importlib.util.spec_from_file_location("maintenance_coupling_test", HERE / "maintenance_coupling_test.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault("maintenance_coupling_test", module)
    spec.loader.exec_module(module)
    return module


def max_relative_difference(a: list, b: list) -> float | None:
    """Largest per-point relative difference of two equally long curves; None if either has no values."""
    if len(a) != len(b):
        raise ValueError(f"curves of {len(a)} and {len(b)} points")
    if any(x is None for x in a) != any(x is None for x in b):
        raise ValueError("one curve has missing values the other has not")
    if not a or any(x is None for x in a):
        return None
    floor = FLOOR_FRACTION * max(max(abs(x) for x in a), max(abs(x) for x in b))
    worst = 0.0
    for x, y in zip(a, b):
        scale = max(abs(x), abs(y), floor)
        if scale > 0:
            worst = max(worst, abs(x - y) / scale)
    return worst


def compare(driver, provs: list, spec_dir: Path, full_cache: Path, runner, restart_cache: Path) -> list:
    grid = driver.cooling_grid_s()
    restart = runner.restart_curves(provs, spec_dir, restart_cache)
    rows = []
    for prov in provs:
        key = (prov["component"], prov["installation_index"], int(round(prov["continuation_of_shutdown_s"])))
        spec = spec_dir / prov["spec_file"]
        sha = hashlib.sha256(spec.read_bytes()).hexdigest()
        stored = json.loads((full_cache / f"{sha}.points.json").read_text(encoding="utf-8"))
        full = driver.cooling_curve(stored, grid, prov["mass_g"], spec.name)
        got = restart[key]["points"]
        row = {"component": key[0], "installation": key[1], "shutdown_s": key[2],
               "heat_max_relative_difference": max_relative_difference([p[1] for p in full], [p[1] for p in got]),
               "dose_max_relative_difference": max_relative_difference([p[2] for p in full], [p[2] for p in got])}
        row.update(runner.restart_info[key])
        rows.append(row)
    return sorted(rows, key=lambda r: (r["component"], r["installation"], r["shutdown_s"]))


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--spec-dir", required=True, help="a run's decay folder: continuation specs and provenance files")
    ap.add_argument("--decay-cache", required=True, help="folder with the full method's <sha>.points.json files")
    ap.add_argument("--actinv", required=True)
    ap.add_argument("--data-dir", required=True)
    ap.add_argument("--output", required=True)
    ap.add_argument("--work-dir", help="where the restart specs and cache go (default: <output>.work)")
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--variant", default=None, help="provenance variant (default: bare_lower_bound)")
    ap.add_argument("--tolerance", type=float, default=DEFAULT_TOLERANCE)
    ap.add_argument("--components", help="comma-separated component ids to include")
    ap.add_argument("--limit-installations", type=int,
                    help="keep only the first N installations of each component")
    args = ap.parse_args(argv)
    driver = load_driver()
    try:
        variant = args.variant or driver.BARE
        src = Path(args.spec_dir)
        provs = [json.loads(p.read_text(encoding="utf-8")) for p in sorted(src.glob(f"*__{variant}.provenance.json"))]
        provs = [p for p in provs if "continuation_of_shutdown_s" in p]
        if args.components:
            wanted = set(args.components.split(","))
            provs = [p for p in provs if p["component"] in wanted]
        if args.limit_installations is not None:
            provs = [p for p in provs if p["installation_index"] <= args.limit_installations]
        if not provs:
            raise driver.Refused(f"no continuation provenance files for variant {variant} in {src}")
        output = Path(args.output)
        work = Path(args.work_dir) if args.work_dir else output.with_name(output.name + ".work")
        # the restart specs are written beside the originals' copies, not into the run's own folder
        spec_dir = work / "specs"
        spec_dir.mkdir(parents=True, exist_ok=True)
        for p in provs:
            (spec_dir / p["spec_file"]).write_bytes((src / p["spec_file"]).read_bytes())
        runner = driver.Runner({"actinv": args.actinv, "data_dir": args.data_dir}, work, {}, {})
        runner.actinv_workers = max(1, args.workers)
        rows = compare(driver, provs, spec_dir, Path(args.decay_cache), runner, work / "restart-cache")
    except (driver.Refused, driver.ToolError, OSError, ValueError, KeyError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    values = [v for r in rows for v in (r["heat_max_relative_difference"], r["dose_max_relative_difference"])
              if v is not None]
    worst = max(values) if values else None
    passed = worst is not None and worst <= args.tolerance
    driver.write_json(output, {
        "schema": SCHEMA, "variant": variant, "tolerance": args.tolerance, "curves": rows,
        "worst_relative_difference": worst, "passed": passed,
        "runs": {"trunk": runner.trunk_runs, "trunk_cache_hits": runner.trunk_cache_hits,
                 "restart_cooling": runner.restart_cooling_runs,
                 "restart_cooling_cache_hits": runner.restart_cooling_cache_hits}})
    print(f"{'PASS' if passed else 'FAIL'}: worst relative difference {worst} over {len(rows)} curves "
          f"(tolerance {args.tolerance:g}); wrote {output}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
