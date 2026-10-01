#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Generate the port-case blanket/shield allocation sweep from the frozen port inputs.

The sweep changes only the blanket and shield thicknesses inside the fixed
0.90 m blanket+shield allocation of the cold-data port reference. Materials,
source, penetration, mesh and every other layer are copied byte-for-byte in
meaning from the reference port scenario and its reference physics case.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_SCENARIO = ROOT / "scenarios/arc-inspired/cold-reference-port.scenario.json"
SOURCE_PHYSICS = ROOT / "scenarios/arc-inspired/cold-reference-port.reference.physics.json"
OUTPUT_DIR = ROOT / "scenarios/arc-inspired/allocation-sweep"
ALLOCATION_M = 0.90
BLANKET_POINTS_M = (0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60)


def variant_id(blanket_m: float) -> str:
    return f"blanket-{round(blanket_m * 100):03d}cm"


def build_scenario() -> dict:
    source = json.loads(SOURCE_SCENARIO.read_text())
    template = next(v for v in source["variants"] if v["id"] == "reference")
    scenario = copy.deepcopy(source)
    scenario["id"] = "arc-cold-reference-port-allocation-sweep-001"
    scenario["title"] = "Blanket/shield allocation sweep with finite outboard port"
    scenario["description"] = (
        "Seven blanket/shield splits of the same 0.90 m allocation in the cold-data "
        "port reference; every other layer, material, the source and the port are "
        "unchanged. Conditional numerical sweep; not an ARC reproduction."
    )
    variants = []
    for blanket in BLANKET_POINTS_M:
        shield = round(ALLOCATION_M - blanket, 2)
        variant = copy.deepcopy(template)
        variant["id"] = variant_id(blanket)
        variant["label"] = f"Blanket {blanket:.2f} m · shield {shield:.2f} m"
        for layer in variant["layers"]:
            if layer["id"] == "blanket":
                layer["thickness_m"] = blanket
            elif layer["id"] == "shield":
                layer["thickness_m"] = shield
        variants.append(variant)
    scenario["variants"] = variants
    scenario["assumptions"] = list(source["assumptions"]) + [
        "Allocation sweep: blanket thickness 0.30-0.60 m in 0.05 m steps with the "
        "shield taking the remainder of a fixed 0.90 m; all other inputs follow the "
        "port reference scenario."
    ]
    return scenario


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify committed files are current")
    args = parser.parse_args()
    scenario_bytes = (json.dumps(build_scenario(), indent=2) + "\n").encode()
    scenario_sha = hashlib.sha256(scenario_bytes).hexdigest()
    files = {OUTPUT_DIR / "scenario.json": scenario_bytes}
    physics_source = json.loads(SOURCE_PHYSICS.read_text())
    for blanket in BLANKET_POINTS_M:
        physics = copy.deepcopy(physics_source)
        physics["id"] = f"allocation-sweep-{variant_id(blanket)}-physics-001"
        physics["scenario_id"] = "arc-cold-reference-port-allocation-sweep-001"
        physics["scenario_sha256"] = scenario_sha
        physics["variant_id"] = variant_id(blanket)
        files[OUTPUT_DIR / f"{variant_id(blanket)}.physics.json"] = (
            json.dumps(physics, indent=2) + "\n"
        ).encode()
    if args.check:
        stale = [p for p, b in files.items() if not p.exists() or p.read_bytes() != b]
        for path in stale:
            print(f"stale: {path.relative_to(ROOT)}")
        return 1 if stale else 0
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    for path, data in files.items():
        path.write_bytes(data)
        print(f"wrote {path.relative_to(ROOT)}")
    print(f"scenario sha256 {scenario_sha}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
