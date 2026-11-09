#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run PROCESS's native Ward--Taylor availability model on authored submodel inputs.

This deliberately does not invoke PROCESS's whole-plant solver. The IFE flag bypasses
the physics-derived component-life calculation so the controlled FPY/duration inputs
are consumed directly by the documented Availability model.
"""
from __future__ import annotations

import importlib.metadata
import json
from pathlib import Path
import sys


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: process_availability_worker.py CONFIG.json OUTPUT.json EXPECTED_VERSION")
    config_path, output_path, expected = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    actual = importlib.metadata.version("process")
    if actual != expected:
        raise RuntimeError(f"PROCESS version mismatch: expected {expected}, got {actual}")
    config = json.loads(config_path.read_text(encoding="utf-8"))
    required = {"plant_horizon_years", "blanket_lifetime_fpy", "divertor_lifetime_fpy",
                "blanket_replacement_days", "divertor_replacement_days", "combined_replacement_days",
                "pulse_burn_seconds", "pulse_total_seconds", "net_electric_power_mw",
                "unplanned_unavailability", "provenance"}
    if not isinstance(config, dict) or set(config) != required:
        raise ValueError(f"PROCESS submodel config must contain exactly {sorted(required)}")
    numbers = required - {"unplanned_unavailability", "provenance"}
    if any(isinstance(config[k], bool) or not isinstance(config[k], (int, float)) for k in numbers):
        raise ValueError("PROCESS controlled-case numeric inputs must be finite numbers")
    if any(not __import__("math").isfinite(float(config[k])) for k in numbers):
        raise ValueError("PROCESS controlled-case numeric inputs must be finite")
    if config["plant_horizon_years"] <= 0 or config["blanket_lifetime_fpy"] <= 0 or config["divertor_lifetime_fpy"] <= 0:
        raise ValueError("PROCESS horizon and component lives must be positive")
    if config["blanket_lifetime_fpy"] > config["plant_horizon_years"] or config["divertor_lifetime_fpy"] > config["plant_horizon_years"]:
        raise ValueError("component lifetime may not exceed the explicit plant horizon in this harness")
    if config["pulse_burn_seconds"] <= 0 or config["pulse_total_seconds"] < config["pulse_burn_seconds"]:
        raise ValueError("pulse total must be at least a positive burn duration")
    if not isinstance(config["unplanned_unavailability"], dict) or set(config["unplanned_unavailability"]) != {
        "uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"
    }:
        raise ValueError("all seven native PROCESS unplanned fractions must be explicit")
    for key, value in config["unplanned_unavailability"].items():
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 <= value < 1:
            raise ValueError(f"{key} must be an explicit fraction in [0, 1)")
    if not isinstance(config["provenance"], dict) or set(config["provenance"]) != required - {"provenance"}:
        raise ValueError("every PROCESS input and the unplanned set need provenance")

    from process.core.model import DataStructure
    from process.models.availability import Availability, AvailabilityModel

    data = DataStructure()
    data.ife.ife = 1
    data.costs.i_plant_availability = AvailabilityModel.WARD_TAYLOR.value
    data.costs.life_plant = float(config["plant_horizon_years"])
    data.fwbs.life_blkt_fpy = float(config["blanket_lifetime_fpy"])
    data.costs.life_div_fpy = float(config["divertor_lifetime_fpy"])
    seconds_per_year = 365.25 * 24 * 60 * 60
    data.costs.t_blkt_replace_yrs = float(config["blanket_replacement_days"]) / 365.25
    data.costs.t_div_replace_yrs = float(config["divertor_replacement_days"]) / 365.25
    data.costs.tcomrepl = float(config["combined_replacement_days"]) / 365.25
    for key, value in config["unplanned_unavailability"].items():
        setattr(data.costs, key, float(value))
    data.times.t_plant_pulse_burn = float(config["pulse_burn_seconds"])
    data.times.t_plant_pulse_total = float(config["pulse_total_seconds"])

    model = Availability()
    model.data = data
    model.run(output=False)
    metrics = {
        "version": actual,
        "source_api": "process.models.availability.Availability.run(WARD_TAYLOR)",
        "submodel_scope": "Ward--Taylor availability only; no PROCESS reactor physics or optimization run",
        "physics_lifetime_bypass": "data.ife.ife=1; component full-power lives supplied as explicit verification assumptions",
        "plant_horizon_years_input": float(config["plant_horizon_years"]),
        "blanket_lifetime_fpy_input": float(config["blanket_lifetime_fpy"]),
        "divertor_lifetime_fpy_input": float(config["divertor_lifetime_fpy"]),
        "replacement_duration_years": {
            "blanket": float(config["blanket_replacement_days"]) / 365.25,
            "divertor": float(config["divertor_replacement_days"]) / 365.25,
            "combined": float(config["combined_replacement_days"]) / 365.25,
        },
        "unplanned_unavailability": config["unplanned_unavailability"],
        "plant_availability_fraction": float(data.costs.f_t_plant_available),
        "capacity_factor": float(data.costs.cpfact),
        "full_power_years_if_horizon_is_calendar": float(data.costs.cpfact) * float(config["plant_horizon_years"]),
        "net_electric_power_mw_authored_reference": float(config["net_electric_power_mw"]),
        "energy_basis_note": "PROCESS Availability model does not calculate electrical power or lifetime electricity; net MW is a shared authored normalization input.",
        "inputs_provenance": config["provenance"],
    }
    output_path.write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(metrics, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
