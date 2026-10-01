#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Independent Decimal arithmetic control for identified FARIS run directories.

Uses only Python's standard library, without importing FARIS or OpenMC. Checks
unit definitions and torus/Cartesian volumes at 50-digit precision. A PASS
applies to arithmetic on supplied values, never reactor/nuclear-data validity.
"""
import argparse
from decimal import Decimal, getcontext
import hashlib
import json
from pathlib import Path

getcontext().prec = 50
PI = Decimal("3.1415926535897932384626433832795028841971693993751")
ELEMENTARY_CHARGE = Decimal("1.602176634e-19")
TOLERANCE = Decimal("1e-12")


def load(path):
    with path.open() as stream:
        return json.load(stream, parse_float=Decimal)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            value.update(block)
    return value.hexdigest()


def relative_error(actual, expected):
    error = abs(Decimal(actual) - expected) / (abs(expected) if expected else 1)
    if error >= TOLERANCE:
        raise ValueError(f"Arithmetic discrepancy {error} exceeds {TOLERANCE}")
    return error


def check(directory):
    inp = load(directory / "input.json")
    raw = load(directory / "solver/transport-artifact.json")
    record = load(directory / "run.json")
    request = inp["request"]
    rate = (Decimal(request["fusion_power_mw"]) * Decimal(1000000)
            / (Decimal(request["source"]["energy_per_reaction_ev"]) * ELEMENTARY_CHARGE)
            * Decimal(request["source"]["neutrons_per_reaction"]))
    volumes = {json.dumps(v["domain"], sort_keys=True):
               Decimal(v["value"]) * (Decimal("0.000001") if v["unit"] == "cubic_centimetre" else 1)
               for v in raw["volumes"]}
    results = {v["response_id"]: v for v in record["normalized"]["results"]}
    errors = []
    for tally in raw["tallies"]:
        factors = {"cm_per_source": Decimal(".01"), "particles_per_source": Decimal(1),
                   "events_per_source": Decimal(1), "ev_per_source": ELEMENTARY_CHARGE}
        scale = rate * factors[tally["unit"]]
        result = results[tally["response_id"]]
        volume = volumes[json.dumps(result["domain"], sort_keys=True)]
        for field, raw_field, divisor in [
            ("mean", "mean", volume), ("standard_error", "standard_error", volume),
            ("integrated_mean", "mean", 1), ("integrated_standard_error", "standard_error", 1),
        ]:
            expected = Decimal(tally[raw_field]) * scale / Decimal(divisor)
            errors.append(relative_error(result[field], expected))
    variant = next(v for v in inp["manifest"]["variants"] if v["id"] == request["variant_id"])
    major = Decimal(inp["manifest"]["major_radius_m"])
    outer = Decimal(variant["components"][-1]["outer_minor_radius_m"])
    mesh = inp["mesh"]
    bin_volume = Decimal(1)
    for axis in range(3):
        bin_volume *= ((Decimal(mesh["upper_right_m"][axis]) - Decimal(mesh["lower_left_m"][axis]))
                       / Decimal(mesh["dimensions"][axis]))
    shells = {c["id"]: 2 * PI * PI * major * (Decimal(c["outer_minor_radius_m"])**2
              - Decimal(c["inner_minor_radius_m"])**2) for c in variant["components"]}
    volume_errors = []
    for item in raw["volumes"]:
        domain = item["domain"]
        expected = (shells[domain["component_id"]] if domain["kind"] == "component"
                    else bin_volume if domain["kind"] == "mesh" else 2 * PI * PI * major * outer**2)
        factor = Decimal(".000001") if item["unit"] == "cubic_centimetre" else Decimal(1)
        volume_errors.append(relative_error(Decimal(item["value"]) * factor, expected))
    magnet = results["magnets-flux"]
    return {
        "run_directory": str(directory), "variant_id": request["variant_id"],
        "histories": raw["histories"], "seed": inp["sampling"]["seed"],
        "threads": inp["sampling"]["threads"],
        "elapsed_seconds": float(record["execution"]["elapsed_seconds"]),
        "run_record_sha256": digest(directory / "run.json"),
        "raw_artifact_sha256": digest(directory / "solver/transport-artifact.json"),
        "input_sha256": digest(directory / "input.json"),
        "independent_source_neutron_rate_per_s": str(rate),
        "checked_tallies": len(raw["tallies"]), "checked_values": len(errors),
        "max_relative_normalization_difference": str(max(errors)),
        "max_relative_volume_difference": str(max(volume_errors)),
        "magnet_flux_relative_standard_error":
            str(Decimal(magnet["standard_error"]) / Decimal(magnet["mean"])) if magnet["mean"] else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, action="append", required=True, help="Run directory; repeat")
    parser.add_argument("--output", type=Path, help="New output file; otherwise stdout")
    args = parser.parse_args()
    report = {
        "schema_version": "faris-independent-transport-arithmetic/v0.1",
        "arithmetic_control": "PASS", "scientific_qualification": "NOT_EVALUATED",
        "scope": "50-digit Decimal source-strength, unit conversions and torus/Cartesian volume arithmetic only; relative tolerance 1e-12. Does not authenticate origins or qualify reactor predictions.",
        "runs": [check(directory) for directory in args.run],
    }
    encoded = json.dumps(report, indent=2) + "\n"
    if args.output:
        with args.output.open("x") as stream:
            stream.write(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
