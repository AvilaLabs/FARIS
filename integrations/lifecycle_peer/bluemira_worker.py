#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run the public bluemira LifeCycle API in its separately pinned environment."""
from __future__ import annotations

import importlib.metadata
import json
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
    from bluemira.fuel_cycle.timeline_tools import (
        ExponentialAvailabilityStrategy,
        LogNormalAvailabilityStrategy,
        TruncNormAvailabilityStrategy,
    )

    kind = spec.get("kind")
    args = dict(spec.get("parameters", {}))
    if kind not in {"exponential", "lognormal", "truncated_normal"}:
        raise ValueError("availability strategy must be exponential, lognormal, or truncated_normal")
    if "rng_seed" in args and args["rng_seed"] != seed:
        raise ValueError("availability strategy RNG seed must match the recorded lifecycle RNG seed")
    args["rng_seed"] = seed
    cls = {"exponential": ExponentialAvailabilityStrategy,
           "lognormal": LogNormalAvailabilityStrategy,
           "truncated_normal": TruncNormAvailabilityStrategy}[kind]
    return cls(**args)


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
    lifecycle.write(str(output_path))
    native_metrics = {
        "version": actual,
        "calendar_years": json_safe(getattr(lifecycle, "T", None)),
        "full_power_years": json_safe(getattr(lifecycle, "fpy", None)),
        "overall_availability": json_safe(getattr(lifecycle, "A_global", None)),
        "planned_maintenance": json_safe(getattr(lifecycle, "total_planned_maintenance", None)),
        "ramp_time": json_safe(getattr(lifecycle, "total_ramptime", None)),
        "unplanned_downtime": json_safe(getattr(lifecycle, "t_unplanned_m", None)),
        "blanket_replacements": json_safe(getattr(lifecycle, "n_blk_replace", None)),
        "divertor_replacements": json_safe(getattr(lifecycle, "n_div_replace", None)),
        "rng_seed": seed_text_to_int(seed_text),
        "source_api": "bluemira.fuel_cycle.lifecycle.LifeCycle.make_timeline",
    }
    print(json.dumps(native_metrics, allow_nan=False))
    return 0


def seed_text_to_int(value: str) -> int:
    seed = int(value)
    if seed < 0:
        raise ValueError("RNG seed must be nonnegative")
    return seed


if __name__ == "__main__":
    raise SystemExit(main())
