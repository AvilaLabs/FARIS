# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run the authored, one-boundary-outage native lifecycle control case."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import sys
import time

HERE = Path(__file__).resolve().parent
CASE_DIR = HERE / "cases" / "controlled_fixed"
MANIFEST_PATH = CASE_DIR / "manifest.json"
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import compare as peer_compare


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_case() -> tuple[dict, dict[str, Path]]:
    case = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if case.get("schema") != "faris-controlled-lifecycle-case/0.1.0":
        raise ValueError("unsupported controlled-case schema")
    inputs = {}
    for peer in ("process", "bluemira"):
        record = case["peers"][peer]["input" if peer == "process" else "config"]
        path = (CASE_DIR / record["path"]).resolve()
        if not path.is_relative_to(CASE_DIR.resolve()) or not path.is_file():
            raise ValueError(f"{peer} input is missing or outside the controlled-case directory")
        if file_sha256(path) != record["sha256"]:
            raise ValueError(f"{peer} input SHA-256 mismatch")
        inputs[peer] = path
    authored = case["authored_inputs"]
    process_config = json.loads(inputs["process"].read_text(encoding="utf-8"))
    process_expectations = {
        "plant_horizon_years": authored["calendar_horizon_years"],
        "blanket_lifetime_fpy": authored["blanket_lifetime_full_power_years"],
        "divertor_lifetime_fpy": authored["process_divertor_lifetime_fpy"],
        "blanket_replacement_days": authored["fixed_blanket_replacement_days"],
        "net_electric_power_mw": authored["net_electric_normalization_mw"],
        "pulse_burn_seconds": authored["pulse_period_seconds"],
        "pulse_total_seconds": authored["pulse_period_seconds"],
    }
    if any(not near(process_config.get(key), value, tol=1e-12) for key, value in process_expectations.items()):
        raise ValueError("PROCESS input does not match the authored controlled-case manifest")
    if (process_config.get("unplanned_unavailability") != dict.fromkeys(
            ("uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"), 0.0)):
        raise ValueError("PROCESS unplanned fractions do not match the zero-unplanned case")
    blue_config = json.loads(inputs["bluemira"].read_text(encoding="utf-8"))
    blue_life = blue_config["lifecycle_config"]
    expected_rate = 10.2 * authored["fusion_power_mw"] / 2037.0
    expected_dpa = expected_rate * authored["blanket_lifetime_full_power_years"]
    if (not near(blue_life.get("bmd"), authored["fixed_blanket_replacement_days"] * 86400.0, tol=1e-6)
            or not near(blue_life.get("blk_1_dpa"), expected_dpa, tol=1e-12)
            or not near(blue_life.get("blk_2_dpa"), expected_dpa, tol=1e-12)
            or not near(blue_life.get("blk_dmg"), expected_rate, tol=1e-12)
            or not near(blue_life.get("t_pulse"), authored["pulse_period_seconds"], tol=1e-12)
            or not near(blue_life.get("A_global"), 1.0, tol=0.0)
            or not near(blue_life.get("div_dmg"), authored["bluemira_compat_divertor_damage_rate_dpa_fpy"], tol=1e-12)
            or not near(blue_life.get("div_dpa"), authored["bluemira_compat_divertor_damage_rate_dpa_fpy"]
                        * authored["bluemira_compat_divertor_lifetime_fpy"], tol=1e-12)
            or not near(blue_life.get("dmd"), 0.0, tol=0.0)
            or blue_config.get("availability_strategy") != {"kind": "zero_unplanned", "parameters": {}}):
        raise ValueError("bluemira config does not match the authored duration, damage-rate, pulse, or zero-work compatibility control")
    faris = (HERE.parents[1] / case["faris_reference"]["path_from_repository_root"]).resolve()
    if not faris.is_file() or file_sha256(faris) != case["faris_reference"]["sha256"]:
        raise ValueError("FARIS reference archive is missing or its SHA-256 differs")
    inputs["faris"] = faris
    return case, inputs


def near(actual, expected: float, tol: float = 1e-6) -> bool:
    return isinstance(actual, (int, float)) and not isinstance(actual, bool) and math.isfinite(actual) and math.isclose(float(actual), expected, rel_tol=0, abs_tol=tol)


