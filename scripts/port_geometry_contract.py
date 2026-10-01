#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Strict package eligibility checks for the finite-port geometry audit."""
from __future__ import annotations

import math
from typing import Any


def validate_ownership_audits(audit: dict[str, Any], penetration: dict[str, Any], *,
                              scenario_sha256: str, variant_id: str,
                              input_sha256: str, component_materials: dict[str, str]) -> None:
    if (audit.get("schema_version") != "faris-openmc-geometry-ownership-audit/v0.1"
            or audit.get("status") != "PASS"
            or audit.get("checks_are_geometry_only") is not True
            or audit.get("scientific_qualification") != "NOT_EVALUATED"
            or "Geometry.find" not in audit.get("method", "")
            or "zero transport histories" not in audit.get("method", "")
            or audit.get("scenario_sha256") != scenario_sha256
            or audit.get("variant_id") != variant_id
            or audit.get("input_sha256") != input_sha256):
        raise ValueError("geometry ownership audit is unsupported, failing, or identity-mismatched")
    if (not _sha256(scenario_sha256) or not _sha256(input_sha256)
            or not isinstance(variant_id, str) or not variant_id):
        raise ValueError("geometry audit has malformed identity fields")
    radius = audit.get("plasma_radius_m")
    gap = audit.get("declared_plasma_to_first_wall_clearance_m")
    first_wall_inner = audit.get("first_wall_inner_radius_m")
    if (not _positive_finite(radius) or not _positive_finite(gap)
            or not _positive_finite(first_wall_inner)
            or abs((first_wall_inner - radius) - gap) > 1e-9
            or audit.get("clearance_status") != "PASS"):
        raise ValueError("plasma to first-wall clearance is not positively verified")

    toroidal = audit.get("toroidal_probe_directions_rad")
    cross_section = audit.get("cross_section_probe_directions_rad")
    if (not isinstance(toroidal, list) or len(toroidal) != 3
            or not isinstance(cross_section, list) or len(cross_section) != 4
            or any(not _finite(value) for value in toroidal + cross_section)):
        raise ValueError("geometry ownership probe directions are unsupported")
    phis = [f"{float(value):.8f}" for value in toroidal]
    thetas = [f"{float(value):.8f}" for value in cross_section]
    expected: dict[str, tuple[str, str]] = {
        f"plasma-interior-phi-{phi}": ("plasma-source-domain", "void") for phi in phis
    }
    for phi in phis:
        expected[f"clearance-near-plasma-phi-{phi}"] = ("plasma-first-wall-clearance", "void")
        expected[f"clearance-near-first-wall-phi-{phi}"] = ("plasma-first-wall-clearance", "void")
    for component, material in component_materials.items():
        for phi in phis:
            expected[f"{component}-near-inner-phi-{phi}"] = (component, material)
            expected[f"{component}-near-outer-phi-{phi}"] = (component, material)
            for theta in thetas:
                expected[f"{component}-mid-phi-{phi}-theta-{theta}"] = (component, material)

    probes = audit.get("probes")
    if (not isinstance(probes, list) or audit.get("probe_count") != len(probes)
            or audit.get("failed_probe_count") != 0 or len(probes) != len(expected)):
        raise ValueError("geometry audit has missing, extra, or failed probes")
    seen: set[str] = set()
    cells: dict[str, int] = {}
    materials: dict[str, int] = {}
    for probe in probes:
        if not isinstance(probe, dict):
            raise ValueError("geometry audit contains a malformed probe")
        probe_id = probe.get("probe_id")
        if not isinstance(probe_id, str) or probe_id in seen or probe_id not in expected:
            raise ValueError("geometry audit has an unexpected or duplicate probe")
        seen.add(probe_id)
        cell_name, material_id = expected[probe_id]
        if (probe.get("status") != "PASS"
                or probe.get("expected_cell_name") != cell_name
                or probe.get("observed_cell_name") != cell_name
                or not _positive_integer(probe.get("expected_cell_id"))
                or probe.get("expected_cell_id") != probe.get("observed_cell_id")):
            raise ValueError(f"OpenMC cell ownership mismatch at {probe_id}")
        if material_id == "void":
            if (probe.get("expected_material_id") != "void"
                    or probe.get("expected_openmc_material_name") is not None
                    or probe.get("observed_openmc_material_name") is not None
                    or probe.get("expected_openmc_material_id") is not None
                    or probe.get("observed_openmc_material_id") is not None):
                raise ValueError(f"expected void cell contains material at {probe_id}")
        else:
            if (probe.get("expected_material_id") != material_id
                    or probe.get("expected_openmc_material_name") != material_id
                    or probe.get("observed_openmc_material_name") != material_id
                    or not _positive_integer(probe.get("expected_openmc_material_id"))
                    or probe.get("expected_openmc_material_id") != probe.get("observed_openmc_material_id")):
                raise ValueError(f"OpenMC material ownership mismatch at {probe_id}")
            name = probe["observed_openmc_material_name"]
            numeric_id = probe["observed_openmc_material_id"]
            if name in materials and materials[name] != numeric_id:
                raise ValueError(f"OpenMC material identity is inconsistent at {probe_id}")
            materials[name] = numeric_id
        numeric_cell = probe["observed_cell_id"]
        if cell_name in cells and cells[cell_name] != numeric_cell:
            raise ValueError(f"OpenMC cell identity is inconsistent at {probe_id}")
        cells[cell_name] = numeric_cell
        point = probe.get("point_xyz_cm")
        if not isinstance(point, list) or len(point) != 3 or any(not _finite(v) for v in point):
            raise ValueError(f"invalid geometry probe coordinate at {probe_id}")

    if len(seen) != len(expected):
        raise ValueError("geometry audit probe inventory is incomplete")
    if (not isinstance(penetration, dict)
            or penetration.get("scenario_sha256") != scenario_sha256
            or penetration.get("variant_id") != variant_id
            or penetration.get("not_a_physical_validation") is not True
            or penetration.get("fractional_volume_standard_errors_are_binomial") is not True
            or penetration.get("independent_of_Rust_midpoint_quadrature") is not True):
        raise ValueError("port void-confirmation audit is unsupported or identity-mismatched")
    counts = penetration.get("cell_counts")
    confirmations = penetration.get("final_port_void_confirmation_counts_by_component")
    if (not isinstance(counts, dict) or set(counts) != set(component_materials)
            or not isinstance(confirmations, dict) or set(confirmations) != set(component_materials)
            or any(not _positive_integer(counts[item])
                   or not _positive_integer(confirmations[item])
                   or confirmations[item] != counts[item]
                   for item in component_materials)):
        raise ValueError("not all intersected component samples are confirmed as port void")
    if not _positive_integer(penetration.get("samples")):
        raise ValueError("port volume audit has no bounded sample count")


def _finite(value: Any) -> bool:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return False
    try:
        return math.isfinite(value)
    except OverflowError:
        return False


def _positive_finite(value: Any) -> bool:
    return _finite(value) and value > 0


def _positive_integer(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def _sha256(value: Any) -> bool:
    if not isinstance(value, str) or len(value) != 64:
        return False
    try:
        bytes.fromhex(value)
    except ValueError:
        return False
    return True
