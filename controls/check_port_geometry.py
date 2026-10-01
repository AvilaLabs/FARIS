#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Independent adaptive-quadrature check for one finite FARIS outboard port.

SciPy integrates the toroidal shell/box intersection independently from the
Rust midpoint rule and OpenMC point-classification audit. The check compares
geometry estimates only; it cannot qualify materials, nuclear data, or design
performance.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path

from scipy.integrate import dblquad

QUADRATURE_ABS_TOL_M3 = 1.0e-10
QUADRATURE_REL_TOL = 1.0e-9
JOINT_SIGMA_TOLERANCE = 5.0
CM3_TO_M3 = 1.0e-6


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            value.update(block)
    return value.hexdigest()


def read_json(path):
    with path.open() as stream:
        return json.load(stream)


def solid_torus_x_length(major, minor, y, z, xmin, xmax):
    if abs(y) >= minor:
        return 0.0
    half = math.sqrt(minor * minor - y * y)
    radial_min = major - half
    radial_max = major + half
    x0 = max(xmin, math.sqrt(max(0.0, radial_min * radial_min - z * z)))
    x1 = min(xmax, math.sqrt(max(0.0, radial_max * radial_max - z * z)))
    return max(0.0, x1 - x0)


def shell_box_volume(major, inner, outer, bounds):
    lower, upper = bounds["minimum_xyz_m"], bounds["maximum_xyz_m"]
    xmin, xmax = lower[0], upper[0]

    def area(z, y):
        return max(
            0.0,
            solid_torus_x_length(major, outer, y, z, xmin, xmax)
            - solid_torus_x_length(major, inner, y, z, xmin, xmax),
        )

    volume, quadrature_error = dblquad(
        area,
        lower[2],
        upper[2],
        lambda _z: lower[1],
        lambda _z: upper[1],
        epsabs=QUADRATURE_ABS_TOL_M3,
        epsrel=QUADRATURE_REL_TOL,
    )
    return volume, quadrature_error


