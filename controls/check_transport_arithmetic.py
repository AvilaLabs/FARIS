#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Independent Decimal arithmetic control for identified FARIS run directories.

Uses only Python's standard library, without importing FARIS or OpenMC. Checks
unit definitions and torus/Cartesian volumes at 50-digit precision, including
the exact volumes of the toroidal magnet regions (inboard half, outboard half
without the port sector, port sector) and their normalization. A PASS applies
to arithmetic on supplied values, never reactor/nuclear-data validity.
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


def domain_key(domain):
    # Floats are parsed as Decimal; their text is stable on both sides.
    return json.dumps(domain, sort_keys=True, default=str)


def region_volume_m3(major, inner, outer, region):
    """Exact full-torus volume of a toroidal-shell region, 50 digits.

    With R = R0 + r cos(theta) and dV = R r dr dtheta dphi, a half annulus
    integrates to R0 pi (b^2 - a^2)/2 +/- 2 (b^3 - a^3)/3 (plus outboard,
    minus inboard); multiply by the toroidal extent: 2 pi for a half, 2 w for
    the port sector of half width w, 2 pi - 2 w for the outboard half without it.
    """
    half = major * PI * (outer**2 - inner**2) / 2
    skew = 2 * (outer**3 - inner**3) / 3
    kind = region["kind"]
    if kind == "inboard_half":
        return 2 * PI * (half - skew)
    if kind == "port_sector":
        return 2 * Decimal(region["half_width_rad"]) * (half + skew)
    width = region.get("excluding_sector_half_width_rad")
    return (2 * PI - (2 * Decimal(width) if width is not None else 0)) * (half + skew)


def relative_error(actual, expected):
    error = abs(Decimal(actual) - expected) / (abs(expected) if expected else 1)
    if error >= TOLERANCE:
        raise ValueError(f"Arithmetic discrepancy {error} exceeds {TOLERANCE}")
    return error


def penetration_removes(inp, component_id):
    penetration = inp["manifest"].get("penetration")
    return penetration is not None and component_id in penetration["affected_component_ids"]


