#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Offline identity and field/spectrum checks for recorded transport bundles."""
from __future__ import annotations

import hashlib
import json
import math
import re
from typing import Any

MAX_BUNDLE_BYTES = 32 * 1024 * 1024
MAX_MESH_BINS = 32_768
MAX_NORMALIZED_RESPONSES = 8_192
REQUIRED_FILES = {
    "run.json", "input.json", "scenario.json", "audit.json",
    "reactor_transport.py", "solver/transport-artifact.json",
}
BATCH_VALUES_FILE = "solver/transport-batch-values.json"
REQUIRED_RESPONSES = {"total-tritium-production", "heating-total-whole-model"}


def bare_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label}: missing SHA-256 identity")
    result = value.removeprefix("sha256:")
    if len(result) != 64:
        raise ValueError(f"{label}: malformed SHA-256 identity")
    try:
        bytes.fromhex(result)
    except ValueError as error:
        raise ValueError(f"{label}: malformed SHA-256 identity") from error
    return result


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def validate_geometry_audit(audit: dict[str, Any], *, scenario_sha256: str,
                            input_sha256: str, variant_id: str,
                            component_materials: dict[str, str]) -> None:
    if (audit.get("schema_version") != "faris-openmc-geometry-ownership-audit/v0.1"
            or audit.get("status") != "PASS"
            or audit.get("checks_are_geometry_only") is not True
            or audit.get("scientific_qualification") != "NOT_EVALUATED"
            or audit.get("scenario_sha256") != scenario_sha256
            or audit.get("input_sha256") != input_sha256
            or audit.get("variant_id") != variant_id):
        raise ValueError("worker geometry audit is unsupported, failing, or identity-mismatched")
    radius = audit.get("plasma_radius_m")
    gap = audit.get("declared_plasma_to_first_wall_clearance_m")
    first_wall = audit.get("first_wall_inner_radius_m")
    if (not _positive_finite(radius) or not _positive_finite(gap)
            or not _positive_finite(first_wall)
            or abs((first_wall - radius) - gap) > 1e-9
            or audit.get("clearance_status") != "PASS"):
        raise ValueError("worker geometry audit does not verify positive plasma-wall clearance")
    phis = audit.get("toroidal_probe_directions_rad")
    thetas = audit.get("cross_section_probe_directions_rad")
    if (not isinstance(phis, list) or len(phis) != 3
            or not isinstance(thetas, list) or len(thetas) != 4
            or any(not _finite(value) for value in phis + thetas)):
        raise ValueError("worker geometry audit has unsupported probe directions")
    if not component_materials:
        raise ValueError("geometry ownership requires declared component materials")
    expected: dict[str, tuple[str, str]] = {
        f"plasma-interior-phi-{phi:.8f}": ("plasma-source-domain", "void")
        for phi in phis
    }
    for phi in phis:
        expected[f"clearance-near-plasma-phi-{phi:.8f}"] = ("plasma-first-wall-clearance", "void")
        expected[f"clearance-near-first-wall-phi-{phi:.8f}"] = ("plasma-first-wall-clearance", "void")
    for component_id, material_id in component_materials.items():
        for phi in phis:
            expected[f"{component_id}-near-inner-phi-{phi:.8f}"] = (component_id, material_id)
            expected[f"{component_id}-near-outer-phi-{phi:.8f}"] = (component_id, material_id)
            for theta in thetas:
                expected[f"{component_id}-mid-phi-{phi:.8f}-theta-{theta:.8f}"] = (
                    component_id, material_id)
    probes = audit.get("probes")
    if (not isinstance(probes, list) or len(probes) != len(expected)
            or audit.get("probe_count") != len(probes)
            or audit.get("failed_probe_count") != 0):
        raise ValueError("worker geometry audit has missing, extra, or failed probes")
    seen: set[str] = set()
    cell_ids: dict[str, int] = {}
    material_ids: dict[str, int] = {}
    for probe in probes:
        if not isinstance(probe, dict):
            raise ValueError("worker geometry audit contains a malformed probe")
        probe_id = probe.get("probe_id")
        if not isinstance(probe_id, str) or probe_id in seen or probe_id not in expected:
            raise ValueError("worker geometry audit has an unexpected or duplicate probe")
        seen.add(probe_id)
        cell_name, material = expected[probe_id]
        cell_id = probe.get("observed_cell_id")
        if (probe.get("status") != "PASS"
                or probe.get("expected_cell_name") != cell_name
                or probe.get("observed_cell_name") != cell_name
                or not _positive_int(probe.get("expected_cell_id"))
                or cell_id != probe.get("expected_cell_id")):
            raise ValueError(f"worker cell ownership failed at {probe_id}")
        if cell_name in cell_ids and cell_ids[cell_name] != cell_id:
            raise ValueError(f"worker cell ID changes across probes for {cell_name}")
        cell_ids[cell_name] = cell_id
        if material == "void":
            if (probe.get("expected_material_id") != "void"
                    or any(probe.get(key) is not None for key in (
                        "expected_openmc_material_name", "expected_openmc_material_id",
                        "observed_openmc_material_name", "observed_openmc_material_id"))):
                raise ValueError(f"expected void is assigned material at {probe_id}")
        else:
            numeric_id = probe.get("observed_openmc_material_id")
            if (probe.get("expected_material_id") != material
                    or probe.get("expected_openmc_material_name") != material
                    or probe.get("observed_openmc_material_name") != material
                    or not _positive_int(probe.get("expected_openmc_material_id"))
                    or numeric_id != probe.get("expected_openmc_material_id")):
                raise ValueError(f"worker material ownership failed at {probe_id}")
            if material in material_ids and material_ids[material] != numeric_id:
                raise ValueError(f"worker material ID changes across probes for {material}")
            material_ids[material] = numeric_id
        point = probe.get("point_xyz_cm")
        if not isinstance(point, list) or len(point) != 3 or any(not _finite(value) for value in point):
            raise ValueError(f"worker geometry audit has invalid coordinates at {probe_id}")
    if len(seen) != len(expected):
        raise ValueError("worker geometry audit probe inventory is incomplete")