def evaluate(case: dict, process: dict, blue: dict, faris_fixed: dict, faris_computed: dict) -> dict:
    authored = case["authored_inputs"]
    horizon = authored["calendar_horizon_years"]
    expected_replacements = authored["expected_blanket_replacement_count"]
    campaign_life_years = authored["blanket_lifetime_full_power_years"]
    replacement_years = authored["fixed_blanket_replacement_days"] / 365.25
    expected_availability = (authored["blanket_cycle_count"] * campaign_life_years) / (
        authored["blanket_cycle_count"] * campaign_life_years + replacement_years
    )
    checks = {
        "process_horizon_is_authored": near(process.get("plant_horizon_years_input"), horizon),
        "process_zero_unplanned": isinstance(process.get("unplanned_unavailability"), dict)
        and all(near(value, 0.0, tol=0.0) for value in process["unplanned_unavailability"].values()),
        "process_fixed_duration_is_authored": near(process.get("replacement_duration_years", {}).get("blanket"), authored["fixed_blanket_replacement_days"] / 365.25),
        "process_native_availability_matches_control": near(process.get("plant_availability_fraction"), expected_availability),
        "bluemira_horizon_matches_authored": near(blue.get("calendar_years"), horizon),
        "bluemira_one_boundary_blanket_replacement": near(blue.get("blanket_replacements"), expected_replacements, tol=0.0),
        "bluemira_zero_unplanned": near(blue.get("unplanned_downtime"), 0.0),
        "bluemira_operational_load_factor_is_unity": near(blue.get("operational_load_factor_input"), 1.0, tol=0.0),
        "bluemira_zero_work_divertor_compatibility_phases": (
            near(blue.get("divertor_replacements"), authored["expected_bluebmira_zero_work_divertor_phases"], tol=0.0)
            and near(blue.get("planned_maintenance"), authored["fixed_blanket_replacement_days"] * 86400.0, tol=1e-6)
        ),
        "peer_availability_agrees": near(process.get("plant_availability_fraction"), blue.get("overall_availability")),
    }
    mwh = None
    if (checks["process_horizon_is_authored"] and checks["bluemira_horizon_matches_authored"]
            and isinstance(process.get("capacity_factor"), (int, float))
            and isinstance(blue.get("full_power_years"), (int, float))):
        power = authored["net_electric_normalization_mw"]
        mwh = {
            "unit": "conditional MWh(e)",
            "basis": "shared authored MW(e) × peer-native full-power years × 8760 h/year; not a native electrical output",
            "process": power * float(process["capacity_factor"]) * horizon * 8760,
            "bluemira": power * float(blue["full_power_years"]) * 8760,
        }
    passed = all(checks.values())
    return {
        "status": "matched controlled code-to-code baseline" if passed else "incomplete; native output checks did not all match",
        "checks": checks,
        "expected_planned_availability_from_authored_one_outage_schedule": expected_availability,
        "conditional_energy_normalization": mwh,
        "faris_fixed_reference": faris_fixed,
        "faris_computed_reference": faris_computed,
        "faris_computed_minus_fixed": {
            "lifetime_net_electricity_mwh": faris_computed["lifetime_net_electricity_mwh"] - faris_fixed["lifetime_net_electricity_mwh"],
            "total_replacement_downtime_s": faris_computed["total_replacement_downtime_s"] - faris_fixed["total_replacement_downtime_s"],
            "availability_fraction": faris_computed["availability_fraction"] - faris_fixed["availability_fraction"],
        },
        "faris_comparison_note": "The FARIS archived 30-year fixed/computed result is separate historical context. It is not the authored one-outage horizon and is not physical validation.",
    }


def run(process_python: str, bluemira_python: str, out: Path) -> dict:
    case, inputs = load_case()
    if out.exists() and any(out.iterdir()):
        raise ValueError(f"output directory is not empty: {out}")
    out.mkdir(parents=True, exist_ok=True)
    timeout, cap = case["limits"]["timeout_seconds"], case["limits"]["output_bytes"]
    outputs = {}
    for peer, python, config in (
        ("process", process_python, inputs["process"]),
        ("bluemira", bluemira_python, inputs["bluemira"]),
    ):
        folder = out / peer
        folder.mkdir()
        target = folder / ("availability.json" if peer == "process" else "timeline.json")
        worker = HERE / ("process_availability_worker.py" if peer == "process" else "bluemira_worker.py")
        args = [python, str(worker), str(config), str(target)]
        if peer == "process":
            args.append(case["peers"][peer]["version"])
        else:
            args.extend((str(case["peers"][peer]["rng_seed"]), case["peers"][peer]["version"]))
        started = time.monotonic()
        record = peer_compare.bounded_run(args, cwd=folder, timeout=timeout, output_bytes=cap)
        if record["return_code"] != 0 or not target.is_file():
            raise RuntimeError(f"{peer} worker failed or omitted native output: {record['stderr'][-1000:]}")
        if target.stat().st_size > cap:
            raise ValueError(f"{peer} output exceeds configured byte cap")
        outputs[peer] = json.loads(target.read_text(encoding="utf-8"))
        record.update({"input_sha256": file_sha256(config), "output_sha256": file_sha256(target),
                       "output_path": str(target.relative_to(out)), "wall_seconds": round(time.monotonic() - started, 6)})
        (folder / "run-record.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    faris_fixed = peer_compare.parse_faris_maintenance(inputs["faris"], case["faris_reference"]["design_id"], "fixed", "blanket")
    faris_computed = peer_compare.parse_faris_maintenance(inputs["faris"], case["faris_reference"]["design_id"], "computed", "blanket")
    result = {
        "schema": "faris-controlled-lifecycle-result/0.1.0",
        "case_id": case["case_id"],
        "interpretation": case["scope"],
        "authored_case_manifest_sha256": file_sha256(MANIFEST_PATH),
        "native_outputs": outputs,
        "comparison": evaluate(case, outputs["process"], outputs["bluemira"], faris_fixed, faris_computed),
    }
    target = out / "comparison.json"
    target.write_text(json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--process-python", required=True, help="Python in the separately pinned PROCESS 3.4.2 environment")
    parser.add_argument("--bluemira-python", required=True, help="Python in the separately pinned bluemira 2.14.0 environment")
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        result = run(args.process_python, args.bluemira_python, args.out.resolve())
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    print(json.dumps(result["comparison"], indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