def check_regions(inp, raw, variant, major, volumes, results):
    """Region volumes and region normalization, independent of the worker and Rust.

    1. Without a port a region volume equals its exact analytic volume (1e-12).
       With a port the stochastic volume must lie below the analytic one, by no
       more than the port box volume, and carry a nonzero error where removed.
    2. The inboard, outboard-without-port-sector and port-sector volumes sum to
       the component's own volume (they partition the same points exactly).
    3. The same regions' raw fast-flux track lengths sum to the whole-component
       fast flux (the regions partition the cell), and no fast flux exceeds the
       energy-integrated flux of its component.
    Returns None when the run has no region responses.
    """
    entries = [(item, item["domain"]) for item in raw["volumes"] if item["domain"]["kind"] == "component_region"]
    if not entries:
        return None
    components = {c["id"]: c for c in variant["components"]}
    port = inp["manifest"].get("penetration")
    box = None
    if port is not None:
        low, high = port["bounds_m"]["minimum_xyz_m"], port["bounds_m"]["maximum_xyz_m"]
        box = Decimal(1)
        for axis in range(3):
            box *= Decimal(high[axis]) - Decimal(low[axis])
    to_m3 = lambda item: Decimal(item["value"]) * (Decimal(".000001") if item["unit"] == "cubic_centimetre" else 1)
    by_component = {}
    report = {}
    for item, domain in entries:
        component = components[domain["component_id"]]
        analytic = region_volume_m3(major, Decimal(component["inner_minor_radius_m"]),
                                    Decimal(component["outer_minor_radius_m"]), domain["region"])
        value = to_m3(item)
        if penetration_removes(inp, domain["component_id"]):
            removed = analytic - value
            if removed < -TOLERANCE * analytic or removed > box:
                raise ValueError(f"region volume {domain_key(domain)} is outside [analytic - port box, analytic]")
            difference = removed
        else:
            difference = relative_error(value, analytic)
        by_component.setdefault(domain["component_id"], []).append((domain, value))
        report[domain_key(domain)] = {
            "analytic_full_volume_m3": str(analytic), "artifact_volume_m3": str(value),
            "removed_or_difference": str(difference),
            "standard_error_m3": str(to_m3({"value": item.get("standard_error", 0), "unit": item["unit"]})),
        }
    partition_errors = []
    for component_id, items in by_component.items():
        kinds = {item[0]["region"]["kind"] + str(item[0]["region"].get("excluding_sector_half_width_rad") is not None)
                 for item in items}
        if kinds != {"inboard_halfFalse", "outboard_halfTrue", "port_sectorFalse"}:
            continue  # not the exclusive three-way partition
        component_volume = next(volumes[k] for k in volumes if json.loads(k) == {"kind": "component", "component_id": component_id})
        partition_errors.append(relative_error(sum(v for _, v in items), component_volume))
    # Raw fast-flux additivity over the partition, per component.
    raw_mean = {t["response_id"]: Decimal(t["mean"]) for t in raw["tallies"]}
    additive_errors = []
    for component_id, items in by_component.items():
        region_ids = [rid for rid, r in results.items()
                      if r["domain"]["kind"] == "component_region" and r["domain"]["component_id"] == component_id]
        whole = [rid for rid, r in results.items()
                 if r["domain"] == {"kind": "component", "component_id": component_id}
                 and r["score"]["kind"] == "flux_above"]
        if len(region_ids) == 3 and len(whole) == 1:
            additive_errors.append(relative_error(sum(raw_mean[i] for i in region_ids), raw_mean[whole[0]]))
        total = raw_mean.get(f"{component_id}-flux")
        if total is not None and whole and raw_mean[whole[0]] > total * (1 + TOLERANCE):
            raise ValueError(f"{component_id} fast flux exceeds its energy-integrated flux")
    return {
        "regions": report,
        "max_partition_volume_difference": str(max(partition_errors)) if partition_errors else None,
        "max_fast_flux_additivity_difference": str(max(additive_errors)) if additive_errors else None,
    }


def check(directory):
    inp = load(directory / "input.json")
    raw = load(directory / "solver/transport-artifact.json")
    record = load(directory / "run.json")
    request = inp["request"]
    rate = (Decimal(request["fusion_power_mw"]) * Decimal(1000000)
            / (Decimal(request["source"]["energy_per_reaction_ev"]) * ELEMENTARY_CHARGE)
            * Decimal(request["source"]["neutrons_per_reaction"]))
    volumes = {domain_key(v["domain"]):
               Decimal(v["value"]) * (Decimal("0.000001") if v["unit"] == "cubic_centimetre" else 1)
               for v in raw["volumes"]}
    results = {v["response_id"]: v for v in record["normalized"]["results"]}
    errors = []
    for tally in raw["tallies"]:
        factors = {"cm_per_source": Decimal(".01"), "particles_per_source": Decimal(1),
                   "events_per_source": Decimal(1), "ev_per_source": ELEMENTARY_CHARGE}
        scale = rate * factors[tally["unit"]]
        result = results[tally["response_id"]]
        volume = volumes[domain_key(result["domain"])]
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
        factor = Decimal(".000001") if item["unit"] == "cubic_centimetre" else Decimal(1)
        if domain["kind"] == "component_region":
            continue  # checked below: exact only for the unperforated shell
        expected = (shells[domain["component_id"]] if domain["kind"] == "component"
                    else bin_volume if domain["kind"] == "mesh" else 2 * PI * PI * major * outer**2)
        volume_errors.append(relative_error(Decimal(item["value"]) * factor, expected))
    regions = check_regions(inp, raw, variant, major, volumes, results)
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
        "regions": regions,
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