def parse_embedded(files: dict[str, str], key: str) -> tuple[bytes, dict[str, Any]]:
    if not isinstance(files.get(key), str):
        raise ValueError(f"recorded transport bundle lacks {key}")
    encoded = files[key].encode("utf-8")
    try:
        value = json.loads(encoded)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"recorded bundle contains invalid JSON at {key}") from error
    if not isinstance(value, dict):
        raise ValueError(f"recorded bundle entry is not a JSON object: {key}")
    return encoded, value


def validate_recorded_bundle(bundle: dict[str, Any], *, scenario_sha256: str,
                             variant_id: str, expected_run_sha256: str | None = None,
                             expected_raw_artifact_sha256: str | None = None,
                             mesh_nonzero_flux_bin_count: int | None = None) -> dict[str, Any]:
    if bundle.get("schema_version") != "faris-recorded-transport-bundle/v0.1":
        raise ValueError("unsupported recorded transport bundle schema")
    files = bundle.get("files")
    if not isinstance(files, dict) or not REQUIRED_FILES <= set(files):
        raise ValueError("recorded transport bundle has missing required files")
    if set(files) - REQUIRED_FILES - {"solver/worker-result.json", "solver/transport-spectra.json",
                                      BATCH_VALUES_FILE}:
        raise ValueError("recorded transport bundle contains undeclared files")
    if any(not isinstance(value, str) for value in files.values()):
        raise ValueError("recorded bundle file contents must all be UTF-8 text")
    size = len(json.dumps(bundle, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
    if size > MAX_BUNDLE_BYTES or len(files) > 9:
        raise ValueError("recorded transport bundle exceeds its file or byte bound")

    run_bytes, run = parse_embedded(files, "run.json")
    scenario_bytes, scenario = parse_embedded(files, "scenario.json")
    input_bytes, input_record = parse_embedded(files, "input.json")
    artifact_bytes, artifact = parse_embedded(files, "solver/transport-artifact.json")
    actual_scenario_sha = digest_bytes(scenario_bytes)
    actual_run_sha = digest_bytes(run_bytes)
    actual_input_sha = digest_bytes(input_bytes)
    raw_sha = digest_bytes(artifact_bytes)
    if (actual_scenario_sha != scenario_sha256
            or run.get("scenario_sha256") != scenario_sha256
            or run.get("variant_id") != variant_id
            or run.get("execution", {}).get("execution_status") != "SUCCEEDED"
            or bare_sha256(run.get("input_sha256"), "run input") != actual_input_sha
            or bare_sha256(input_record.get("request", {}).get("scenario_sha256"), "input scenario")
            != scenario_sha256
            or input_record.get("request", {}).get("variant_id") != variant_id
            or bare_sha256(run.get("raw_artifact_sha256"), "run raw artifact") != raw_sha):
        raise ValueError("recorded bundle run/input/scenario/raw artifact identities do not match")
    if expected_run_sha256 is not None and actual_run_sha != bare_sha256(expected_run_sha256, "expected run"):
        raise ValueError("recorded bundle run.json differs from the indexed run identity")
    if (expected_raw_artifact_sha256 is not None
            and raw_sha != bare_sha256(expected_raw_artifact_sha256, "expected raw artifact")):
        raise ValueError("recorded bundle raw artifact differs from the indexed identity")

    for run_field, filename in (("audit_sha256", "audit.json"),
                                ("adapter_sha256", "reactor_transport.py")):
        if filename not in files or not isinstance(files[filename], str):
            raise ValueError(f"recorded bundle is missing {filename}")
        if bare_sha256(run.get(run_field), f"run {run_field}") != digest_bytes(files[filename].encode()):
            raise ValueError(f"recorded bundle {filename} hash differs from run record")
    if "solver/worker-result.json" not in files or "solver/transport-spectra.json" not in files:
        raise ValueError("offline fields require the exact worker and transport-spectra artifacts")
    worker_bytes, worker = parse_embedded(files, "solver/worker-result.json")
    spectra_bytes, spectra = parse_embedded(files, "solver/transport-spectra.json")
    worker_sha = digest_bytes(worker_bytes)
    spectra_sha = digest_bytes(spectra_bytes)
    if bare_sha256(run.get("worker_result_sha256"), "run worker result") != worker_sha:
        raise ValueError("recorded worker result does not match run receipt")
    if bare_sha256(run.get("transport_spectra_sha256"), "run spectra") != spectra_sha:
        raise ValueError("recorded spectra do not match run receipt")
    # Runs recorded since the response covariance carry the per-batch values
    # behind it; older bundles do not. When present it must be the file the
    # worker and the transport artifact name.
    if BATCH_VALUES_FILE in files:
        batch_sha = digest_bytes(files[BATCH_VALUES_FILE].encode("utf-8"))
        covariance = artifact.get("response_covariance") or {}
        if (bare_sha256(worker.get("transport_batch_values_sha256"), "worker batch values") != batch_sha
                or bare_sha256(covariance.get("batch_values_sha256"), "artifact batch values") != batch_sha):
            raise ValueError("recorded per-batch values do not match the worker and artifact records")
    if (spectra.get("schema_version") != "faris-transport-spectra/v0.1"
            or spectra.get("scenario_sha256") != scenario_sha256
            or spectra.get("variant_id") != variant_id
            or bare_sha256(spectra.get("input_sha256"), "spectra input") != actual_input_sha):
        raise ValueError("recorded spectrum artifact identity differs from the run")
    normalized_spectra = run.get("normalized_spectra")
    raw_spectra = spectra.get("spectra")
    if (not isinstance(normalized_spectra, list) or not normalized_spectra
            or not isinstance(raw_spectra, list) or len(raw_spectra) != len(normalized_spectra)):
        raise ValueError("run bundle lacks raw or normalized spectral fields")
    expected_spectra = {(item.get("component_id"), item.get("particle")) for item in normalized_spectra
                        if isinstance(item, dict)}
    physics = input_record.get("physics")
    component_assignments = physics.get("component_assignments") if isinstance(physics, dict) else None
    if not isinstance(component_assignments, list) or not component_assignments:
        raise ValueError("recorded bundle input has no component assignment list")
    expected_components = set()
    for item in component_assignments:
        component_id = item.get("component_id") if isinstance(item, dict) else None
        if not isinstance(component_id, str) or not component_id or component_id in expected_components:
            raise ValueError("recorded bundle has malformed or duplicate component assignments")
        expected_components.add(component_id)
    if (expected_spectra != {(component, particle) for component in expected_components
                              for particle in ("neutron", "photon")}
            or len(expected_spectra) != len(normalized_spectra)):
        raise ValueError("normalized spectra do not cover each component and particle scope exactly")
    for item in raw_spectra:
        if not isinstance(item, dict):
            raise ValueError("raw spectrum entry is not an object")
        if (item.get("component_id"), item.get("particle")) not in expected_spectra:
            raise ValueError("raw spectrum is not bound to a declared component")
        edges = item.get("energy_edges_ev")
        means = item.get("mean_cm_per_source_per_bin")
        errors = item.get("standard_error_cm_per_source_per_bin")
        if (not isinstance(edges, list) or len(edges) < 2
                or not isinstance(means, list) or len(means) + 1 != len(edges)
                or not isinstance(errors, list) or len(errors) != len(means)
                or any(not _finite(number) for number in edges + means + errors)):
            raise ValueError("raw spectrum energy bins or estimates are malformed")
        if (any(right <= left for left, right in zip(edges, edges[1:]))
                or any(value < 0 for value in means)
                or any(value < 0 for value in errors)):
            raise ValueError("raw spectrum bins or standard errors are outside their valid domain")
    raw_spectrum_pairs = {(item["component_id"], item["particle"]) for item in raw_spectra}
    if len(raw_spectrum_pairs) != len(raw_spectra) or raw_spectrum_pairs != expected_spectra:
        raise ValueError("raw spectra do not cover each normalized spectrum exactly once")

    normalized = run.get("normalized")
    results = normalized.get("results") if isinstance(normalized, dict) else None
    if not isinstance(results, list) or len(results) > MAX_NORMALIZED_RESPONSES:
        raise ValueError("run exceeds the supported 8,192 normalized-response bound")
    ids = {item.get("response_id") for item in results if isinstance(item, dict)} if isinstance(results, list) else set()
    if not REQUIRED_RESPONSES <= ids:
        raise ValueError("run lacks required integrated tritium/heating responses")
    mesh = run.get("mesh")
    dimensions = mesh.get("dimensions") if isinstance(mesh, dict) else None
    if (not isinstance(dimensions, list) or len(dimensions) != 3
            or any(not _positive_int(value) for value in dimensions)):
        raise ValueError("run lacks a valid spatial field mesh")
    bin_count = math.prod(dimensions)
    if bin_count > MAX_MESH_BINS:
        raise ValueError("spatial field mesh exceeds the supported 32,768-bin limit")
    mesh_results = [item for item in results or []
                    if item.get("domain", {}).get("kind") == "mesh"]
    if (len(mesh_results) != bin_count
            or any(item.get("domain", {}).get("mesh_id") != mesh.get("id") for item in mesh_results)
            or {item.get("domain", {}).get("bin") for item in mesh_results} != set(range(bin_count))):
        raise ValueError("normalized mesh fields do not cover every declared mesh bin")
    if (not isinstance(mesh_nonzero_flux_bin_count, int)
            or mesh_nonzero_flux_bin_count <= 0 or mesh_nonzero_flux_bin_count > bin_count):
        raise ValueError("verified run inspection reports no usable spatial field bins")
    geometry_audit = worker.get("geometry_ownership_audit")
    if not isinstance(geometry_audit, dict):
        raise ValueError("worker result lacks geometry ownership diagnostics")
    component_materials = {item["component_id"]: item["material_id"]
                           for item in component_assignments}
    validate_geometry_audit(
        geometry_audit, scenario_sha256=scenario_sha256,
        input_sha256=actual_input_sha, variant_id=variant_id,
        component_materials=component_materials)

    return {
        "run_sha256": actual_run_sha,
        "input_sha256": actual_input_sha,
        "scenario_sha256": actual_scenario_sha,
        "variant_id": variant_id,
        "raw_artifact_sha256": raw_sha,
        "worker_result_sha256": worker_sha,
        "transport_spectra_sha256": spectra_sha,
        "mesh_dimensions": dimensions,
        "mesh_bin_count": bin_count,
        "mesh_nonzero_flux_bin_count": mesh_nonzero_flux_bin_count,
        "spectral_curve_count": len(normalized_spectra),
        "normalized_response_count": len(results),
    }


SWEEP_VARIANT_PATTERN = re.compile(r"blanket-\d{3}cm")
SWEEP_BUNDLE_DIRECTORY = "sweep/bundles"


def sweep_allocations(scenario: dict[str, Any]) -> dict[str, tuple[float, float]]:
    """Return {variant id: (blanket m, shield m)} for the allocation-sweep scenario."""
    allocations: dict[str, tuple[float, float]] = {}
    for variant in scenario.get("variants") or []:
        thickness = {layer.get("id"): layer.get("thickness_m") for layer in variant.get("layers") or []}
        blanket, shield = thickness.get("blanket"), thickness.get("shield")
        if (not isinstance(variant.get("id"), str) or not _positive_finite(blanket)
                or not _positive_finite(shield)):
            raise ValueError("sweep scenario variant lacks blanket and shield thicknesses")
        allocations[variant["id"]] = (float(blanket), float(shield))
    return allocations


def inspect_sweep_bundle(bundle: dict[str, Any], *, scenario_sha256: str,
                         scenario: dict[str, Any]) -> dict[str, Any]:
    """Validate one allocation-sweep bundle and return its identity record.

    The variant, seed and mesh usability come from the bundle's own run record;
    the full recorded-bundle contract then binds every embedded artifact.
    """
    files = bundle.get("files")
    if not isinstance(files, dict):
        raise ValueError("recorded transport bundle has missing required files")
    run_bytes, run = parse_embedded(files, "run.json")
    variant_id = run.get("variant_id")
    if not isinstance(variant_id, str) or not SWEEP_VARIANT_PATTERN.fullmatch(variant_id):
        raise ValueError(f"sweep run variant is not blanket-NNNcm: {variant_id!r}")
    allocations = sweep_allocations(scenario)
    if variant_id not in allocations:
        raise ValueError(f"sweep variant is not declared by the allocation-sweep scenario: {variant_id}")
    seed = (run.get("sampling") or {}).get("seed")
    if not isinstance(seed, int) or isinstance(seed, bool):
        raise ValueError(f"sweep run {variant_id} records no integer transport seed")
    mesh_id = (run.get("mesh") or {}).get("id")
    results = (run.get("normalized") or {}).get("results") or []
    nonzero = sum(1 for item in results
                  if isinstance(item, dict) and (item.get("domain") or {}).get("kind") == "mesh"
                  and (item.get("domain") or {}).get("mesh_id") == mesh_id
                  and _finite(item.get("mean")) and item["mean"] > 0.0)
    summary = validate_recorded_bundle(
        bundle, scenario_sha256=scenario_sha256, variant_id=variant_id,
        mesh_nonzero_flux_bin_count=nonzero)
    blanket, shield = allocations[variant_id]
    return {"variant_id": variant_id, "blanket_thickness_m": blanket,
            "shield_thickness_m": shield, "seed": seed,
            "run_record_sha256": "sha256:" + digest_bytes(run_bytes),
            "raw_artifact_sha256": summary["raw_artifact_sha256"],
            "offline_field_and_spectrum_identity": summary}


def check_sweep_set(records: list[dict[str, Any]]) -> None:
    """Refuse duplicate variants or reused seeds across a set of sweep records."""
    variants = [item["variant_id"] for item in records]
    seeds = [item["seed"] for item in records]
    if len(set(variants)) != len(variants):
        raise ValueError("sweep bundles repeat a variant")
    if len(set(seeds)) != len(seeds):
        raise ValueError("sweep bundles reuse a transport seed")


def _finite(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _positive_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def _positive_finite(value: Any) -> bool:
    return _finite(value) and value > 0
