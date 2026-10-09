#!/usr/bin/env python3
"""R1 driver (docs/notes/CAD_TRANSPORT_RISK_TESTS.md): six window configurations plus the analog reference.

    r1_driver.py --model-dir DIR --work-dir DIR --out results-r1.json [--configs C1,C2,...] [--skip-analog]

For each configuration: a generation job (MGXS, random-ray FW-CADIS) and a production job of 1800 s at 4 threads. The analog
reference runs for 7200 s. Jobs run one at a time under the memory cap (r6_version.run_guarded); a step whose
output JSON already exists is not repeated. OpenMC 0.15.3 only.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_configs as cfg  # noqa: E402
import r6_version as R6  # noqa: E402

HERE = Path(__file__).resolve().parent
PYTHON = R6.ENVIRONMENTS["0.15.3"]
GENERATE_TIMEOUT_S = 4 * 3600.0


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--work-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--configs", default="C1,C2,C3,C4,C5,C6")
    ap.add_argument("--skip-analog", action="store_true")
    ap.add_argument("--threads", type=int, default=4)
    args = ap.parse_args()
    work = args.work_dir
    jobs = {}
    names = args.configs.split(",")
    for name in names:
        base = work / name
        gen_json, prod_json = base / "generate.json", base / "produce.json"
        if not gen_json.is_file():
            cmd = [PYTHON, HERE / "r1_worker.py", "generate", "--config", name, "--model-dir", args.model_dir,
                   "--run-dir", base / "generate", "--out", gen_json, "--threads", args.threads]
            jobs[f"{name}:generate"] = R6.run_guarded(cmd, base / "generate.log", GENERATE_TIMEOUT_S, args.threads)
        gen = load(gen_json)
        if not gen or not all(s["ok"] for s in gen["steps"].values()):
            print(f"{name}: generation did not complete; production skipped", flush=True)
            continue
        if not prod_json.is_file():
            cmd = [PYTHON, HERE / "r1_worker.py", "produce", "--config", name, "--windows", gen["weight_windows_file"],
                   "--model-dir", args.model_dir, "--run-dir", base / "produce", "--out", prod_json, "--threads", args.threads]
            jobs[f"{name}:produce"] = R6.run_guarded(cmd, base / "produce.log", cfg.PRODUCTION_BUDGET_S + 900.0, args.threads)
    analog_json = work / "analog" / "analog.json"
    if not args.skip_analog and not analog_json.is_file():
        cmd = [PYTHON, HERE / "r1_worker.py", "analog", "--model-dir", args.model_dir, "--run-dir", work / "analog" / "run",
               "--out", analog_json, "--threads", args.threads]
        jobs["analog"] = R6.run_guarded(cmd, work / "analog" / "analog.log", cfg.ANALOG_BUDGET_S + 900.0, args.threads)

    result = {"schema": "faris-r1/v1", "protocol": "docs/notes/CAD_TRANSPORT_RISK_TESTS.md#r1", "openmc": "0.15.3",
              "model_dir": str(args.model_dir), "production_budget_s": cfg.PRODUCTION_BUDGET_S, "analog_budget_s": cfg.ANALOG_BUDGET_S,
              "generation_seeds": cfg.GENERATION_SEEDS, "mgxs_seed": cfg.MGXS_SEED, "production_seeds": cfg.PRODUCTION_SEEDS,
              "analog_seed": cfg.ANALOG_SEED, "driver_jobs": jobs, "configs": {}}
    prods = {}
    for name in cfg.CONFIGS:
        gen, prod = load(work / name / "generate.json"), load(work / name / "produce.json")
        entry = {"definition": cfg.CONFIGS[name], "generate": gen, "produce": prod}
        gt = R6.parse_time_file((work / name / "generate.log").with_suffix(".time"))
        pt = R6.parse_time_file((work / name / "produce.log").with_suffix(".time"))
        entry["generate_job"], entry["produce_job"] = gt, pt
        result["configs"][name] = entry
        if prod and prod["steps"]["production"]["ok"]:
            prods[name] = prod["production"]
    analog = load(analog_json)
    result["analog"] = analog
    result["analog_job"] = R6.parse_time_file(analog_json.with_suffix(".time").parent / "analog.time") if analog else None
    if prods and analog and analog["steps"]["analog"]["ok"]:
        result["summary"] = cfg.summarise(prods, analog["production"])
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps(result.get("summary", {}).get("verdict"), indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
