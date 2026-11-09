#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Prepare exact per-mesh/material ACTINV requests and photon-source mixtures.

This module has no solver entrypoint. FARIS Rust owns cancellable ACTINV execution through its
Activation API; OpenMC supplies a hash-bound 709-group track-length response. The helper performs
bounded, deterministic reshaping only.
"""
from __future__ import annotations

import hashlib
import argparse
import json
import math
import os
from pathlib import Path
import xml.etree.ElementTree as ET

MAX_BINS = 5000
MAX_GROUPS = 709
MAX_SOURCE_GROUPS = 10000
VOLUME_REL_TOL = 1.0e-5
MAX_REQUEST_BYTES = 4 * 1024 * 1024
MAX_RESPONSE_BYTES = 256 * 1024 * 1024


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def checked_ref(ref: dict, label: str) -> Path:
    if set(ref) != {"path", "sha256"}:
        raise ValueError(f"{label} reference must contain path and sha256")
    path = Path(ref["path"]).resolve(strict=True)
    if not path.is_file() or path.stat().st_size > MAX_RESPONSE_BYTES:
        raise ValueError(f"{label} must be a bounded regular file")
    if file_sha256(path).lower() != str(ref["sha256"]).lower():
        raise ValueError(f"{label} SHA-256 mismatch")
    return path


def validate_safe_model_sources(root: ET.Element) -> None:
    """Accept only standard data-defined IndependentSource and MeshSource XML."""
    source_types = {
        "space": {"box", "point", "cartesian", "cylindrical", "spherical", "fission", "mesh"},
        "angle": {"isotropic", "monodirectional", "mu-phi", "polar_azimuthal", "mueller"},
        "energy": {"discrete", "uniform", "maxwell", "watt", "tabular", "general_evaporation",
                   "evaporation", "madland_nix", "muir", "legendre", "spatial"},
        "time": {"discrete", "uniform", "maxwell", "watt", "tabular", "general_evaporation",
                 "evaporation", "madland_nix", "muir", "legendre", "spatial"},
    }
    for element in root.iter():
        if {key.lower() for key in element.attrib}.intersection(
            {"library", "module", "callable", "python_module"}
        ):
            raise ValueError("model XML must not load compiled or external plugins")
    settings = root.find("settings")
    if settings is None:
        raise ValueError("checked OpenMC model has no settings element")
    mesh_ids = {node.attrib.get("id") for node in settings.findall("mesh")}
    sources = settings.findall("source")
    if not sources:
        raise ValueError("checked OpenMC model has no explicit source distribution")

    def check_independent(source: ET.Element) -> float:
        if source.attrib.get("type", "independent").lower() != "independent":
            raise ValueError("mesh source bins must be built-in IndependentSources")
        if set(source.attrib) - {"type", "strength", "particle"}:
            raise ValueError("source XML has unsupported attributes or executable/file-backed plugin references")
        if source.attrib.get("particle", "neutron").lower() != "neutron":
            raise ValueError("the checked activation source must contain neutrons only")
        if any(child.tag not in {"space", "angle", "energy", "time", "constraints"} for child in source):
            raise ValueError("independent source contains an unsupported XML element")
        strength = _finite(float(source.attrib.get("strength", "1")), "source strength", minimum=0)
        for child in source:
            if child.tag in source_types:
                kind = child.attrib.get("type", "").lower()
                if kind not in source_types[child.tag]:
                    raise ValueError(f"unsupported data-defined source distribution {child.tag}:{kind}")
                allowed_attrs = {"type", "mesh"} if kind == "mesh" else {"type"}
                if child.tag == "angle" and kind == "mu-phi":
                    allowed_attrs |= {"reference_uvw", "reference_vwu"}
                    for key in {"reference_uvw", "reference_vwu"}.intersection(child.attrib):
                        for value in child.attrib[key].split():
                            _finite(float(value), f"angle {key}")
                if set(child.attrib) - allowed_attrs:
                    raise ValueError("source distribution has unsupported or external-plugin attributes")
                allowed_descendants = {"parameters"}
                if child.tag == "angle" and kind == "mu-phi":
                    allowed_descendants |= {"mu", "phi"}
                if any(node.tag not in allowed_descendants for node in list(child.iter())[1:]):
                    raise ValueError("source distribution contains unsupported nested XML")
                for nested in list(child):
                    if nested.tag in {"mu", "phi"}:
                        if nested.attrib.get("type", "") not in source_types["energy"]:
                            raise ValueError("mu-phi angle distributions must use a standard numeric distribution")
                        if set(nested.attrib) - {"type"} or any(n.tag != "parameters" for n in nested):
                            raise ValueError("mu-phi angle XML contains unsupported fields")
                for parameter in child.iter("parameters"):
                    if list(parameter) or parameter.attrib:
                        raise ValueError("source distribution parameters must be plain numeric data")
                    for value in (parameter.text or "").replace(",", " ").split():
                        _finite(float(value), f"{child.tag} distribution parameter")
            elif child.tag == "constraints" and any(
                nested.tag not in {"domain_type", "domain_ids", "rejection_strategy", "energy_bounds", "time_bounds", "fissionable"}
                for nested in child
            ):
                raise ValueError("source constraints contain an unsupported field")
        return strength

    total_strength = 0.0
    for source in sources:
        source_type = source.attrib.get("type", "independent").lower()
        if source_type == "independent":
            strength = check_independent(source)
            if strength <= 0:
                raise ValueError("source strength must be positive")
            total_strength += strength
        elif source_type == "mesh":
            if set(source.attrib) - {"type", "mesh", "strength"}:
                raise ValueError("mesh source has unsupported or external-plugin attributes")
            if source.attrib.get("mesh") not in mesh_ids:
                raise ValueError("mesh source references an undeclared settings mesh")
            bins = source.findall("source")
            if not bins:
                raise ValueError("mesh source has no data-defined source bins")
            strengths = [check_independent(item) for item in bins]
            if any(value <= 0 for value in strengths):
                raise ValueError("mesh source bin strengths must be positive")
            outer = _finite(float(source.attrib.get("strength", "nan")), "mesh source strength", minimum=0)
            if not math.isclose(math.fsum(strengths), outer, rel_tol=1e-12, abs_tol=1e-12):
                raise ValueError("mesh source outer strength does not equal its source-bin strengths")
            if any(child.tag not in {"source", "constraints"} for child in source):
                raise ValueError("mesh source contains unsupported or executable XML")
            if any(node.tag not in {"source", "constraints", "domain_type", "domain_ids", "rejection_strategy",
                                    "energy_bounds", "time_bounds", "fissionable"}
                   for child in source if child.tag == "constraints" for node in child.iter()):
                raise ValueError("mesh source constraints contain unsupported XML")
            total_strength += outer
        else:
            raise ValueError(f"unsupported OpenMC source type {source_type!r}; only independent and mesh are allowed")
    if not math.isclose(total_strength, 1.0, rel_tol=1e-10, abs_tol=1e-12):
        raise ValueError("checked neutron source strengths do not sum to unit source strength")


def activation_batches_from_artifacts(request: dict) -> dict:
    """Read hash-bound OpenMC 709 response + checked model/material artifacts into batches."""
    expected = {"schema", "model", "geometry", "component_material_map", "response_npz",
                "neutron_response_receipt", "cross_sections", "activation_library_npz", "activation_library_index", "activation_mesh",
                "source_groups", "event", "base_source_rate_n_s", "photon_energy_edges_ev"}
    if set(request) != expected or request.get("schema") != "faris-maintenance-709-response-request/v0.1":
        raise ValueError("request fields/schema do not match faris-maintenance-709-response-request/v0.1")
    model_path = checked_ref(request["model"], "OpenMC model XML")
    geometry_path = checked_ref(request["geometry"], "DAGMC H5M")
    map_path = checked_ref(request["component_material_map"], "component/material map")
    response_path = checked_ref(request["response_npz"], "709-group OpenMC response")
    response_record_path = checked_ref(request["neutron_response_receipt"], "neutron response execution receipt")
    cross_sections_path = checked_ref(request["cross_sections"], "OpenMC cross sections")
    library_path = checked_ref(request["activation_library_npz"], "ACTINV TENDL-709 library")
    library_index_path = checked_ref(request["activation_library_index"], "ACTINV library index")
    root = ET.parse(model_path).getroot()
    validate_safe_model_sources(root)
    dags = root.findall(".//dagmc/filename")
    if len(dags) != 1 or (model_path.parent / (dags[0].text or "")).resolve() != geometry_path:
        raise ValueError("model XML must reference the hash-bound exact DAGMC geometry")
    component_map = json.loads(map_path.read_bytes())
    if set(component_map) != {"schema", "geometry_sha256", "materials"} or component_map["schema"] != "faris-cad-component-material-map/v0.1":
        raise ValueError("component/material map has unsupported schema")
    if component_map["geometry_sha256"].lower() != request["geometry"]["sha256"].lower():
        raise ValueError("component/material map is not bound to the selected H5M")
    seen_material_ids = set()
    for component, ids in component_map["materials"].items():
        if not isinstance(component, str) or not component or not isinstance(ids, list) or not ids:
            raise ValueError("component material map requires a nonempty positive ID list per component")
        for material_id in ids:
            if isinstance(material_id, bool) or not isinstance(material_id, int) or material_id <= 0 or material_id in seen_material_ids:
                raise ValueError("CAD material IDs must be positive and unique across all components")
            seen_material_ids.add(material_id)
    library_index = json.loads(library_index_path.read_bytes())
    if library_index.get("schema") != "actinv-library-index-1" or library_index.get("sha256_npz", "").lower() != request["activation_library_npz"]["sha256"].lower():
        raise ValueError("activation library index does not bind the selected TENDL-709 NPZ")
    import numpy as np
    with np.load(library_path, allow_pickle=False) as archive:
        energy_edges = archive["bounds"].astype(float).tolist()
    boundary_hash = hashlib.sha256(b"ACTINV-GROUP-BOUNDARIES-v1\0" +
                                   np.asarray(energy_edges, dtype="<f8").tobytes()).hexdigest()
    if len(energy_edges) != MAX_GROUPS + 1 or boundary_hash.lower() != library_index.get("group_boundary_sha256", "").lower():
        raise ValueError("ACTINV 709 energy boundaries are invalid or do not match the library index")
    response_record = json.loads(response_record_path.read_bytes())
    if response_record.get("schema") != "faris-maintenance-709-neutron-response/v0.1" or response_record.get("status") != "PASS":
        raise ValueError("709 neutron response execution receipt is absent, unsupported, or not PASS")
    expected_record = {
        "response_npz_sha256": request["response_npz"]["sha256"],
        "model_sha256": request["model"]["sha256"],
        "geometry_sha256": request["geometry"]["sha256"],
        "cross_sections_sha256": request["cross_sections"]["sha256"],
        "group_boundary_sha256": library_index["group_boundary_sha256"],
        "activation_mesh": request["activation_mesh"],
    }
    for field, expected_value in expected_record.items():
        if response_record.get(field) != expected_value:
            raise ValueError(f"709 neutron response receipt {field} does not match the request")
    source = response_record.get("source", {})
    if source.get("unit_strength") != 1.0 or source.get("base_source_rate_n_s") != request["base_source_rate_n_s"]:
        raise ValueError("709 neutron response unit-source normalization differs from the requested physical base rate")
    kernel_sha = source.get("kernel_sha256")
    if not isinstance(kernel_sha, str) or len(kernel_sha) != 64 or any(c not in "0123456789abcdef" for c in kernel_sha.lower()):
        raise ValueError("709 neutron response receipt omits the normalized source-kernel SHA-256")
    solver = response_record.get("solver", {})
    particles = solver.get("particles")
    batches = solver.get("batches")
    lost = solver.get("lost_particles")
    if not all(isinstance(x, int) and x > 0 for x in (particles, batches)) or batches < 2:
        raise ValueError("709 neutron response receipt has no complete batch evidence")
    if not isinstance(lost, int) or lost < 0 or lost > 1e-6 * particles * batches:
        raise ValueError("709 neutron response lost-particle count is unavailable or above 1e-6")
    runtime = response_record.get("runtime", {})
    if not runtime.get("openmc_python_version") or not runtime.get("openmc_c_library") or not runtime.get("openmc_c_library_sha256"):
        raise ValueError("709 neutron response receipt omits actual OpenMC runtime identity")
    with np.load(response_path, allow_pickle=False) as archive:
        arrays = {key: archive[key] for key in ("element_indices", "material_ids", "material_volumes_cm3",
                                                "integrated_track_length_cm_per_source")}
    recipes = material_recipes(str(model_path))
    source_groups = request["source_groups"]
    active_components = {group["component"] for group in source_groups}
    all_materials = component_map["materials"]
    if not active_components.issubset(all_materials):
        raise ValueError("an installed history component has no CAD materials")
    active_map = {name: all_materials[name] for name in sorted(active_components)}
    selected_rows = [i for i, mid in enumerate(arrays["material_ids"]) if int(mid) in
                     {m for ids in active_map.values() for m in ids}]
    arrays = {key: value[selected_rows] for key, value in arrays.items()}
    result = build_activation_batches(
        response_arrays=arrays, activation_mesh=request["activation_mesh"],
        materials_by_component=active_map, recipes=recipes, source_groups=source_groups,
        shutdown_s=request["event"]["shutdown_s"], cooling_s=request["event"]["cooling_s"],
        base_source_rate_n_s=request["base_source_rate_n_s"],
        neutron_energy_edges_ev=energy_edges,
        photon_energy_edges_ev=request["photon_energy_edges_ev"],
    )
    result["provenance"] = {
        "model_sha256": request["model"]["sha256"],
        "geometry_sha256": request["geometry"]["sha256"],
        "component_material_map_sha256": request["component_material_map"]["sha256"],
        "neutron_response_sha256": request["response_npz"]["sha256"],
        "actinv_library_npz_sha256": request["activation_library_npz"]["sha256"],
        "actinv_library_index_sha256": request["activation_library_index"]["sha256"],
        "group_boundary_sha256": library_index["group_boundary_sha256"],
        "group_structure": "FISPACT-709, ascending integrated group flux",
        "mesh_material_flux_units_in_response": "integrated particle-cm per source neutron",
    }
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    request_bytes = args.request.read_bytes()
    if len(request_bytes) > MAX_REQUEST_BYTES:
        raise SystemExit(f"request exceeds {MAX_REQUEST_BYTES} bytes")
    request = json.loads(request_bytes)
    base = args.request.resolve().parent
    for key in ("model", "geometry", "component_material_map", "response_npz", "neutron_response_receipt", "cross_sections", "activation_library_npz", "activation_library_index"):
        if not Path(request[key]["path"]).is_absolute():
            request[key]["path"] = str(base / request[key]["path"])
    result = activation_batches_from_artifacts(request)
    result["request_sha256"] = hashlib.sha256(request_bytes).hexdigest()
    output = json.dumps(result, sort_keys=True, separators=(",", ":")).encode("utf-8")
    if len(output) > MAX_RESPONSE_BYTES:
        raise SystemExit(f"ACTINV batch payload exceeds {MAX_RESPONSE_BYTES} bytes")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("xb") as stream:
        stream.write(output)
        stream.write(b"\n")
    print(json.dumps({"schema": "faris-maintenance-actinv-event-batch-receipt/v0.1",
                      "status": "PASS", "request_sha256": result["request_sha256"],
                      "batch_payload": str(args.out.resolve()),
                      "batch_payload_sha256": file_sha256(args.out),
                      "batch_count": len(result["batches"]),
                      "zone_count": sum(len(batch["activation_request"]["zones"]) for batch in result["batches"])}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


def _finite(value, label, *, minimum=None):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError(f"{label} must be finite")
    value = float(value)
    if minimum is not None and value < minimum:
        raise ValueError(f"{label} must be >= {minimum}")
    return value


def material_recipes(model_xml: str) -> dict[int, dict]:
    """Read only numeric OpenMC material data; no model/source plugin is loaded."""
    root = ET.parse(model_xml).getroot()
    out = {}
    for node in root.findall("./materials/material"):
        mid = int(node.attrib["id"])
        density_node = node.find("density")
        if density_node is None:
            raise ValueError(f"material {mid} has no density")
        density = _finite(float(density_node.attrib["value"]), f"material {mid} density", minimum=0.0)
        units = density_node.attrib.get("units", "g/cm3").lower().replace(" ", "")
        if units in {"kg/m3", "kg/m^3"}:
            density /= 1000.0
        elif units not in {"g/cm3", "g/cc", "g/cm^3"}:
            raise ValueError(f"material {mid} has unsupported density unit {units!r}")
        if density <= 0:
            raise ValueError(f"material {mid} density must be positive")
        composition = {}
        for nuclide in node.findall("nuclide"):
            name = nuclide.attrib.get("name")
            if not name or "ao" not in nuclide.attrib:
                raise ValueError(f"material {mid} needs explicit atom fractions for every nuclide")
            amount = _finite(float(nuclide.attrib["ao"]), f"material {mid} atom fraction", minimum=0)
            if amount:
                composition[name] = amount
        if not composition:
            raise ValueError(f"material {mid} has no positive nuclide atom fractions")
        total = math.fsum(composition.values())
        if not math.isclose(total, 1.0, rel_tol=1e-6, abs_tol=1e-8):
            raise ValueError(f"material {mid} atom fractions sum to {total}, not 1")
        out[mid] = {"density_g_cm3": density, "composition": composition}
    if not out:
        raise ValueError("model XML has no numeric material records")
    return out


def split_irradiation_schedule(source_group: dict, shutdown_s: float) -> list[dict]:
    """Remove the explicit post-shutdown zero-flux tail; retain all exact pre-shutdown steps."""
    install = _finite(source_group["installation_s"], "installation time", minimum=0)
    shutdown = _finite(shutdown_s, "shutdown time", minimum=0)
    target = shutdown - install
    if target <= 0:
        raise ValueError("shutdown must follow this component installation")
    steps = source_group["schedule"]
    if not isinstance(steps, list) or not steps or len(steps) > MAX_SOURCE_GROUPS:
        raise ValueError("component schedule is empty or exceeds its step cap")
    out, elapsed = [], 0.0
    for step in steps:
        dt = _finite(step["duration_s"], "schedule duration", minimum=0)
        rate = _finite(step["source_rate_n_s"], "schedule source rate", minimum=0)
        if dt <= 0:
            raise ValueError("schedule duration must be positive")
        if elapsed < target - 1e-6:
            portion = min(dt, target - elapsed)
            out.append({"duration_s": portion, "flux_scale": rate})
            elapsed += portion
            if portion < dt and rate != 0:
                raise ValueError("a nonzero irradiation interval crosses the shutdown boundary")
        elif rate != 0:
            raise ValueError("post-shutdown schedule contains a nonzero source rate")
    if not math.isclose(elapsed, target, rel_tol=1e-10, abs_tol=1e-6):
        raise ValueError("component schedule does not end at its exact event shutdown")
    return out


def build_activation_batches(
    *,
    response_arrays: dict,
    activation_mesh: dict,
    materials_by_component: dict[str, list[int]],
    recipes: dict[int, dict],
    source_groups: list[dict],
    shutdown_s: float,
    cooling_s: list[float],
    base_source_rate_n_s: float,
    neutron_energy_edges_ev: list[float],
    photon_energy_edges_ev: list[float],
) -> dict:
    """Create one FARIS ActivationRequest JSON per exact component history schedule.

    response_arrays names: element_indices, material_ids, material_volumes_cm3, and
    integrated_track_length_cm_per_source [bin, 709]. OpenMC flux scores are track length;
    divide by material-in-voxel volume first, then apply the fixed base neutron rate.
    """
    base_rate = _finite(base_source_rate_n_s, "base neutron source rate", minimum=0)
    if base_rate <= 0:
        raise ValueError("base neutron source rate must be positive")
    edges = [_finite(x, "neutron energy edge", minimum=0) for x in neutron_energy_edges_ev]
    photon_edges = [_finite(x, "photon energy edge", minimum=0) for x in photon_energy_edges_ev]
    if len(edges) != MAX_GROUPS + 1 or any(b <= a for a, b in zip(edges, edges[1:])):
        raise ValueError("activation response must use 710 strictly ascending FISPACT-709 edges")
    if len(photon_edges) < 3 or any(b <= a for a, b in zip(photon_edges, photon_edges[1:])):
        raise ValueError("photon energy bounds must be strictly ascending with at least two groups")
    elems = response_arrays["element_indices"]
    mids = response_arrays["material_ids"]
    volumes = response_arrays["material_volumes_cm3"]
    track = response_arrays["integrated_track_length_cm_per_source"]
    n = len(elems)
    if not 1 <= n <= MAX_BINS or len(mids) != n or len(volumes) != n or tuple(track.shape) != (n, MAX_GROUPS):
        raise ValueError(f"mesh/material response must have 1 to {MAX_BINS} aligned bins by {MAX_GROUPS} groups")
    dims = activation_mesh["dimension"]
    lower, upper = activation_mesh["lower_left_cm"], activation_mesh["upper_right_cm"]
    if len(dims) != 3 or any(not isinstance(d, int) or d < 1 for d in dims) or math.prod(dims) > 5_000_000:
        raise ValueError("activation mesh dimensions are invalid or exceed the voxel cap")
    voxel_cm3 = math.prod((upper[i] - lower[i]) / dims[i] for i in range(3))
    seen_pairs, volume_per_element, observed_by_component = set(), {}, {}
    domains = {}
    for i in range(n):
        element, material_id = int(elems[i]), int(mids[i])
        volume = _finite(float(volumes[i]), "material-in-voxel volume", minimum=0)
        if element < 0 or element >= math.prod(dims) or material_id <= 0 or volume <= 0:
            raise ValueError("response bin has invalid voxel/material identity or volume")
        pair = (element, material_id)
        if pair in seen_pairs:
            raise ValueError("mesh/material response repeats a voxel-material bin")
        seen_pairs.add(pair)
        volume_per_element[element] = volume_per_element.get(element, 0.0) + volume
        recipe = recipes.get(material_id)
        if recipe is None:
            raise ValueError(f"response material {material_id} has no checked numeric recipe")
        component_matches = [name for name, ids in materials_by_component.items() if material_id in ids]
        if len(component_matches) != 1:
            raise ValueError(f"material {material_id} must map to exactly one history component")
        values = [float(x) for x in track[i]]
        if any(not math.isfinite(x) or x < 0 for x in values):
            raise ValueError("integrated track-length bins must be finite and nonnegative")
        # flux score is integrated particle-cm/source. Normalize over the matching material volume.
        flux = [x / volume * base_rate for x in values]
        if any(not math.isfinite(x) for x in flux):
            raise ValueError("volume-normalized 709-group flux is non-finite")
        zone_id = f"e{element}_m{material_id}"
        observed_by_component.setdefault(component_matches[0], set()).add(material_id)
        domains.setdefault(component_matches[0], []).append({
            "id": zone_id,
            "mesh_element": element,
            "material_id": material_id,
            "mass_g": volume * recipe["density_g_cm3"],
            "volume_m3": volume * 1.0e-6,
            "basis": "atom_fraction",
            "composition": recipe["composition"],
            "spectrum": {"energy_edges_ev": edges, "flux_per_cm2_s": flux},
            "material_volume_cm3": volume,
        })
    for element, volume in volume_per_element.items():
        if volume > voxel_cm3 * (1.0 + VOLUME_REL_TOL):
            raise ValueError(f"material-volume sum {volume} cm3 exceeds voxel {element} volume {voxel_cm3} cm3")
    if set(domains) != set(materials_by_component):
        missing = sorted(set(materials_by_component) - set(domains))
        raise ValueError(f"currently installed components have no mesh/material activation bins: {missing}")
    for component, material_ids in materials_by_component.items():
        missing_ids = sorted(set(material_ids) - observed_by_component.get(component, set()))
        if missing_ids:
            raise ValueError(f"installed component {component} is missing mesh/material bins for IDs {missing_ids}")
    by_component = {g["component"]: g for g in source_groups}
    if set(by_component) != set(domains):
        raise ValueError("every currently installed component must have exactly one source schedule")
    batches = []
    for component in sorted(domains):
        group = by_component[component]
        irr_steps = split_irradiation_schedule(group, shutdown_s)
        steps = [{"duration_s": s["duration_s"], "flux_scale": s["flux_scale"] / base_rate}
                 for s in irr_steps]
        zone_metadata = {
            zone["id"]: {"mesh_element": zone.pop("mesh_element"),
                         "material_id": zone.pop("material_id"),
                         "material_volume_cm3": zone.pop("material_volume_cm3")}
            for zone in domains[component]
        }
        batches.append({
            "component": component,
            "zone_metadata": zone_metadata,
            "activation_request": {
                "zones": domains[component],
                "steps": steps,
                "schedule_input_sha256": None,
                "cooling_s": cooling_s,
                "photon": {"group_boundaries_ev": photon_edges},
                "mode": "auto",
            },
        })
    return {
        "schema": "faris-maintenance-actinv-event-batches/v0.1",
        "uncertainty_scope": "ACTINV inventory and source are conditional on the supplied neutron 709-group response, composition, mesh material volumes, decay data, and activation library; neutron sampling and nuclear-data/model uncertainty are excluded.",
        "spatial_approximation": "Each activation result represents one mesh-element/material intersection with volume from OpenMC MeshMaterialVolumes; 709-group track length is divided by that same material volume. Photon emission is uniform inside that material-constrained voxel bin.",
        "source_rate_normalization": "OpenMC unit-strength integrated track length is divided by matching material-in-voxel cm3 and multiplied by the authored base neutron source rate. Each history step uses exact Rust source_rate/base_source_rate. ACTINV photon groups are already absolute photons/s and are not multiplied by fusion rate again.",
        "batches": batches,
    }


def photon_emitters_from_activation(result: dict, zone_metadata: dict[str, dict], cooling_s: float) -> list[dict]:
    """Convert a complete FARIS ActivationResult into exact-strength spatial source bins."""
    if result.get("schema") != "faris-activation-result/v0.1":
        raise ValueError("unsupported ACTINV result schema")
    emitters = []
    for zone in result.get("zones", []):
        if zone.get("completeness") != "COMPLETE" or zone.get("gaps"):
            raise ValueError(f"activation zone {zone.get('id')} has data gaps or conditional completeness")
        meta = zone_metadata.get(zone.get("id"))
        if meta is None:
            raise ValueError("ACTINV zone does not map to a checked mesh/material source bin")
        point = next((p for p in zone.get("points", []) if p.get("cooling_s") == cooling_s), None)
        if point is None:
            raise ValueError(f"activation zone {zone['id']} lacks exact cooling point {cooling_s}")
        source = point.get("photon_source") or {}
        outside = _finite(source.get("photons_outside_groups_per_s", 0), "outside-group photon rate", minimum=0)
        total = _finite(source.get("total_photons_per_s", 0), "total photon source rate", minimum=0)
        groups = source.get("groups")
        if not isinstance(groups, list) or not groups:
            raise ValueError(f"activation zone {zone['id']} has no photon groups")
        grouped = math.fsum(_finite(g.get("photons_per_s"), "photon group rate", minimum=0) for g in groups)
        if outside > max(1e-12, total * 1e-12):
            raise ValueError(f"activation zone {zone['id']} has {outside} photons/s outside requested group bounds; refusing unbounded truncation")
        if not math.isclose(grouped, total, rel_tol=1e-8, abs_tol=1e-12):
            raise ValueError(f"activation zone {zone['id']} grouped photon rate does not conserve total photons/s")
        spectra = []
        for group in groups:
            rate = _finite(group.get("photons_per_s"), "photon group rate", minimum=0)
            if rate:
                energy = _finite(group.get("centroid_ev"), "photon group centroid", minimum=0)
                spectra.append({"energy_ev": energy,
                                "energy_low_ev": _finite(group.get("low_ev"), "photon group lower edge", minimum=0),
                                "energy_high_ev": _finite(group.get("high_ev"), "photon group upper edge", minimum=0),
                                "photons_per_s": rate})
        if total:
            emitters.append({**meta, "zone_id": zone["id"], "cooling_s": cooling_s,
                             "photons_per_s": total, "energy_groups": spectra,
                             "activation_input_sha256": zone.get("run", {}).get("problem_sha256")})
    return emitters


def regular_mesh_voxel_bounds(mesh: dict, element: int) -> tuple[list[float], list[float]]:
    """OpenMC RegularMesh element order is x-fastest, then y, then z (0-based output index)."""
    dims = mesh["dimension"]
    count = math.prod(dims)
    if not isinstance(element, int) or not 0 <= element < count:
        raise ValueError("mesh element index is outside the authored regular mesh")
    nx, ny, _ = dims
    i = element % nx
    j = (element // nx) % ny
    k = element // (nx * ny)
    lower_left, upper_right = mesh["lower_left_cm"], mesh["upper_right_cm"]
    widths = [(upper_right[a] - lower_left[a]) / dims[a] for a in range(3)]
    index = [i, j, k]
    lower = [lower_left[a] + index[a] * widths[a] for a in range(3)]
    upper = [lower[a] + widths[a] for a in range(3)]
    return lower, upper


def total_photon_strength(emitters: list[dict]) -> float:
    """Sum already-absolute ACTINV photon rates once for fixed-source rate normalization."""
    total = math.fsum(_finite(emitter["photons_per_s"], "emitter rate", minimum=0) for emitter in emitters)
    if not math.isfinite(total) or total <= 0:
        raise ValueError("the activated bins have no positive finite photon source strength")
    return total


def make_openmc_photon_sources(openmc, mesh: dict, emitters: list[dict], materials_by_id: dict[int, object]):
    """Build per-bin material-constrained OpenMC sources with ACTINV's absolute photons/s.

    Position is sampled from an authored voxel Box and rejected unless the particle lies in the
    exact cloned DAGMC material. This avoids vacuum dilution and retains the voxel/material domain.
    """
    sources = []
    for emitter in emitters:
        material_id = int(emitter["material_id"])
        material = materials_by_id.get(material_id)
        if material is None:
            raise ValueError(f"no OpenMC material object for activation source material {material_id}")
        total = _finite(emitter["photons_per_s"], "emitter rate", minimum=0)
        if total <= 0:
            continue
        groups = emitter["energy_groups"]
        rates = [_finite(g["photons_per_s"], "group photon rate", minimum=0) for g in groups]
        energies = [_finite(g["energy_ev"], "photon group energy", minimum=0) for g in groups]
        if not groups or any(rate <= 0 for rate in rates) or not math.isclose(math.fsum(rates), total, rel_tol=1e-8, abs_tol=1e-12):
            raise ValueError("emitter energy groups must conserve its absolute photons/s strength")
        lower, upper = regular_mesh_voxel_bounds(mesh, int(emitter["mesh_element"]))
        source = openmc.IndependentSource(
            space=openmc.stats.Box(lower, upper),
            angle=openmc.stats.Isotropic(),
            energy=openmc.stats.Discrete(energies, [rate / total for rate in rates]),
            particle="photon",
            strength=total,
            constraints={"domains": [material], "rejection_strategy": "resample"},
        )
        sources.append(source)
    if not sources:
        raise ValueError("no positive ACTINV decay-photon emission sources are available")
    return sources