def check(scenario_path, variant_id, worker_path=None, artifact_path=None, run_path=None):
    scenario_bytes = scenario_path.read_bytes()
    scenario = json.loads(scenario_bytes)
    geometry = scenario["geometry"]
    feature = scenario.get("penetration")
    if not feature or feature.get("kind") != "outboard_rectangular_prism":
        raise ValueError("scenario must define an outboard_rectangular_prism penetration")
    bounds = feature["bounds_m"]
    minimum, maximum = bounds["minimum_xyz_m"], bounds["maximum_xyz_m"]
    if len(minimum) != 3 or len(maximum) != 3 or any(
        not math.isfinite(x) for x in minimum + maximum
    ) or any(minimum[i] >= maximum[i] for i in range(3)):
        raise ValueError("port bounds must be finite increasing XYZ coordinates")
    major = geometry["major_radius_m"]
    plasma_outer = geometry["plasma_minor_radius_m"]
    radial_inner = plasma_outer + geometry["plasma_to_first_wall_gap_m"]
    radial_outer = radial_inner + geometry["radial_build_m"]
    if minimum[0] - major <= plasma_outer:
        raise ValueError("port violates the plasma source support clearance")
    if minimum[1] > 0 or maximum[1] < 0 or minimum[2] > 0 or maximum[2] < 0:
        raise ValueError("port must cross the outboard Y=Z=0 centerline")
    if maximum[0] <= major + radial_outer:
        raise ValueError("port does not exit the outer radial envelope")

    variant = next((v for v in scenario["variants"] if v["id"] == variant_id), None)
    if variant is None:
        raise ValueError(f"unknown variant {variant_id}")
    components = []
    radius = radial_inner
    for layer in variant["layers"]:
        outer = radius + layer["thickness_m"]
        intersection, quad_error = shell_box_volume(major, radius, outer, bounds)
        full = 2.0 * math.pi**2 * major * (outer**2 - radius**2)
        components.append({
            "component_id": layer["id"],
            "inner_minor_radius_m": radius,
            "outer_minor_radius_m": outer,
            "unperforated_volume_m3": full,
            "independent_intersection_volume_m3": intersection,
            "scipy_reported_quadrature_error_m3": quad_error,
            "remaining_volume_from_quadrature_m3": full - intersection,
        })
        radius = outer

    affected = [c["component_id"] for c in components if c["independent_intersection_volume_m3"] > 1e-12]
    if affected != feature["affected_component_ids"]:
        raise ValueError(f"feature component set/order mismatch: expected {affected}")
    total, total_error = shell_box_volume(major, radial_inner, radial_outer, bounds)
    sum_intersections = sum(c["independent_intersection_volume_m3"] for c in components)
    closure_error = abs(total - sum_intersections)
    closure_tolerance = 10.0 * (total_error + sum(c["scipy_reported_quadrature_error_m3"] for c in components)) + 1e-9
    if closure_error > closure_tolerance:
        raise ValueError("component intersection volumes fail radial partition closure")

    report = {
        "schema_version": "faris-independent-port-volume-check/v0.1",
        "geometry_check": "PASS",
        "transport_volume_check": "NOT_EVALUATED",
        "scientific_qualification": "NOT_EVALUATED",
        "scenario_id": scenario["id"],
        "scenario_sha256": hashlib.sha256(scenario_bytes).hexdigest(),
        "variant_id": variant_id,
        "penetration_id": feature["id"],
        "bounds_semantics": "global right-handed axis-aligned rectangular prism; XYZ metres",
        "source_support_clearance_m": minimum[0] - major - plasma_outer,
        "source_containment": "PASS: rectangular port is disjoint from the uniform plasma-torus source support",
        "radial_partition_closure_m": radius - radial_outer,
        "component_intersection_sum_m3": sum_intersections,
        "single_union_intersection_m3": total,
        "intersection_partition_closure_error_m3": closure_error,
        "intersection_partition_closure_tolerance_m3": closure_tolerance,
        "quadrature": {
            "method": "scipy.integrate.dblquad adaptive QUADPACK; analytic x-length, adaptive integration over global YZ face",
            "absolute_tolerance_m3": QUADRATURE_ABS_TOL_M3,
            "relative_tolerance": QUADRATURE_REL_TOL,
            "reported_error_is_rigorous_bound": False,
        },
        "components": components,
    }
    if worker_path or artifact_path:
        if not worker_path or not artifact_path or not run_path:
            raise ValueError("--worker-result, --transport-artifact, and --run-record must be supplied together")
        worker = read_json(worker_path)
        artifact = read_json(artifact_path)
        run = read_json(run_path)
        run_root = run_path.parent
        if worker_path.resolve() != (run_root / "solver/worker-result.json").resolve():
            raise ValueError("worker-result must come from the run directory's solver record")
        if artifact_path.resolve() != (run_root / "solver/transport-artifact.json").resolve():
            raise ValueError("transport artifact must come from the run directory")
        if run.get("scenario_sha256") != hashlib.sha256(scenario_bytes).hexdigest():
            raise ValueError("run record does not bind the supplied exact scenario bytes")
        if run.get("variant_id") != variant_id:
            raise ValueError("run record variant does not match the requested variant")
        if run.get("raw_artifact_sha256") != digest(artifact_path):
            raise ValueError("run record does not bind the supplied transport artifact")
        input_path = run_root / "input.json"
        if not input_path.is_file() or run.get("input_sha256") != digest(input_path):
            raise ValueError("run record does not bind the adjacent input snapshot")
        run_input = read_json(input_path)
        if run_input.get("manifest", {}).get("source_sha256") != hashlib.sha256(scenario_bytes).hexdigest():
            raise ValueError("input manifest does not bind the supplied exact scenario bytes")
        input_manifest = run_input["manifest"]
        if input_manifest.get("geometry_volume_status") != "PENETRATION_ESTIMATE_NOT_INDEPENDENTLY_VALIDATED":
            raise ValueError("input manifest lacks the expected unvalidated-volume status")
        manifest_variant = next(v for v in input_manifest["variants"] if v["id"] == variant_id)
        rust_estimates = {c["id"]: c for c in manifest_variant["components"]}
        midpoint_checks = []
        for component in components:
            cid = component["component_id"]
            estimate = rust_estimates[cid]["penetration_intersection_estimate"]
            delta = abs(component["independent_intersection_volume_m3"] - estimate["volume_m3"])
            tolerance = 5.0 * estimate["refinement_delta_m3"] + 10.0 * component["scipy_reported_quadrature_error_m3"] + 1e-12
            passed = delta <= tolerance
            midpoint_checks.append({
                "component_id": cid,
                "rust_midpoint_volume_m3": estimate["volume_m3"],
                "rust_refinement_delta_m3": estimate["refinement_delta_m3"],
                "scipy_adaptive_volume_m3": component["independent_intersection_volume_m3"],
                "difference_m3": delta,
                "response_specific_tolerance_m3": tolerance,
                "status": "PASS" if passed else "FAIL",
            })
        if any(c["status"] != "PASS" for c in midpoint_checks):
            raise ValueError("Rust midpoint and independent adaptive quadrature disagree")
        report["rust_midpoint_validation"] = midpoint_checks
        audit = worker["penetration_volume_audit"]
        expected_scenario_sha = hashlib.sha256(scenario_bytes).hexdigest()
        if audit.get("scenario_sha256") != expected_scenario_sha or audit.get("variant_id") != variant_id:
            raise ValueError("worker volume audit is not bound to this scenario and variant")
        if worker.get("transport_artifact_sha256") != digest(artifact_path):
            raise ValueError("worker result does not bind the supplied raw transport artifact")
        if audit.get("fractional_volume_standard_errors_are_binomial") is not True or audit.get("independent_of_Rust_midpoint_quadrature") is not True:
            raise ValueError("worker volume audit does not declare the expected independent sampling method")
        if audit.get("not_a_physical_validation") is not True:
            raise ValueError("worker must explicitly limit its volume audit to geometry")
        if audit.get("samples", 0) <= 0:
            raise ValueError("worker volume audit has no samples")
        raw_volumes = {
            item["domain"]["component_id"]: item
            for item in artifact["volumes"]
            if item["domain"]["kind"] == "component"
        }
        audit_estimates = audit["intersection_estimates_m3"]
        audit_se = audit["intersection_standard_errors_m3"]
        validations = []
        for component in components:
            cid = component["component_id"]
            if cid not in audit_estimates or cid not in audit_se or cid not in raw_volumes:
                raise ValueError(f"worker omitted volume data for {cid}")
            independent = component["independent_intersection_volume_m3"]
            scipy_error = component["scipy_reported_quadrature_error_m3"]
            openmc_intersection = audit_estimates[cid]
            openmc_intersection_se = audit_se[cid]
            intersection_tolerance = JOINT_SIGMA_TOLERANCE * math.hypot(scipy_error, openmc_intersection_se) + 1e-9
            intersection_delta = abs(independent - openmc_intersection)
            volume = raw_volumes[cid]
            if volume["unit"] != "cubic_centimetre":
                raise ValueError("port component volumes must be exported in cubic centimetres")
            openmc_remaining = volume["value"] * CM3_TO_M3
            openmc_remaining_se = volume["standard_error"] * CM3_TO_M3
            expected_remaining = component["remaining_volume_from_quadrature_m3"]
            remaining_tolerance = JOINT_SIGMA_TOLERANCE * math.hypot(scipy_error, openmc_remaining_se) + 1e-9
            remaining_delta = abs(expected_remaining - openmc_remaining)
            passed = intersection_delta <= intersection_tolerance and remaining_delta <= remaining_tolerance
            validations.append({
                "component_id": cid,
                "adaptive_quadrature_intersection_m3": independent,
                "openmc_point_sample_intersection_m3": openmc_intersection,
                "openmc_point_sample_standard_error_m3": openmc_intersection_se,
                "intersection_difference_m3": intersection_delta,
                "intersection_joint_tolerance_m3": intersection_tolerance,
                "quadrature_remaining_volume_m3": expected_remaining,
                "openmc_remaining_volume_m3": openmc_remaining,
                "openmc_remaining_standard_error_m3": openmc_remaining_se,
                "remaining_volume_difference_m3": remaining_delta,
                "remaining_volume_joint_tolerance_m3": remaining_tolerance,
                "status": "PASS" if passed else "FAIL",
            })
        report["transport_volume_check"] = "PASS" if all(v["status"] == "PASS" for v in validations) else "FAIL"
        report["volume_validation_method"] = "independent SciPy adaptive quadrature vs seeded OpenMC Python Geometry.find uniform point classification and OpenMC remaining-cell volume estimate"
        report["joint_sigma_tolerance"] = JOINT_SIGMA_TOLERANCE
        report["checks_are_geometry_only"] = True
        report["worker_result_sha256"] = digest(worker_path)
        report["transport_artifact_sha256"] = digest(artifact_path)
        report["run_record_sha256"] = digest(run_path)
        report["input_sha256"] = digest(input_path)
        report["component_volume_validation"] = validations
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", type=Path, required=True)
    parser.add_argument("--variant", required=True)
    parser.add_argument("--worker-result", type=Path)
    parser.add_argument("--transport-artifact", type=Path)
    parser.add_argument("--run-record", type=Path)
    parser.add_argument("--output", type=Path, help="Create a new report file; otherwise print to stdout")
    args = parser.parse_args()
    report = check(args.scenario, args.variant, args.worker_result, args.transport_artifact, args.run_record)
    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        with args.output.open("x") as stream:
            stream.write(text)
    else:
        print(text, end="")


if __name__ == "__main__":
    main()
