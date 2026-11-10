#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run the public bluemira LifeCycle API in its separately pinned environment."""
from __future__ import annotations

import importlib.metadata
import json
import math
from pathlib import Path
import sys
from typing import Any


def json_safe(value: Any) -> Any:
    if hasattr(value, "to_dict"):
        return json_safe(value.to_dict())
    if hasattr(value, "tolist"):
        return json_safe(value.tolist())
    if hasattr(value, "item"):
        return value.item()
    if isinstance(value, dict):
        return {str(k): json_safe(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [json_safe(v) for v in value]
    return value


class NativeTimelineJSONEncoder(json.JSONEncoder):
    """Serialize the native timeline without changing its numerical values.

    Bluemira's v2.14.0 ``NumpyJSONEncoder`` handles ndarray values, but its
    fallback does not handle NumPy scalar values (for example ``np.int64`` in
    ``blanket_change_index``). ``LifeCycle.write`` forwards ``cls`` to its
    documented ``json_writer`` boundary, so normalize only NumPy containers at
    serialization time and leave the upstream timeline/model untouched.
    """

    def default(self, value: Any):
        import numpy as np

        if isinstance(value, np.ndarray):
            return value.tolist()
        if isinstance(value, np.generic):
            return value.item()
        return super().default(value)


def make_learning(spec: dict[str, Any]):
    from bluemira.fuel_cycle.timeline_tools import (
        GompertzLearningStrategy,
        UniformLearningStrategy,
        UserSpecifiedLearningStrategy,
    )

    kind = spec.get("kind")
    args = spec.get("parameters", {})
    if kind == "uniform" and not args:
        return UniformLearningStrategy()
    if kind == "user_specified":
        return UserSpecifiedLearningStrategy(args["operational_availabilities"])
    if kind == "gompertz":
        return GompertzLearningStrategy(**args)
    raise ValueError("learning strategy must be uniform, user_specified, or gompertz")


def make_availability(spec: dict[str, Any], seed: int):
    if spec.get("kind") == "zero_unplanned":
        if set(spec) != {"kind", "parameters"} or spec["parameters"] != {}:
            raise ValueError("zero_unplanned availability takes no parameters")
        # LifeCycle's documented extension point is OperationalAvailabilityStrategy
        # with generate_distribution(n, integral). Keep its actual maintenance and
        # timeline loop; this strategy only sets the unplanned distribution to zero.
        import numpy as np
        from bluemira.fuel_cycle.timeline_tools import LogNormalAvailabilityStrategy

        class ZeroUnplannedAvailabilityStrategy(LogNormalAvailabilityStrategy):
            def __init__(self):
                # Initialize bluemira's native strategy state. sigma is bypassed by
                # this deterministic override; it is not a claimed outage statistic.
                super().__init__(sigma=1.0, rng_seed=seed)

            def generate_distribution(self, n: int, integral: float):
                if n < 0 or integral < -1e-12:
                    raise ValueError("invalid unplanned distribution request")
                if abs(integral) > 1e-9:
                    raise ValueError("zero-unplanned control is incompatible with requested positive unplanned downtime")
                return np.zeros(n)

        return ZeroUnplannedAvailabilityStrategy()

    from bluemira.fuel_cycle.timeline_tools import (
        ExponentialAvailabilityStrategy,
        LogNormalAvailabilityStrategy,
        TruncNormAvailabilityStrategy,
    )

    kind = spec.get("kind")
    args = dict(spec.get("parameters", {}))
    if kind not in {"exponential", "lognormal", "truncated_normal"}:
        raise ValueError("availability strategy must be zero_unplanned, exponential, lognormal, or truncated_normal")
    if "rng_seed" in args and args["rng_seed"] != seed:
        raise ValueError("availability strategy RNG seed must match the recorded lifecycle RNG seed")
    args["rng_seed"] = seed
    cls = {"exponential": ExponentialAvailabilityStrategy,
           "lognormal": LogNormalAvailabilityStrategy,
           "truncated_normal": TruncNormAvailabilityStrategy}[kind]
    return cls(**args)


def native_timeline_metrics(lifecycle: Any, actual: str, seed: int) -> dict[str, Any]:
    """Read actual metrics from the pinned LifeCycle's returned native Timeline."""
    timeline = lifecycle.T
    calendar_years = json_safe(getattr(timeline, "plant_life", None))
    ft = getattr(timeline, "ft", None)
    full_power_years = json_safe(ft[-1] if ft is not None and len(ft) else None)
    effective_availability = None
    if (isinstance(calendar_years, (int, float)) and math.isfinite(calendar_years) and calendar_years > 0
            and isinstance(full_power_years, (int, float)) and math.isfinite(full_power_years)):
        effective_availability = float(full_power_years) / float(calendar_years)
    return {
        "version": actual,
        "calendar_years": calendar_years,
        "full_power_years": full_power_years,
        "overall_availability": effective_availability,
        "operational_load_factor_input": json_safe(getattr(lifecycle.params, "A_global", None)),
        "planned_maintenance": json_safe(getattr(lifecycle, "total_planned_maintenance", None)),
        "ramp_time": json_safe(getattr(lifecycle, "total_ramptime", None)),
        "unplanned_downtime": json_safe(getattr(lifecycle, "t_unplanned_m", None)),
        "blanket_replacements": json_safe(getattr(lifecycle, "n_blk_replace", None)),
        "divertor_replacements": json_safe(getattr(lifecycle, "n_div_replace", None)),
        "continuous_damage_budget_fpy_input": json_safe(getattr(lifecycle, "fpy", None)),
        "native_timeline_maintenance_phase_names": [
            name for name in getattr(lifecycle, "phase_names", []) if "Phase M" in name
        ],
        "native_timeline_operation_phase_names": [
            name for name in getattr(lifecycle, "phase_names", []) if "Phase P" in name
        ],
        "timeline_metrics_basis": "actual pulse-resolved bluemira Timeline.plant_life and Timeline.ft[-1]; not the continuous LifeCycle.fpy budget",
        "rng_seed": seed,
        "source_api": "bluemira.fuel_cycle.lifecycle.LifeCycle.make_timeline",
    }


def main() -> int:
    if len(sys.argv) != 5:
        raise SystemExit("usage: bluemira_worker.py CONFIG.json OUTPUT.json RNG_SEED EXPECTED_VERSION")
    config_path, output_path, seed_text, expected = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    actual = importlib.metadata.version("bluemira")
    if actual != expected:
        raise RuntimeError(f"bluemira version mismatch: expected {expected}, got {actual}")
    config = json.loads(config_path.read_text(encoding="utf-8"))
    required = {"lifecycle_config", "learning_strategy", "availability_strategy", "inputs", "provenance"}
    if not isinstance(config, dict) or set(config) != required:
        raise ValueError(f"LifeCycle config must contain exactly {sorted(required)}")
    if not isinstance(config["lifecycle_config"], dict) or not config["lifecycle_config"]:
        raise ValueError("lifecycle_config must be explicitly supplied")
    if not isinstance(config["inputs"], dict):
        raise ValueError("LifeCycle inputs must be an explicit object (empty only when justified)")
    if not isinstance(config["provenance"], dict) or set(config["provenance"]) != set(config["lifecycle_config"]):
        raise ValueError("every LifeCycle config value needs its own unit and provenance")
    for key, record in config["provenance"].items():
        if not isinstance(record, dict) or set(record) != {"unit", "kind", "reference"} or not all(str(record[k]).strip() for k in record):
            raise ValueError(f"lifecycle_config.{key} provenance must include unit, kind and reference")

    from bluemira.fuel_cycle.lifecycle import LifeCycle

    lifecycle = LifeCycle(config["lifecycle_config"], make_learning(config["learning_strategy"]),
                          make_availability(config["availability_strategy"], seed_text_to_int(seed_text)),
                          config["inputs"], rng_seed=seed_text_to_int(seed_text))
    timeline = lifecycle.make_timeline()
    lifecycle.write(str(output_path), cls=NativeTimelineJSONEncoder)
    native_metrics = native_timeline_metrics(lifecycle, actual, seed_text_to_int(seed_text))
    print(json.dumps(native_metrics, allow_nan=False))
    return 0


def seed_text_to_int(value: str) -> int:
    seed = int(value)
    if seed < 0:
        raise ValueError("RNG seed must be nonnegative")
    return seed


if __name__ == "__main__":
    raise SystemExit(main())
