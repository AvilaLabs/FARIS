#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""R3, mesh-based R2S (openmc.deplete.R2SManager) on RM-M, OpenMC 0.15.3.

    r3_r2s.py --model-dir D --out-dir R [--neutron-particles N --neutron-batches B --photon-particles N --photon-budget S]

Steps run one at a time with their own timings: neutron transport (flux and microscopic cross sections per mesh
element and material), activation (1 full-power year at 525 MW, then decay), and photon transport at cooling times of
1 d, 7 d and 30 d. The chain is ~/nuclear-data/p32-work/chain/depletion/chain.xml.

Workarounds OpenMC 0.15.3 needs (recorded in the result): output_dir is a pathlib.Path (the manager divides it with `/`),
micro_kwargs carries an explicit nuclide list, and the photon model's source_rejection_fraction is 1e-4 (the
material-constrained mesh sources reject most samples).

The prospective full-emission reference activates the complete H5M vertex bounding box on a separate coarse mesh;
the R3 response mesh remains fixed for dose scoring. The starting activation cell size is not a source-resolution
adequacy claim and requires discretization-convergence checks. The dose tally is volume-integrated track length times the ICRP-116 AP photon effective dose coefficient. OpenMC
multiplies fixed-source tallies by the total source strength, so with the photon source in photons/s the tally is
pSv.cm3 per second. The photon Monte Carlo standard error is computed from per-batch aggregate totals; it is
conditional on the generated decay-photon source and does not propagate uncertainty from neutron transport or
activation.
"""
from __future__ import annotations

import argparse
import itertools
import json
import math
import os
from pathlib import Path
import resource
import shutil
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import r3_activation_coverage as AC  # noqa: E402
import r3_dose_statistics as DS  # noqa: E402
import r3_shutdown_dose_export as DX  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402
import r3_bounded_microxs as BM  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
CE_DATA_LOWER_EDGE_EV = 1.0e-5
CE_DATA_DIAGNOSTIC_EDGES_EV = (0.0, CE_DATA_LOWER_EDGE_EV, 5.0e7)
CE_DATA_DIAGNOSTIC_MAX_BINS = 8192
CE_DATA_DIAGNOSTIC_BYTES_PER_BIN = 24


def scoring_mesh(openmc):
    mesh = openmc.RegularMesh(name="r3-scored-region")
    mesh.lower_left = sc.MESH_LOWER_CM
    mesh.upper_right = sc.MESH_UPPER_CM
    mesh.dimension = sc.MESH_DIMENSION
    return mesh


def dose_tally(openmc, mesh, name="photon_dose"):
    energies, coeffs = openmc.data.dose_coefficients("photon", "AP")
    tally = openmc.Tally(name=name)
    tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                     openmc.EnergyFunctionFilter(energies, coeffs, interpolation="log-log")]
    tally.scores = ["flux"]
    return tally


def model_nuclides(openmc, model) -> list[str]:
    return sorted({n for m in model.materials for n in m.get_nuclides()})


def below_data_edge_diagnostic_plan(openmc, model, cross_sections_xml: Path = RM.FARIS_XS) -> tuple[list, dict]:
    """Plan same-solve tallies for flux and MT102 response below the CE data grid."""
    import h5py

    materials = sorted(list(model.materials), key=lambda m: (str(m.name), int(m.id)))
    if not materials or len({int(m.id) for m in materials}) != len(materials):
        raise ValueError("below-data-edge diagnostic requires nonempty materials with unique IDs")
    if len({str(m.name) for m in materials}) != len(materials):
        raise ValueError("below-data-edge diagnostic requires unique material names")
    index = AC._cross_section_index(Path(cross_sections_xml).resolve(strict=True))
    rows = []
    captures = []
    data_files = {}
    for material in materials:
        nuclides = sorted(set(str(n) for n in material.get_nuclides()))
        for nuclide in nuclides:
            path = index.get(nuclide)
            if path is None or not path.is_file():
                raise ValueError(f"below-data-edge diagnostic has no indexed CE data for {nuclide}")
            with h5py.File(path, "r") as handle:
                temp_name = BM._nearest_temperature(handle, nuclide, RM.DATA_TEMPERATURE_K, 1.0)
                edge = float(handle[f"{nuclide}/energy/{temp_name}"][0])
                if f"{nuclide}/reactions/reaction_102/{temp_name}/xs" not in handle:
                    raise ValueError(f"{nuclide} has no MT102 data for the same-solve below-data-edge diagnostic")
            data_files[nuclide] = {"path": str(path), "sha256": AC.sha256_file(path)}
            if not math.isclose(edge, CE_DATA_LOWER_EDGE_EV, rel_tol=0.0, abs_tol=1.0e-14):
                raise ValueError(f"{nuclide} CE data lower edge {edge:.17g} eV differs from pinned diagnostic edge {CE_DATA_LOWER_EDGE_EV:.17g} eV")
        row = {"name": str(material.name), "id": int(material.id), "nuclides": nuclides}
        rows.append(row)
        if nuclides:
            tally = openmc.Tally(name=f"r3_below_data_capture_{int(material.id)}")
            tally.filters = [openmc.MaterialFilter([material]), openmc.EnergyFilter(CE_DATA_DIAGNOSTIC_EDGES_EV)]
            tally.nuclides = nuclides
            tally.scores = ["102"]
            captures.append(tally)
    flux = openmc.Tally(name="r3_below_data_material_flux")
    flux.filters = [openmc.MaterialFilter(materials), openmc.EnergyFilter(CE_DATA_DIAGNOSTIC_EDGES_EV)]
    flux.nuclides = ["total"]
    flux.scores = ["flux"]
    bins = len(materials) * 2 + sum(len(row["nuclides"]) * 2 for row in rows)
    estimate = bins * CE_DATA_DIAGNOSTIC_BYTES_PER_BIN
    if bins > CE_DATA_DIAGNOSTIC_MAX_BINS or estimate > 256 * 1024:
        raise ValueError(f"below-data-edge diagnostic tally estimate exceeds its bound: {bins} bins, {estimate} bytes")
    plan = {"schema": "faris-r3-below-data-edge-diagnostic/v1",
            "purpose": "same-solve CE flux and MT102 capture below the first pointwise data edge; diagnostic only",
            "energy_edges_eV": list(CE_DATA_DIAGNOSTIC_EDGES_EV),
            "energy_bins": ["below CE data edge: 0-1e-5 eV", "at/above CE data edge: 1e-5-50 MeV"],
            "ce_data_lower_edge_eV": CE_DATA_LOWER_EDGE_EV,
            "transport_cutoff_eV": float((model.settings.cutoff or {}).get("energy_neutron", 0.0)),
            "reaction_mt": 102, "reaction": "(n,gamma)", "materials": rows,
            "ce_nuclide_data_files": data_files,
            "flux_tally": flux.name, "capture_tallies": [t.name for t in captures],
            "capture_result_layout": "OpenMC C API flattened filter bins in Energy x Nuclide x Score order; two energy bins per material-nuclide",
            "group_mean_xs_policy": "pointwise collapse clips outside the tabulated interval; below-data-edge group mean is zero",
            "activation_response_below_data_edge": "unresolved until this same-solve CE MT102 diagnostic is measured; no chain-completeness claim",
            "interpretation": "CE below-edge captures use OpenMC's first-segment extrapolation; quantify separately from the bounded activation collapse",
            "tally_bin_count_estimate": bins, "tally_bytes_estimate": estimate,
            "tally_bytes_per_bin_estimate": CE_DATA_DIAGNOSTIC_BYTES_PER_BIN}
    if plan["transport_cutoff_eV"] != 0.0:
        raise ValueError("below-data-edge diagnostic requires the unchanged default zero neutron energy cutoff")
    plan["configuration_sha256"] = _canonical_sha256(plan)
    return [flux, *captures], plan


def _canonical_sha256(value: dict) -> str:
    import hashlib
    raw = json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def _paired_energy_fraction(low_batches: list[float], high_batches: list[float]) -> dict:
    if len(low_batches) < 2 or len(low_batches) != len(high_batches):
        return {"fraction": None, "standard_error": None, "status": "UNAVAILABLE",
                "reason": "paired batch arrays are missing or have mismatched lengths"}
    low = [float(x) for x in low_batches]
    high = [float(x) for x in high_batches]
    if any(not math.isfinite(x) or x < 0.0 for x in low + high):
        return {"fraction": None, "standard_error": None, "status": "UNAVAILABLE",
                "reason": "paired batch scores contain non-finite or negative values"}
    low_total, high_total = math.fsum(low), math.fsum(high)
    den = low_total + high_total
    if not math.isfinite(den) or den <= 0.0:
        return {"fraction": None, "standard_error": None, "status": "UNAVAILABLE",
                "reason": "no positive tallied score in either energy bin"}
    if low_total == 0.0:
        return {"fraction": 0.0, "standard_error": None,
                "status": "UNRESOLVED_ZERO_OBSERVED_LOW_BIN",
                "reason": "zero observed score below the data edge does not bound an unobserved low-energy tail",
                "uncertainty_scope": "uncertainty unavailable for an empty energy bin"}
    if high_total == 0.0:
        return {"fraction": 1.0, "standard_error": None,
                "status": "UNRESOLVED_ZERO_OBSERVED_HIGH_BIN",
                "reason": "zero observed score at or above the data edge leaves the ratio denominator's high-energy contribution unresolved",
                "uncertainty_scope": "uncertainty unavailable for an empty energy bin"}
    fraction = low_total / den
    residual = [a - fraction * (a + b) for a, b in zip(low, high)]
    mean_total = den / len(low)
    mean_residual = math.fsum(residual) / len(residual)
    variance = math.fsum((x - mean_residual) ** 2 for x in residual) / (len(residual) - 1)
    return {"fraction": fraction, "standard_error": math.sqrt(variance / len(residual)) / mean_total,
            "status": "AVAILABLE", "uncertainty_scope": "paired batch delta-method SE; shared-history covariance retained"}


def summarize_below_data_edge_batches(plan: dict, cumulative_by_tally: dict[str, list[list[float]]],
                                      batches: int, particles_per_batch: int) -> dict:
    """Retain batch results and calculate paired below/above flux and capture fractions."""
    ids = plan.get("tally_ids", {})
    flux_key = str(ids.get(plan["flux_tally"], ""))
    flux_rows = cumulative_by_tally.get(flux_key, [])
    if len(flux_rows) != batches or any(len(row) != 2 * len(plan["materials"]) for row in flux_rows):
        raise ValueError("below-data-edge flux diagnostic batch count or material-energy shape mismatch")

    def increments(rows, width, label):
        if len(rows) != batches or any(len(row) != width for row in rows):
            raise ValueError(f"below-data-edge {label} batch count or result shape mismatch")
        values = [[float(value) for value in row] for row in rows]
        if any(not math.isfinite(value) or value < 0.0 for row in values for value in row):
            raise ValueError(f"below-data-edge {label} has non-finite or negative cumulative scores")
        prior = [0.0] * width
        by_bin = [[] for _ in range(width)]
        for row in values:
            delta = [value - old for value, old in zip(row, prior)]
            if any(not math.isfinite(value) or value < 0.0 for value in delta):
                raise ValueError(f"below-data-edge {label} has invalid batch increments")
            for index, value in enumerate(delta):
                by_bin[index].append(value)
            prior = row
        return values, by_bin

    flux_cumulative, flux_deltas = increments(flux_rows, 2 * len(plan["materials"]), "flux")
    materials = []
    for i, material in enumerate(plan["materials"]):
        low, high = flux_deltas[2 * i], flux_deltas[2 * i + 1]
        capture_key = str(ids.get(f"r3_below_data_capture_{material['id']}", ""))
        nuclides = material["nuclides"]
        cap_rows = cumulative_by_tally.get(capture_key, [])
        if nuclides:
            cap_cumulative, cap_deltas = increments(cap_rows, 2 * len(nuclides), f"MT102 {material['name']}")
        else:
            cap_cumulative, cap_deltas = [], []
        captures = []
        for j, nuclide in enumerate(nuclides):
            # OpenMC's flattened filter/nuclide axes are Energy x Nuclide x Score.
            below, above = cap_deltas[j], cap_deltas[len(nuclides) + j]
            captures.append({"nuclide": nuclide,
                             "below_vs_tallied_capture_fraction": _paired_energy_fraction(below, above),
                             "raw_batch_increments_below_and_above": {"below": below, "above": above}})
        materials.append({"name": material["name"], "id": material["id"],
                          "flux_below_vs_tallied_fraction": _paired_energy_fraction(low, high),
                          "raw_batch_flux_increments_below_and_above": {"below": low, "above": high},
                          "captures_mt102": captures,
                          "raw_batch_capture_cumulative_flattened": cap_cumulative})
    return {"plan": plan, "batches": batches, "particles_per_batch": particles_per_batch,
            "histories": batches * particles_per_batch, "materials": materials,
            "raw_batch_flux_cumulative_flattened": flux_cumulative,
            "raw_batch_cumulative_by_tally": cumulative_by_tally,
            "uncertainty_scope": "paired batch delta-method SE; shared-history covariance retained where available"}


def build_models(openmc, model_dir: Path, mesh, n_particles, n_batches, p_particles, p_batches, seeds):
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    neutron, dag = RM.build_model(openmc, model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = neutron.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = n_batches, n_particles, seeds[0]
    s.source = [source]
    s.photon_transport = False
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials)
    ps = openmc.Settings()
    ps.run_mode = "fixed source"
    ps.batches, ps.particles, ps.seed = p_batches, p_particles, seeds[1]
    ps.photon_transport = True
    ps.source_rejection_fraction = 1.0e-4
    ps.temperature = dict(s.temperature)
    ps.output = {"summary": False, "tallies": False}
    photon.settings = ps
    photon.tallies = openmc.Tallies([dose_tally(openmc, mesh)])
    return neutron, photon, ids, dag


def activation_domain(openmc, model_dir: Path, dag, cell_cm: float,
                      energy_group_count: int = sc.ACTIVATION_GROUP_COUNT) -> tuple[object, dict]:
    """Build a coarse activation mesh from verified H5M vertex bounds and its pinned model card."""
    import hashlib

    h5m_path = (model_dir / "rm_m.h5m").resolve()
    card_path = (model_dir / "rm_m_model_card.json").resolve()
    card_bytes = card_path.read_bytes()
    card = json.loads(card_bytes)
    if Path(dag.filename).resolve() != h5m_path:
        raise ValueError("DAGMC universe is not bound to the model-dir H5M whose vertices are being recorded")
    h5m_hash = RM.sha256(h5m_path)
    card_h5m_hash = card.get("outputs", {}).get("h5m_sha256")
    if card.get("schema") != "faris-rm-m-model-card/v1" or card_h5m_hash != h5m_hash:
        raise ValueError("activation bounds require the model card pinned to this exact RM-M H5M")
    bounding_radius = float(card.get("bounding_sphere_radius", {}).get("value", 0.0))
    if not math.isfinite(bounding_radius) or bounding_radius <= 0.0:
        raise ValueError("RM-M model card has no valid authored bounding-sphere radius")
    vertices = DX._h5m_vertex_bounds(h5m_path)
    actual_box = dag.bounding_box
    lower = tuple(map(float, actual_box.lower_left))
    upper = tuple(map(float, actual_box.upper_right))
    if list(lower) != vertices["lower_left_cm"] or list(upper) != vertices["upper_right_cm"]:
        raise ValueError("OpenMC DAGMC bounds differ from the actual H5M node-coordinate bounds")
    if vertices["max_vertex_radius_cm"] > bounding_radius:
        raise ValueError("H5M vertex bounds exceed the model card's authored containment sphere")
    grid = sc.activation_grid_spec(lower, upper, cell_cm, energy_group_count=energy_group_count)
    mesh = openmc.RegularMesh(name="r3-full-geometry-activation")
    mesh.lower_left = tuple(grid["lower_left_cm"])
    mesh.upper_right = tuple(grid["upper_right_cm"])
    mesh.dimension = tuple(grid["dimension"])
    domain = {
        "schema": "faris-r3-activation-domain/v1",
        "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
        "scope": "full H5M vertex bounding box; material-bearing mesh domains are identified by OpenMC material-volume sampling",
        "photon_source_scope": "decay photons from all material-bearing domains identified in the full activation mesh; coarse-cell spatial approximation requires convergence",
        "emission_transport_scope": "generated decay photons and their descendants are transported; all may score in the shared response mesh",
        "source_rate_normalization": "R2SManager MeshSource strengths in photons/s; OpenMC fixed-source tally applies source-total normalization",
        "geometry": {"h5m_path": str(h5m_path), "h5m_sha256": h5m_hash,
                     "model_card_path": str(card_path), "model_card_sha256": hashlib.sha256(card_bytes).hexdigest(),
                     "model_card_bounding_sphere_radius_cm": bounding_radius,
                     "h5m_vertex_lower_left_cm": list(lower), "h5m_vertex_upper_right_cm": list(upper),
                     "h5m_max_vertex_radius_cm": vertices["max_vertex_radius_cm"]},
        "activation_mesh": {**grid, "mesh_xml": DX._mesh_definition(mesh)},
        "response_mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM), "upper_right_cm": list(sc.MESH_UPPER_CM),
                          "dimension": list(sc.MESH_DIMENSION), "voxel_cm": sc.MESH_VOXEL_CM},
        "discretization_limit": "100 cm is a starting mesh only; no activation-source resolution adequacy is claimed",
    }
    return mesh, domain


def material_volume_sampling_model(openmc, transport_model, dag, mesh, clearance_cm: float = 100.0):
    """Make an auxiliary bounded void world only when the activation mesh exceeds the transport vacuum sphere.

    OpenMC's mesh material-volume estimator requires the complete mesh to be inside the geometry's
    root universe. The R3 full-H5M mesh is aligned outward to whole cells and may extend past the
    unchanged transport vacuum sphere even though the extra mesh cells contain no DAGMC material.
    This auxiliary model reuses the same DAGMC universe and materials, enlarges only the vacuum
    sampling boundary for geometric sampling; neutron/photon transport always exports and uses
    the original model geometry.
    """
    if not math.isfinite(float(clearance_cm)) or clearance_cm <= 0.0:
        raise ValueError("auxiliary mesh sampling world requires a positive finite void clearance")
    surfaces = transport_model.geometry.get_all_surfaces()
    source_surface = surfaces.get(RM.SPHERE_SURFACE_ID)
    if not isinstance(source_surface, openmc.Sphere) or source_surface.boundary_type != "vacuum":
        raise ValueError("R3 transport model does not contain its expected vacuum bounding sphere")
    lower = tuple(float(x) for x in mesh.lower_left)
    upper = tuple(float(x) for x in mesh.upper_right)
    if len(lower) != 3 or len(upper) != 3 or any(not math.isfinite(x) for x in (*lower, *upper)):
        raise ValueError("activation mesh has invalid bounds for auxiliary material-volume sampling")
    if any(upper[i] <= lower[i] for i in range(3)):
        raise ValueError("activation mesh bounds are not strictly increasing")
    corner_radius = max(math.sqrt(sum(c[i] * c[i] for i in range(3)))
                        for c in itertools.product(*[(lower[i], upper[i]) for i in range(3)]))
    required_radius = corner_radius + float(clearance_cm)
    if required_radius <= float(source_surface.r):
        return transport_model, {
            "kind": "original transport geometry",
            "used_only_for": "Mesh.material_volumes geometric sampling",
            "transport_model_geometry_changed": False,
            "transport_vacuum_sphere_radius_cm": float(source_surface.r),
            "mesh_corner_radius_cm": corner_radius,
            "sampling_vacuum_sphere_radius_cm": float(source_surface.r),
        }

    sampling_sphere = openmc.Sphere(
        surface_id=RM.SPHERE_SURFACE_ID + 1, r=required_radius, boundary_type="vacuum")
    sampling_cell = openmc.Cell(cell_id=10001, fill=dag, region=-sampling_sphere)
    sampling_model = openmc.Model(
        geometry=openmc.Geometry([sampling_cell]), materials=transport_model.materials)
    sampling_model.settings = transport_model.settings
    return sampling_model, {
        "kind": "auxiliary DAGMC material-volume sampling world",
        "used_only_for": "Mesh.material_volumes geometric sampling",
        "transport_model_geometry_changed": False,
        "transport_vacuum_sphere_radius_cm": float(source_surface.r),
        "sampling_vacuum_sphere_radius_cm": required_radius,
        "mesh_corner_radius_cm": corner_radius,
        "void_clearance_beyond_mesh_cm": required_radius - corner_radius,
        "same_dagmc_universe": True,
        "same_material_collection": True,
        "neutron_photon_transport_model_retains_original_geometry": True,
    }


def _update_reused_elapsed_summary(record: dict) -> None:
    """Keep imported transport cost visible and count it once in the stage total."""
    original = record.get("reused_neutron_original_elapsed_seconds")
    if not isinstance(original, (int, float)) or not math.isfinite(original) or original <= 0.0:
        return
    steps = record.get("steps", {})
    validation_collapse = steps.get("neutron_transport", {}).get("seconds")
    record["reused_neutron_validation_and_collapse_elapsed_seconds"] = validation_collapse
    completed_seconds = [float(step["seconds"]) for step in steps.values()
                         if isinstance(step, dict) and isinstance(step.get("seconds"), (int, float))
                         and math.isfinite(step["seconds"]) and step["seconds"] >= 0.0]
    record["method_elapsed_seconds_including_reused_neutron"] = float(original) + sum(completed_seconds)
    record["method_elapsed_scope"] = (
        "sum of recorded R2S step wall times plus original neutron transport wall time exactly once; "
        "reused-neutron step time covers validation and new collapse only")


def timed(record, name, fn):
    t0 = time.time()
    entry = {"ok": False, "seconds": None, "peak_rss_mb": None, "error": None}
    record["steps"][name] = entry
    try:
        fn()
        entry["ok"] = True
    except Exception:
        import traceback
        entry["error"] = traceback.format_exc(limit=8)
    finally:
        entry["seconds"] = time.time() - t0
        entry["peak_rss_mb"] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                                   resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0
    return entry["ok"]


def mark_neutron_only_complete(record: dict) -> dict:
    """Mark an intentional neutron-stage-only run without implying R2S dose results."""
    record["workflow_status"] = "NEUTRON_STAGE_COMPLETE"
    record["scientific_interpretation_status"] = "NOT_EVALUATED"
    record["activation_status"] = "NOT_RUN"
    record["microxs_status"] = "NOT_RUN"
    record["photon_transport_status"] = "NOT_RUN"
    record["shutdown_dose_status"] = "NOT_RUN"
    record["qualification_verdict"] = (
        "NOT_PROVIDED: neutron flux and optional below-data-edge diagnostic only; "
        "no activation, photon transport, shutdown dose, or qualification result")
    return record


def tree_file_bytes(root: Path) -> int:
    return sum(path.stat().st_size for path in Path(root).rglob("*") if path.is_file())


def inventory_emission(openmc, chain, mgr, time_index):
    """Photon emission of the activated inventory, from the chain's decay data and the nuclide atoms.

    This is a different path from the manager's source (Material.get_decay_photon_energy with clipping): atoms times the
    per-atom photon emission rate, summed over every activation material, and the same sum by nuclide as photon
    power in MeV/s for the nuclide ranking.
    """
    import numpy as np

    results = mgr.results["depletion_results"]
    per_decay = {}
    for nuc in chain.nuclides:
        src = nuc.sources.get("photon") if nuc.sources else None
        if src is None:
            continue
        x, p = np.asarray(src.x, dtype=float), np.asarray(src.p, dtype=float)
        # the chain stores the photon spectrum per atom per second (the decay constant is already in it)
        per_decay[nuc.name] = (float(p.sum()), float((x * p).sum()) / 1.0e6)
    total = 0.0
    power = {}
    for mat in mgr.results["activation_materials"]:
        for name, atoms in results[time_index].get_material(str(mat.id)).get_nuclide_atoms().items():
            if name in per_decay and atoms > 0.0:
                n_ph, mev = per_decay[name]
                total += atoms * n_ph
                power[name] = power.get(name, 0.0) + atoms * mev
    return total, power


def inventory_emission_chunked(openmc, chain, activation_plan_path: Path,
                               activation_record_path: Path, time_index: int):
    """Read the all-domain inventory/emission sums extracted before chunk retirement."""
    plan = json.loads(Path(activation_plan_path).read_text(encoding="utf-8"))
    record = json.loads(Path(activation_record_path).read_text(encoding="utf-8"))
    if record.get("plan_sha256") != plan.get("plan_sha256") or record.get("status") != "PASS":
        raise ValueError("inventory cross-check requires a completed activation record bound to its plan")
    BM.verify_activation_microxs_receipts(activation_plan_path, activation_record_path)
    summaries = record.get("time_summaries", [])
    if not 0 <= int(time_index) < len(summaries):
        raise ValueError("activation inventory summary lacks the requested cooling state")
    summary = summaries[int(time_index)]
    return (float(summary["inventory_photons_per_s"]),
            {str(name): float(value) for name, value in summary["power_mev_per_s_by_nuclide"].items()},
            float(summary["unclipped_photons_per_s"]))


def source_strength(sources) -> float:
    total = 0.0
    for ms in sources:
        for src in ms.sources.ravel():
            total += float(src.strength)
    return total


def sampling_settings(args) -> dict:
    """The producer's recorded sampling and irradiation contract, also used by boundary fixtures."""
    return {
        "neutron_particles_per_batch": args.neutron_particles,
        "neutron_batches": args.neutron_batches,
        "photon_particles_per_batch": args.photon_particles,
        "photon_max_batches": args.photon_max_batches,
        "photon_budget_s": args.photon_budget,
        "seeds": [args.seed, args.seed + 1],
        "activation_groups": getattr(args, "activation_groups", sc.ACTIVATION_GROUPS),
        "below_data_edge_diagnostic": getattr(args, "below_data_edge_diagnostic", False),
        "stop_after_neutron": getattr(args, "stop_after_neutron", False),
        "activation_cell_cm": args.activation_cell_cm,
        "mat_vol_samples": args.mat_vol_samples,
        "timesteps": sc.TIMESTEPS,
        "source_rate_n_s": sc.SOURCE_RATES[0],
        "source_rates_n_s": sc.SOURCE_RATES,
    }


def _resolve_record_ref(ref: dict, base: Path, label: str) -> tuple[Path, str]:
    if not isinstance(ref, dict) or not isinstance(ref.get("path"), str) or not isinstance(ref.get("sha256"), str):
        raise ValueError(f"reused neutron receipt lacks a valid {label} path/hash")
    raw = Path(ref["path"])
    path = raw.resolve() if raw.is_absolute() else (base / raw).resolve()
    if not path.is_file() or RM.sha256(path) != ref["sha256"]:
        raise ValueError(f"reused neutron {label} is missing or its SHA-256 changed: {path}")
    return path, ref["sha256"]


def _validate_neutron_reuse_source(record_path: Path, *, expected_settings: dict,
                                   expected_groups: dict, expected_chain: Path,
                                   expected_cross_sections: Path, current_basis: dict,
                                   current_materials_sha256: str, expected_runtime: dict,
                                   expected_threads: int, expected_resource_limits: dict) -> dict:
    """Validate a failed R2S record's neutron-stage inputs for explicit reuse.

    This imports only neutron transport evidence. It deliberately ignores any
    partial MicroXS file and requires a new collapse plan/output directory.
    """
    record_path = Path(record_path).resolve(strict=True)
    source_run = record_path.parent
    record = json.loads(record_path.read_text(encoding="utf-8"))
    if record.get("kind") != "r3-r2s" or record.get("source_problem_protocol") != sc.FULL_EMISSION_PROTOCOL:
        raise ValueError("reuse source is not the same full-emission R2S protocol")
    if record.get("settings") != expected_settings:
        raise ValueError("reused neutron settings differ from the requested R2S settings")
    if record.get("activation_group_basis", {}).get("energy_edges_eV") != expected_groups.get("energy_edges_eV") or \
            record.get("settings", {}).get("activation_groups") != expected_settings.get("activation_groups"):
        raise ValueError("reused neutron activation group structure differs from the request")
    if record.get("runtime_identity") != expected_runtime or record.get("threads") != expected_threads:
        raise ValueError("reused neutron OpenMC runtime identity or thread count differs from the current run")
    if record.get("library", {}).get("cross_sections_xml") != str(expected_cross_sections.resolve()) or \
            record.get("library", {}).get("sha256") != RM.sha256(expected_cross_sections):
        raise ValueError("reused neutron cross-section library identity differs from the current library")
    if record.get("chain") != str(expected_chain.resolve()) or record.get("chain_sha256") != RM.sha256(expected_chain):
        raise ValueError("reused neutron depletion chain differs from the current chain")
    if record.get("steps", {}).get("neutron_transport", {}).get("ok") is not False:
        raise ValueError("reuse source must be an incomplete R2S run with no completed MicroXS/activation")
    if "bounded_activation" in record or record.get("photon"):
        raise ValueError("reuse source already contains downstream activation or photon results")
    if record.get("activation_microxs", {}).get("status") not in (None, "NOT_RUN"):
        raise ValueError("reuse source contains a MicroXS result; only neutron transport may be imported")

    # The source must have failed after neutron transport, at the old collapse.
    error = record["steps"]["neutron_transport"].get("error") or ""
    if "collapsed group means are invalid" not in error:
        raise ValueError("reuse source did not fail at the known bounded-collapse stage")
    integrity = record.get("neutron_transport_integrity", {})
    assessment = integrity.get("lost_particle_assessment", {})
    original_elapsed = integrity.get("elapsed_s")
    if (integrity.get("status") != "PASS" or assessment.get("status") != "PASS" or
            assessment.get("lost_particles") != 0 or assessment.get("histories", 0) <= 0 or
            integrity.get("histories") != assessment.get("histories") or
            not isinstance(original_elapsed, (int, float)) or not math.isfinite(original_elapsed) or original_elapsed <= 0.0):
        raise ValueError("reused neutron transport lacks a passing, zero-loss integrity receipt")
    if integrity.get("model_xml_sha256") != integrity.get("solver_model", {}).get("sha256"):
        raise ValueError("neutron model XML hash is not bound consistently in the integrity receipt")

    basis = record.get("shutdown_dose_basis", {})
    for name, current_ref in current_basis.items():
        old_ref = basis.get(name)
        if not isinstance(old_ref, dict) or old_ref.get("sha256") != current_ref.get("sha256"):
            raise ValueError(f"reused neutron physical source basis differs: {name}")
        _resolve_record_ref(old_ref, source_run, f"source basis {name}")

    plan_ref = record.get("bounded_protocol_plan", {})
    protocol_path, protocol_file_sha = _resolve_record_ref(plan_ref, source_run, "bounded protocol plan")
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol_body = dict(protocol)
    protocol_digest = protocol_body.pop("protocol_sha256", None)
    if (protocol.get("schema") != "faris-r3-bounded-campaign-protocol/v1" or
            protocol_digest != BM._canonical_sha256(protocol_body) or
            protocol_digest != plan_ref.get("protocol_sha256") or
            protocol.get("sampling_settings") != expected_settings):
        raise ValueError("reused neutron protocol plan does not match its recorded settings/hash")
    mesh_ref = protocol.get("activation_mesh", {})
    mesh_path, mesh_sha = _resolve_record_ref(mesh_ref, source_run, "activation mesh")
    if (mesh_sha != record.get("activation_domain", {}).get("sha256") or
            mesh_path != (source_run / record.get("activation_domain", {}).get("path", "")).resolve()):
        raise ValueError("reused neutron activation mesh path/hash is inconsistent")
    input_refs = protocol.get("inputs", {})
    model_path, model_sha = _resolve_record_ref(input_refs.get("model_xml", {}), protocol_path.parent, "model XML")
    if model_sha != integrity["model_xml_sha256"]:
        raise ValueError("reused neutron model XML does not match transport integrity")
    materials_path, materials_sha = _resolve_record_ref(input_refs.get("materials_xml", {}), protocol_path.parent, "materials XML")
    record_materials_path, record_materials_sha = _resolve_record_ref(
        record.get("neutron_materials_xml", {}), source_run, "recorded materials XML")
    if record_materials_path != materials_path or record_materials_sha != materials_sha:
        raise ValueError("neutron materials XML does not match the R2S record receipt")
    if materials_sha != current_materials_sha256:
        raise ValueError("reused neutron materials differ from the freshly built model")
    volume_path, volume_sha = _resolve_record_ref(input_refs.get("mesh_material_volumes", {}), source_run, "mesh-material volumes")
    volume_receipt = integrity.get("mesh_material_volumes", {})
    if volume_sha != volume_receipt.get("sha256"):
        raise ValueError("reused neutron volume map is not bound to the integrity receipt")
    flux_path = (protocol_path.parent / "fluxes.npy").resolve()
    flux_std_path = (protocol_path.parent / "fluxes_std_dev.npy").resolve()
    if (not flux_path.is_file() or RM.sha256(flux_path) != integrity.get("flux_mean_sha256") or
            not flux_std_path.is_file() or RM.sha256(flux_std_path) != integrity.get("flux_std_dev_sha256")):
        raise ValueError("reused neutron mean/std-dev flux arrays are missing or changed")
    for label, expected in (("chain", expected_chain), ("cross_sections_xml", expected_cross_sections)):
        path, digest = _resolve_record_ref(input_refs.get(label, {}), protocol_path.parent, label)
        if path != expected.resolve() or digest != RM.sha256(expected):
            raise ValueError(f"reused neutron {label} data differ from the current run")
    if protocol.get("fixed_protocol", {}).get("energy_edges_eV") != expected_groups.get("energy_edges_eV"):
        raise ValueError("reused neutron energy group edges differ from the current run")

    # Bind the obsolete collapse plan as evidence only; its HDF output is never read.
    old_plan_path = protocol_path.parent / "bounded-collapse-plan.json"
    old_plan = json.loads(old_plan_path.read_text(encoding="utf-8"))
    old_plan_digest = old_plan.get("plan_sha256")
    old_plan_body = dict(old_plan)
    old_plan_body.pop("plan_sha256", None)
    if old_plan_digest != BM._canonical_sha256(old_plan_body):
        raise ValueError("reused neutron source's original collapse plan checksum is invalid")
    old_plan_inputs = old_plan.get("inputs", {})
    for name, path, digest in (("fluxes", flux_path, integrity["flux_mean_sha256"]),
                               ("volumes", volume_path, volume_sha),
                               ("materials", materials_path, materials_sha)):
        ref = old_plan_inputs.get(name, {})
        plan_path, plan_sha = _resolve_record_ref(ref, protocol_path.parent, f"original collapse {name}")
        if plan_path != path or plan_sha != digest:
            raise ValueError(f"original collapse plan has different neutron-stage {name} input")
    coverage = record.get("activation_chain_coverage", {})
    if coverage.get("status") != "READY_FOR_R2S_TRANSPORT" or coverage.get("chain_xml_sha256") != RM.sha256(expected_chain):
        raise ValueError("reused neutron coverage receipt is not complete for the pinned chain")
    if old_plan.get("required_nuclide_hdf5") != coverage.get("indexed_data_files"):
        raise ValueError("original collapse plan HDF5 manifest differs from the neutron run's coverage receipt")
    preflight = record.get("bounded_resource_preflight", {})
    if record.get("bounded_resources") != expected_resource_limits:
        raise ValueError("reused neutron activation/collapse resource settings differ from the current request")
    preflight_limits = {
        "output_budget_bytes": expected_resource_limits.get("collapse_activation_output_budget_bytes"),
        "disk_reserve_bytes": expected_resource_limits.get("disk_reserve_bytes"),
        "memory_budget_bytes": expected_resource_limits.get("working_memory_budget_bytes"),
    }
    for key, value in preflight_limits.items():
        if preflight.get(key) != value:
            raise ValueError(f"reused neutron bounded resource preflight differs: {key}")
    return {"record_path": str(record_path), "record_sha256": RM.sha256(record_path),
            "protocol_path": str(protocol_path), "protocol_sha256": protocol_file_sha,
            "protocol_identity_sha256": protocol["protocol_sha256"],
            "old_collapse_plan_path": str(old_plan_path), "old_collapse_plan_sha256": old_plan_digest,
            "old_collapse_partial_output_ignored": True, "model_xml": model_path,
            "materials_xml": materials_path, "volume_path": volume_path, "flux_path": flux_path,
            "flux_std_path": flux_std_path, "volume_sha256": volume_sha,
            "coverage_indexed_data_files": coverage.get("indexed_data_files", []),
            "resource_preflight": preflight, "integrity": integrity,
            "original_neutron_elapsed_seconds": float(original_elapsed)}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--neutron-particles", type=int, default=20000)
    ap.add_argument("--neutron-batches", type=int, default=50)
    ap.add_argument("--photon-particles", type=int, default=100000)
    ap.add_argument("--photon-max-batches", type=int, default=5000)
    ap.add_argument("--photon-budget", type=float, default=3000.0, help="wall seconds per photon-transport run")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--mat-vol-samples", type=int, default=50000)
    ap.add_argument("--activation-cell-cm", type=float, default=sc.DEFAULT_ACTIVATION_CELL_CM,
                    help="coarse full-H5M activation mesh cell size; starting discretization, not adequacy claim")
    ap.add_argument("--activation-groups", choices=sc.ACTIVATION_GROUP_OPTIONS, default=sc.ACTIVATION_GROUPS,
                    help="pinned activation energy basis; historical VITAMIN-J-42 remains the default")
    ap.add_argument("--below-data-edge-diagnostic", action="store_true", default=False,
                    help="add same-solve 0-1e-5 eV material flux and per-nuclide MT102 capture tallies to bounded neutron transport")
    ap.add_argument("--reuse-neutron-from", type=Path, default=None,
                    help="validated failed R2S record whose unchanged neutron transport feeds a fresh collapse/activation run")
    ap.add_argument("--stop-after-neutron", action="store_true", default=False,
                    help="stop after saving the bounded neutron flux, diagnostic, and protocol artifacts; do not run activation or photon dose")
    ap.add_argument("--activation-backend", choices=("bounded", "direct"), default="bounded",
                    help="bounded streams group-mean XS and activates domain chunks; direct retains R2SManager dense arrays")
    ap.add_argument("--microxs-output-budget-gib", type=float, default=8.0,
                    help="explicit retained intermediate budget for bounded collapse")
    ap.add_argument("--disk-reserve-gib", type=float, default=8.0,
                    help="free disk space that must remain after bounded artifacts are written")
    ap.add_argument("--microxs-memory-budget-mib", type=float, default=512.0,
                    help="peak working-set budget for bounded pointwise XS collapse")
    ap.add_argument("--activation-chunk-domains", type=int, default=8,
                    help="number of mesh-material activation domains solved per depletion chunk")
    ap.add_argument("--activation-time-budget", type=float, default=86400.0,
                    help="maximum elapsed seconds checked between bounded depletion chunks")
    ap.add_argument("--capi-domain-index", type=int,
                    help="domain row used by the optional post-run bounded CAPI comparison (default: first positive-flux row)")
    ap.add_argument("--skip-capi-check", action="store_true",
                    help="skip the bounded numerical CAPI sample check (not recommended for a reference run)")
    ap.add_argument("--seed", type=int, default=20261201)
    ap.add_argument("--photon-indices", type=int, nargs="+", default=list(sc.PHOTON_TIME_INDICES),
                    help="time indices for the photon step (S0b D5c: 2 = 1 d only)")
    args = ap.parse_args()
    args.model_dir = args.model_dir.resolve()
    args.out_dir = args.out_dir.resolve()
    if args.reuse_neutron_from is not None:
        args.reuse_neutron_from = args.reuse_neutron_from.resolve()
    import openmc
    import openmc.deplete
    import numpy as np

    group_edges = sc.activation_group_edges(openmc, args.activation_groups, BM.VITAMIN_J_42_EDGES_E_V)
    group_count = len(group_edges) - 1
    if args.activation_backend == "direct" and args.activation_groups != sc.ACTIVATION_GROUPS:
        raise ValueError("extended activation bases currently require the bounded backend")
    if args.below_data_edge_diagnostic and args.activation_backend != "bounded":
        raise ValueError("below-data-edge diagnostic currently requires the bounded backend")
    if args.stop_after_neutron and args.activation_backend != "bounded":
        raise ValueError("--stop-after-neutron currently requires the bounded backend")
    if args.reuse_neutron_from is not None and (args.activation_backend != "bounded" or args.stop_after_neutron):
        raise ValueError("--reuse-neutron-from requires bounded backend and a full downstream run")

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)  # sub-models built inside the manager do not inherit materials.cross_sections
    out = args.out_dir
    if out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 output directory: {out}")
    out.mkdir(parents=True)
    os.chdir(out)
    mesh = scoring_mesh(openmc)
    neutron, photon, ids, dag = build_models(openmc, args.model_dir, mesh, args.neutron_particles, args.neutron_batches,
                                             args.photon_particles, args.photon_max_batches, (args.seed, args.seed + 1))
    neutron_dir = out / "neutron_transport"
    neutron_dir.mkdir()
    neutron_materials_xml = neutron_dir / "materials.xml"
    neutron.materials.export_to_xml(path=neutron_materials_xml)
    nuclides = model_nuclides(openmc, neutron)
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    coverage = AC.assess_coverage(CHAIN, RM.FARIS_XS, nuclides, openmc.data.REACTION_MT)
    record = {"kind": "r3-r2s", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "activation_backend": args.activation_backend,
              "bounded_resources": {"collapse_activation_output_budget_bytes": int(args.microxs_output_budget_gib * BM.GIB),
                                     "disk_reserve_bytes": int(args.disk_reserve_gib * BM.GIB),
                                     "working_memory_budget_bytes": int(args.microxs_memory_budget_mib * 1024 ** 2),
                                     "activation_domain_chunk": args.activation_chunk_domains,
                                     "activation_elapsed_time_budget_s": args.activation_time_budget},
              "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
              "chain": str(CHAIN), "chain_sha256": RM.sha256(CHAIN), "library": RM.library_identity(),
              "activation_chain_coverage": coverage,
              "activation_data_status": coverage["status"],
              "activation_microxs_requested_nuclides": coverage["required_microxs_nuclides"],
              "neutron_materials_xml": {"path": neutron_materials_xml.relative_to(out).as_posix(),
                                         "sha256": RM.sha256(neutron_materials_xml)},
              "mesh": {"lower_left_cm": sc.MESH_LOWER_CM, "upper_right_cm": sc.MESH_UPPER_CM, "dimension": sc.MESH_DIMENSION},
              "settings": sampling_settings(args),
              "workarounds": ["output_dir passed as pathlib.Path", "micro_kwargs nuclides = model nuclides", "photon source_rejection_fraction = 1e-4",
                            "openmc.config['cross_sections'] set to the audited library (manager sub-models do not inherit it)"],
              "nuclides": nuclides}
    record["activation_group_basis"] = {**sc.ACTIVATION_GROUP_BASIS[args.activation_groups],
                                         "energy_edges_eV": list(group_edges)}
    record["runtime_identity"] = DS.runtime_identity(openmc)
    record["neutron_transport_integrity"] = {
        "status": "NOT_EVALUATED",
        "reason": "R2SManager neutron C API lifecycle has not yet been observed",
        "model_xml_sha256": None,
        "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None, "histories": None},
    }
    jpath = out / "r2s_record.json"

    def save():
        _update_reused_elapsed_summary(record)
        jpath.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")

    if coverage["status"] != "READY_FOR_R2S_TRANSPORT":
        record["steps"]["activation_data_preflight"] = {
            "ok": False, "seconds": 0.0, "peak_rss_mb": None,
            "error": "reachable activation nuclides or chain-listed reaction channels lack indexed OpenMC data; no R2S neutron transport was started",
        }
        save()
        return 2

    activation_mesh, activation_domain_data = activation_domain(openmc, args.model_dir, dag, args.activation_cell_cm,
                                                                group_count)
    material_volume_model, material_volume_world = material_volume_sampling_model(
        openmc, neutron, dag, activation_mesh)
    record["material_volume_sampling_world"] = material_volume_world
    activation_domain_data["activation_group_structure"] = args.activation_groups
    domain_bytes = DX._json_bytes(activation_domain_data)
    record["activation_domain"] = {**DX._write_bytes(out / "activation_domain.json", domain_bytes),
                                    "path": "activation_domain.json"}
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    record["shutdown_dose_basis"] = DX.write_basis_artifacts(
        out, neutron, neutron.settings.source[0], mesh,
        {"timesteps": sc.TIMESTEPS, "source_rates_neutrons_s": sc.SOURCE_RATES,
         "chain_path": str(CHAIN), "chain_sha256": RM.sha256(CHAIN)}, energies, coefficients,
        sc.rm_m_geometry_dependencies(args.model_dir), RM.library_identity())
    save()
    mgr = openmc.deplete.R2SManager(neutron, activation_mesh, photon_model=photon) \
        if args.activation_backend == "direct" else None
    bounded_paths = {}

    def step1_reusing_neutron():
        source_run = args.reuse_neutron_from.resolve().parent
        if out == source_run or source_run in out.parents:
            raise ValueError("reuse output must be a separate fresh directory, not inside the source run")
        source = _validate_neutron_reuse_source(
            args.reuse_neutron_from, expected_settings=record["settings"],
            expected_groups=record["activation_group_basis"], expected_chain=CHAIN,
            expected_cross_sections=RM.FARIS_XS,
            current_basis=record["shutdown_dose_basis"],
            current_materials_sha256=record["neutron_materials_xml"]["sha256"],
            expected_runtime=record["runtime_identity"], expected_threads=args.threads,
            expected_resource_limits=record["bounded_resources"])
        source_record = json.loads(Path(source["record_path"]).read_text(encoding="utf-8"))
        if source_record.get("activation_domain", {}).get("sha256") != record["activation_domain"]["sha256"]:
            raise ValueError("reused neutron activation mesh differs from the current generated mesh")
        if source["resource_preflight"].get("status") != "PASS_WITHIN_DECLARED_BUDGETS":
            raise ValueError("reused neutron run lacks a passing bounded resource preflight")
        record["bounded_resource_preflight"] = dict(source["resource_preflight"])
        record["bounded_resource_preflight"]["available_disk_bytes"] = shutil.disk_usage(neutron_dir).free
        record["reused_neutron_original_elapsed_seconds"] = source["original_neutron_elapsed_seconds"]
        inherited_integrity = dict(source["integrity"])
        inherited_integrity.pop("elapsed_s", None)
        inherited_integrity["mesh_material_volumes"] = dict(inherited_integrity["mesh_material_volumes"])
        inherited_integrity["mesh_material_volumes"]["path"] = str(source["volume_path"])
        inherited_integrity["reused_from_record"] = source["record_path"]
        inherited_integrity["neutron_transport_reused"] = True
        inherited_integrity["reused_from_record_sha256"] = source["record_sha256"]
        record["neutron_transport_integrity"] = inherited_integrity
        record["activation_regions"] = int(source["integrity"]["mesh_material_volumes"]["domain_count"])
        record["bounded_protocol_plan"] = {
            "path": source["protocol_path"], "sha256": source["protocol_sha256"],
            "protocol_sha256": source["protocol_identity_sha256"],
            "status": "REUSED_NEUTRON_INPUT_PROTOCOL"}
        record["reused_neutron_stage"] = {
            "source_record_path": source["record_path"], "source_record_sha256": source["record_sha256"],
            "source_protocol_path": source["protocol_path"],
            "source_protocol_file_sha256": source["protocol_sha256"],
            "source_protocol_identity_sha256": source["protocol_identity_sha256"],
            "original_collapse_plan_path": source["old_collapse_plan_path"],
            "original_collapse_plan_sha256": source["old_collapse_plan_sha256"],
            "original_partial_collapse_output_used": False,
            "neutron_transport_executed_again": False,
            "validated_artifacts": {
                "model_xml": {"path": str(source["model_xml"]), "sha256": source["integrity"]["model_xml_sha256"]},
                "materials_xml": {"path": str(source["materials_xml"]), "sha256": record["neutron_materials_xml"]["sha256"]},
                "mesh_material_volumes": {"path": str(source["volume_path"]), "sha256": source["volume_sha256"]},
                "fluxes": {"path": str(source["flux_path"]), "sha256": source["integrity"]["flux_mean_sha256"]},
                "fluxes_std_dev": {"path": str(source["flux_std_path"]), "sha256": source["integrity"]["flux_std_dev_sha256"]}},
            "indexed_data_files_to_revalidate_in_new_collapse_plan": len(source["coverage_indexed_data_files"])}
        provenance_path = neutron_dir / "reused-neutron-stage.json"
        BM._write_json_new(provenance_path, record["reused_neutron_stage"])
        volume_path, flux_path, flux_std_path = source["volume_path"], source["flux_path"], source["flux_std_path"]
        volume_layout = BM.mesh_material_volumes_preflight(volume_path)
        with np.load(volume_path, allow_pickle=False) as saved:
            material_array = np.asarray(saved["materials"])
            actual_domains = int(np.count_nonzero(material_array > -1))
        flux_mean = np.load(flux_path, mmap_mode="r", allow_pickle=False)
        if (actual_domains != record["activation_regions"] or flux_mean.shape != (actual_domains, group_count) or
                flux_mean.dtype != np.dtype(np.float64)):
            raise ValueError("reused neutron flux/volume array shapes differ from the frozen activation domain")
        if volume_layout["domain_count"] != actual_domains:
            raise ValueError("reused neutron volume-domain count changed after validation")
        memory_budget = int(args.microxs_memory_budget_mib * 1024 ** 2)
        output_budget = int(args.microxs_output_budget_gib * BM.GIB)
        disk_reserve = int(args.disk_reserve_gib * BM.GIB)
        if shutil.disk_usage(neutron_dir).free < disk_reserve:
            raise ValueError("current free space is below the declared reserve before reused-stage collapse")

        plan_path = neutron_dir / "bounded-collapse-plan.json"
        collapse_dir = neutron_dir / "bounded-collapse"
        collapse_plan = BM.prepare_plan(plan_path, fluxes_path=flux_path, volumes_path=volume_path,
            chain_path=CHAIN, cross_sections_xml=RM.FARIS_XS, materials_xml=neutron_materials_xml,
            energy_edges_eV=group_edges, output_run_dir=collapse_dir,
            domain_chunk=args.activation_chunk_domains, output_budget_bytes=output_budget,
            disk_reserve_bytes=disk_reserve, memory_limit_bytes=memory_budget,
            energy_group_structure=args.activation_groups)
        old_assets = source["coverage_indexed_data_files"]
        new_assets = collapse_plan.get("required_nuclide_hdf5", [])
        if old_assets != new_assets:
            raise ValueError("current nuclide HDF5 paths/hashes differ from the neutron source's pinned coverage manifest")
        expected_inputs = {
            "fluxes": (source["flux_path"], source["integrity"]["flux_mean_sha256"]),
            "volumes": (source["volume_path"], source["volume_sha256"]),
            "materials": (neutron_materials_xml, record["neutron_materials_xml"]["sha256"]),
            "chain": (CHAIN.resolve(), RM.sha256(CHAIN)),
            "cross_sections": (RM.FARIS_XS.resolve(), RM.sha256(RM.FARIS_XS)),
        }
        for name, (expected_path, expected_sha) in expected_inputs.items():
            planned = collapse_plan.get("inputs", {}).get(name, {})
            if (Path(planned.get("path", "")).resolve() != Path(expected_path).resolve() or
                    planned.get("sha256") != expected_sha):
                raise ValueError(f"fresh collapse plan input changed during neutron reuse validation: {name}")
        collapse_record = BM.run_collapse(plan_path, collapse_dir)
        bounded_paths.update({"volume": volume_path, "flux": flux_path, "flux_std": flux_std_path,
                              "plan": plan_path, "collapse_dir": collapse_dir,
                              "group_mean": Path(collapse_record["output"]["path"]),
                              "collapse_record": collapse_dir / "collapse_record.json"})
        if not args.skip_capi_check:
            capi_domain = args.capi_domain_index
            if capi_domain is None:
                positive = np.flatnonzero(np.sum(flux_mean, axis=1) > 0.0)
                if not len(positive):
                    raise ValueError("no positive-flux activation domain is available for the CAPI sample")
                capi_domain = int(positive[0])
            if capi_domain < 0 or capi_domain >= actual_domains or float(flux_mean[capi_domain].sum()) <= 0.0:
                raise ValueError("selected CAPI sample domain has no positive flux spectrum")
            checks_capi = [{"nuclide": "Fe56", "reaction": "(n,gamma)", "domain_index": capi_domain},
                           {"nuclide": "Fe56", "reaction": "(n,2n)", "domain_index": capi_domain},
                           {"nuclide": "Ni58", "reaction": "(n,gamma)", "domain_index": capi_domain}]
            capi = BM.compare_capi_reference(RM.FARIS_XS, flux_path, group_edges, checks_capi,
                                               energy_group_structure=args.activation_groups)
            BM._write_json_new(neutron_dir / "bounded-capi-check.json", capi)
            record["bounded_capi_check"] = {"path": "neutron_transport/bounded-capi-check.json",
                                            "sha256": RM.sha256(neutron_dir / "bounded-capi-check.json"), **capi}
        group_mean_rel = bounded_paths["group_mean"].relative_to(out).as_posix()
        record["activation_microxs"] = {"kind": "bounded group-mean XS plus per-chunk reconstructed MicroXS",
            "path": group_mean_rel, "sha256": collapse_record["output"]["sha256"],
            "collapse_plan_path": plan_path.relative_to(out).as_posix(),
            "collapse_plan_sha256": collapse_plan["plan_sha256"],
            "collapse_record_path": (collapse_dir / "collapse_record.json").relative_to(out).as_posix(),
            "collapse_record_sha256": RM.sha256(collapse_dir / "collapse_record.json"),
            "required_nuclides": coverage["required_microxs_nuclides"],
            "domain_count": actual_domains, "reaction_count": len(chain.reactions),
            "group_mean_xs_path": group_mean_rel, "group_mean_xs_sha256": collapse_record["output"]["sha256"],
            "coverage_revalidation": "full exact chain/material-root/reaction coverage is independently recomputed by the exporter; each activation chunk also records hash of reconstructed domain MicroXS bytes"}
        return

    def step1():
        if args.reuse_neutron_from is not None:
            step1_reusing_neutron()
            return
        if args.activation_backend == "direct":
            estimate_volumes = activation_mesh.material_volumes(
                material_volume_model, n_samples=args.mat_vol_samples, output=False)
            direct_domains = int(np.count_nonzero(np.asarray(estimate_volumes._materials) > -1))
            direct_estimate = BM.collapse_resource_estimate(
                direct_domains, len(coverage["required_microxs_nuclides"]),
                len(chain.reactions), group_count, args.activation_chunk_domains)
            direct_output = direct_estimate["direct_one_tensor_bytes"]
            if direct_estimate["direct_peak_bytes_estimate"] > int(args.microxs_memory_budget_mib * 1024 ** 2):
                raise ValueError("direct R2S dense MicroXS/flux path exceeds its explicit memory preflight; select bounded backend in a fresh run")
            if direct_output > int(args.microxs_output_budget_gib * BM.GIB):
                raise ValueError("direct R2S MicroXS output exceeds its explicit output budget")
            if shutil.disk_usage(neutron_dir).free < direct_output + int(args.disk_reserve_gib * BM.GIB):
                raise ValueError("direct R2S MicroXS output plus disk reserve exceeds available space")
            record["direct_resource_preflight"] = {
                "status": "PASS_WITHIN_DECLARED_BUDGETS", "sampled_domain_count": direct_domains,
                "resources": direct_estimate, "output_budget_bytes": int(args.microxs_output_budget_gib * BM.GIB),
                "memory_budget_bytes": int(args.microxs_memory_budget_mib * 1024 ** 2),
                "disk_reserve_bytes": int(args.disk_reserve_gib * BM.GIB)}
            save()
            # R2SManager computes the mesh material volumes internally. Route only that
            # geometric estimate through the auxiliary world; restore the original bound
            # method before any neutron transport call.
            original_material_volumes = activation_mesh.material_volumes
            had_instance_override = "material_volumes" in activation_mesh.__dict__
            instance_material_volumes = activation_mesh.__dict__.get("material_volumes")
            def restore_material_volumes():
                if had_instance_override:
                    activation_mesh.material_volumes = instance_material_volumes
                elif "material_volumes" in activation_mesh.__dict__:
                    del activation_mesh.material_volumes

            def sample_in_auxiliary_world(_model, *a, **kw):
                try:
                    return original_material_volumes(material_volume_model, *a, **kw)
                finally:
                    restore_material_volumes()

            activation_mesh.material_volumes = sample_in_auxiliary_world
            try:
                with DS.NeutronManagerRunEvidence(openmc, neutron_dir / "model.xml") as evidence:
                    mgr.step1_neutron_transport(neutron_dir, mat_vol_kwargs={"n_samples": args.mat_vol_samples},
                                                micro_kwargs={"nuclides": coverage["required_microxs_nuclides"],
                                                              "energies": sc.ACTIVATION_GROUPS,
                                                              "chain_file": str(CHAIN), "run_kwargs": {"output": False}})
            finally:
                restore_material_volumes()
            record["neutron_transport_integrity"] = evidence.result()
            if record["neutron_transport_integrity"].get("status") != "PASS":
                raise RuntimeError("R2S neutron transport integrity is " +
                                   str(record["neutron_transport_integrity"].get("status")) + ": " +
                                   str(record["neutron_transport_integrity"].get("reason", "loss gate failed")))
            record["activation_regions"] = len(mgr.results["fluxes"])
            microxs_path = neutron_dir / "micros.h5"
            microxs_inspection = AC.inspect_microxs(microxs_path, coverage["required_microxs_nuclides"])
            actual_domain_nuclides = [sorted(set(micro.nuclides)) for micro in mgr.results["micros"]]
            if not actual_domain_nuclides or any(names != coverage["required_microxs_nuclides"]
                                                 for names in actual_domain_nuclides):
                raise RuntimeError("actual in-memory MicroXS nuclides differ from the complete reachable-chain request")
            record["activation_microxs"] = {
                "kind": "OpenMC per-domain MicroXS HDF5",
                "path": microxs_path.relative_to(out).as_posix(), "sha256": RM.sha256(microxs_path),
                **microxs_inspection,
            }
            return

        volume_path = neutron_dir / "mesh_material_volumes.npz"
        mmv = activation_mesh.material_volumes(material_volume_model,
                                               n_samples=args.mat_vol_samples, output=False)
        mmv.save(volume_path)
        volume_layout = BM.mesh_material_volumes_preflight(volume_path)
        with np.load(volume_path, allow_pickle=False) as saved:
            material_array = np.asarray(saved["materials"])
            volume_array = np.asarray(saved["volumes"], dtype=np.float64)
            actual_domains = int(np.count_nonzero(material_array > -1))
        if actual_domains < 1:
            raise RuntimeError("OpenMC material-volume sampling found no material-bearing activation domains")
        memory_budget = int(args.microxs_memory_budget_mib * 1024 ** 2)
        output_budget = int(args.microxs_output_budget_gib * BM.GIB)
        disk_reserve = int(args.disk_reserve_gib * BM.GIB)
        estimate = BM.collapse_resource_estimate(actual_domains,
                                                  len(coverage["required_microxs_nuclides"]),
                                                  len(chain.reactions), group_count, args.activation_chunk_domains)
        activation_estimate = BM.activation_resource_estimate(
            actual_domains, len(chain.nuclides), len(coverage["required_microxs_nuclides"]),
            len(chain.reactions), len(sc.TIMESTEPS), args.activation_chunk_domains,
            group_count=group_count, mesh_array_bytes=volume_layout["array_bytes"],
            mesh_element_count=volume_layout["shape"][0],
            material_slot_count=volume_layout["shape"][1])
        _chain_reactions, _reachable, metadata_required, collapse_reactions = BM._chain_spec(
            CHAIN, nuclides, openmc.data.REACTION_MT)
        if metadata_required != coverage["required_microxs_nuclides"]:
            raise ValueError("bounded pointwise-array requirements differ from the full-chain coverage guard")
        collapse_data_memory = BM.collapse_metadata_memory_preflight(
            RM.FARIS_XS, metadata_required, collapse_reactions, openmc.data.REACTION_MT,
            actual_domains, args.activation_chunk_domains, group_count=group_count)
        if collapse_data_memory["working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded pointwise XS metadata working estimate {collapse_data_memory['working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        flux_pair_estimate = 2 * actual_domains * group_count * 8
        chunk_count_estimate = (actual_domains + args.activation_chunk_domains - 1) // args.activation_chunk_domains
        chunk_metadata_estimate = chunk_count_estimate * 8192 + 2 * 1024 ** 2
        other_artifacts_estimate = volume_path.stat().st_size + flux_pair_estimate + chunk_metadata_estimate
        non_activation_output_estimate = (estimate["bounded_output_bytes_upper_bound"] +
                                          other_artifacts_estimate)
        remaining_output_budget = output_budget - non_activation_output_estimate
        if remaining_output_budget < 2:
            raise ValueError("compact collapse and neutron flux artifacts consume the declared output budget")
        activation_source_archive_budget = remaining_output_budget // 2
        photon_model_xml_budget = remaining_output_budget - activation_source_archive_budget
        if estimate["bounded_working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded collapse working estimate {estimate['bounded_working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        if activation_estimate["working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded activation working estimate {activation_estimate['working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        if shutil.disk_usage(neutron_dir).free < activation_source_archive_budget + disk_reserve:
            raise ValueError("bounded source archive allowance plus declared disk reserve exceeds available space")
        record["bounded_resource_preflight"] = {
            "status": "PASS_WITHIN_DECLARED_BUDGETS",
            "domain_count": actual_domains,
            "output_budget_bytes": output_budget, "disk_reserve_bytes": disk_reserve,
            "memory_budget_bytes": memory_budget,
            "non_activation_output_upper_bound_bytes": non_activation_output_estimate,
            "source_archive_compressed_hard_cap_bytes": activation_source_archive_budget,
            "source_archive_decompressed_hard_cap_bytes": activation_source_archive_budget,
            "photon_model_xml_hard_cap_bytes": photon_model_xml_budget,
            "flux_pair_estimate_bytes": flux_pair_estimate,
            "mesh_material_volumes_bytes": volume_path.stat().st_size,
            "estimated_chunk_count": chunk_count_estimate,
            "chunk_metadata_allowance_bytes": chunk_metadata_estimate,
            "available_disk_bytes": shutil.disk_usage(neutron_dir).free,
            "collapse": estimate, "collapse_pointwise_memory": collapse_data_memory,
            "activation": activation_estimate}
        save()
        elements, slots = np.where(material_array > -1)
        domain_materials = material_array[elements, slots].astype(np.int64, copy=False)
        domain_volumes = volume_array[elements, slots]
        volume_receipt = {"path": volume_path.relative_to(out).as_posix(), "sha256": RM.sha256(volume_path),
                          "size_bytes": volume_path.stat().st_size, "domain_count": actual_domains,
                          "layout": volume_layout,
                          "domain_map_sha256": BM.domain_map_sha256(elements, domain_materials, domain_volumes)}
        tally = openmc.Tally(name="R3 bounded MicroXS flux")
        tally.filters = [openmc.MeshMaterialFilter.from_volumes(activation_mesh, mmv),
                        openmc.EnergyFilter(group_edges)]
        tally.scores = ["flux"]
        below_edge_tallies, below_edge_plan = (below_data_edge_diagnostic_plan(openmc, neutron)
                                                if args.below_data_edge_diagnostic else ([], None))
        all_neutron_tallies = [tally, *below_edge_tallies]
        tally_ids = [int(item.id) for item in all_neutron_tallies]
        if len(tally_ids) != len(set(tally_ids)):
            raise ValueError("bounded flux and below-data-edge diagnostic tallies have duplicate OpenMC tally IDs")
        if below_edge_plan is not None:
            below_edge_plan["tally_ids"] = {str(item.name): int(item.id) for item in below_edge_tallies}
            below_edge_plan.pop("configuration_sha256", None)
            below_edge_plan["configuration_sha256"] = _canonical_sha256(below_edge_plan)
        neutron.tallies = openmc.Tallies(all_neutron_tallies)
        neutron.export_to_model_xml(str(neutron_dir))
        model_xml = neutron_dir / "model.xml"
        if RM.sha256(neutron_dir / "materials.xml") != record["neutron_materials_xml"]["sha256"]:
            raise ValueError("neutron materials.xml changed after its explicit pre-coverage export")
        if below_edge_plan is not None:
            for nuclide, identity in below_edge_plan["ce_nuclide_data_files"].items():
                if AC.sha256_file(Path(identity["path"])) != identity["sha256"]:
                    raise ValueError(f"CE data file changed after below-data-edge tally planning: {nuclide}")
        protocol = {
            "schema": "faris-r3-bounded-campaign-protocol/v1", "status": "READY_FOR_NEUTRON_TRANSPORT",
            "activation_mesh": record["activation_domain"], "sampling_settings": record["settings"],
            "coverage": {"status": coverage["status"], "required_nuclide_count": len(coverage["required_microxs_nuclides"]),
                         "required_microxs_nuclides": coverage["required_microxs_nuclides"],
                         "reaction_count": len(chain.reactions)},
            "inputs": {"model_xml": {"path": str(model_xml), "sha256": RM.sha256(model_xml)},
                       "materials_xml": {"path": str(neutron_dir / "materials.xml"), "sha256": RM.sha256(neutron_dir / "materials.xml")},
                       "mesh_material_volumes": volume_receipt,
                       "chain": {"path": str(CHAIN), "sha256": RM.sha256(CHAIN)},
                       "cross_sections_xml": {"path": str(RM.FARIS_XS), "sha256": RM.sha256(RM.FARIS_XS)}},
            "implementation": {str(path.name): RM.sha256(path) for path in
                                (Path(__file__), Path(BM.__file__), Path(AC.__file__), Path(DS.__file__))},
            "resource_preflight": record["bounded_resource_preflight"],
            "fixed_protocol": {"energy_groups": args.activation_groups, "energy_group_count": group_count,
                               "energy_edges_eV": list(group_edges),
                               "energy_edges_sha256": sc.ACTIVATION_GROUP_BASIS[args.activation_groups]["energy_edges_sha256"],
                               "flux_tally": "MeshMaterial x activation EnergyFilter flux tally",
                               "below_data_edge_diagnostic": below_edge_plan,
                               "tally_configuration": [{"id": int(item.id), "name": str(item.name)} for item in all_neutron_tallies],
                               "activation_group_mean_below_data_edge": "zero by pointwise-grid clipping; any CE extrapolated response is unresolved unless diagnostic tallies are collected",
                               "domain_order": "row-major MeshMaterialFilter.from_volumes(MeshMaterialVolumes)",
                               "collapse": BM.ALGORITHM, "activation": "OpenMC 0.15.3 independent predictor chunks",
                               "physics_settings": "existing source, irradiation schedule, dose response mesh, and photon source slots are unchanged"}}
        protocol["protocol_sha256"] = BM._canonical_sha256(protocol)
        protocol_path = neutron_dir / "bounded-protocol-plan.json"
        BM._write_json_new(protocol_path, protocol)
        record["bounded_protocol_plan"] = {"path": protocol_path.relative_to(out).as_posix(),
                                            "sha256": RM.sha256(protocol_path),
                                            "protocol_sha256": protocol["protocol_sha256"]}
        save()
        previous = 0.0
        auxiliary_batch_cumulative = {str(item.id): [] for item in below_edge_tallies}

        def consume_flux_batch(cumulative, _batch):
            nonlocal previous
            previous, _increment = DS.scalar_batch_total(float(np.sum(cumulative)), previous)
            for item in below_edge_tallies:
                live = openmc.lib.tallies[int(item.id)]
                if int(live.num_realizations) != int(_batch):
                    raise ValueError(f"below-data-edge tally {item.name} is not aligned to neutron batch {_batch}")
                auxiliary_batch_cumulative[str(item.id)].append(
                    np.asarray(live.results[:, :, 1], dtype=np.float64).reshape(-1).copy().tolist())

        run, final_sums, final_std = DS.run_live_tally(
            openmc, model_xml, tally.id, args.threads, args.neutron_batches, 86400.0,
            consume_flux_batch, particles_per_batch=args.neutron_particles)
        if run.get("stop_reason") != "batch_limit" or run.get("solver_status") != 1 or \
                run.get("batches") != args.neutron_batches:
            raise RuntimeError(f"bounded neutron flux run did not reach exact planned final batch: {run}")
        if run.get("integrity", {}).get("lost_particle_assessment", {}).get("status") != "PASS":
            raise RuntimeError("bounded neutron flux run has unavailable or unacceptable lost-particle evidence")
        if int(np.asarray(final_sums).size) != actual_domains * group_count or int(np.asarray(final_std).size) != actual_domains * group_count:
            raise RuntimeError("bounded neutron flux tally shape differs from material-volume domain x activation group rows")
        if below_edge_plan is not None:
            diagnostic = summarize_below_data_edge_batches(below_edge_plan, auxiliary_batch_cumulative,
                                                           args.neutron_batches, args.neutron_particles)
            diagnostic["status"] = "DESCRIPTIVE_ONLY"
            diagnostic["qualification_verdict"] = "NOT_PROVIDED: CE below-data-edge response diagnostic only; not full activation or dose convergence"
            diagnostic["protocol_sha256"] = protocol["protocol_sha256"]
            diag_path = neutron_dir / "below-data-edge-diagnostic.json"
            BM._write_json_new(diag_path, diagnostic)
            record["below_data_edge_diagnostic"] = {
                "path": diag_path.relative_to(out).as_posix(), "sha256": RM.sha256(diag_path),
                "configuration_sha256": below_edge_plan["configuration_sha256"],
                "protocol_sha256": protocol["protocol_sha256"], "status": diagnostic["status"]}
        flux_path = neutron_dir / "fluxes.npy"
        flux_mean = np.asarray(final_sums, dtype=np.float64).reshape(actual_domains, group_count) / run["batches"]
        np.save(flux_path, flux_mean, allow_pickle=False)
        flux_std_path = neutron_dir / "fluxes_std_dev.npy"
        np.save(flux_std_path, np.asarray(final_std, dtype=np.float64).reshape(actual_domains, group_count), allow_pickle=False)
        record["neutron_transport_integrity"] = {"status": "PASS", **run["integrity"],
            "stop_reason": run["stop_reason"], "solver_status": run["solver_status"],
            "batches": run["batches"], "histories": run["histories"], "elapsed_s": run["elapsed_s"],
            "model_xml_sha256": RM.sha256(model_xml), "flux_mean_sha256": RM.sha256(flux_path),
            "flux_std_dev_sha256": RM.sha256(flux_std_path), "mesh_material_volumes": volume_receipt,
            "mean_estimator": f"OpenMC cumulative flux tally sums divided by actual batch realizations; one row per exact MeshMaterialFilter.from_volumes domain, groups ascending on frozen {args.activation_groups} edges"}
        record["activation_regions"] = actual_domains
        if args.stop_after_neutron:
            record["activation_microxs"] = {
                "status": "NOT_RUN",
                "reason": "--stop-after-neutron exits after neutron flux/diagnostic artifacts; bounded collapse and CAPI comparison are not run"}
            record["microxs_status"] = "NOT_RUN"
            save()
            return
        plan_path = neutron_dir / "bounded-collapse-plan.json"
        collapse_dir = neutron_dir / "bounded-collapse"
        collapse_plan = BM.prepare_plan(plan_path, fluxes_path=flux_path, volumes_path=volume_path,
            chain_path=CHAIN, cross_sections_xml=RM.FARIS_XS, materials_xml=neutron_dir / "materials.xml",
            energy_edges_eV=group_edges, output_run_dir=collapse_dir,
            domain_chunk=args.activation_chunk_domains, output_budget_bytes=output_budget,
            disk_reserve_bytes=disk_reserve, memory_limit_bytes=memory_budget,
            energy_group_structure=args.activation_groups)
        collapse_record = BM.run_collapse(plan_path, collapse_dir)
        bounded_paths.update({"volume": volume_path, "flux": flux_path, "flux_std": flux_std_path,
                              "plan": plan_path, "collapse_dir": collapse_dir,
                              "group_mean": Path(collapse_record["output"]["path"]),
                              "collapse_record": collapse_dir / "collapse_record.json"})
        if not args.skip_capi_check:
            capi_domain = args.capi_domain_index
            if capi_domain is None:
                positive = np.flatnonzero(np.sum(flux_mean, axis=1) > 0.0)
                if not len(positive):
                    raise ValueError("no positive-flux activation domain is available for the CAPI sample")
                capi_domain = int(positive[0])
            if capi_domain < 0 or capi_domain >= actual_domains or float(flux_mean[capi_domain].sum()) <= 0.0:
                raise ValueError("selected CAPI sample domain has no positive flux spectrum")
            checks_capi = [{"nuclide": "Fe56", "reaction": "(n,gamma)", "domain_index": capi_domain},
                           {"nuclide": "Fe56", "reaction": "(n,2n)", "domain_index": capi_domain},
                           {"nuclide": "Ni58", "reaction": "(n,gamma)", "domain_index": capi_domain}]
            capi = BM.compare_capi_reference(RM.FARIS_XS, flux_path, group_edges, checks_capi,
                                               energy_group_structure=args.activation_groups)
            BM._write_json_new(neutron_dir / "bounded-capi-check.json", capi)
            record["bounded_capi_check"] = {"path": "neutron_transport/bounded-capi-check.json",
                                            "sha256": RM.sha256(neutron_dir / "bounded-capi-check.json"), **capi}
        group_mean_rel = bounded_paths["group_mean"].relative_to(out).as_posix()
        record["activation_microxs"] = {"kind": "bounded group-mean XS plus per-chunk reconstructed MicroXS",
            "path": group_mean_rel, "sha256": collapse_record["output"]["sha256"],
            "collapse_plan_path": plan_path.relative_to(out).as_posix(),
            "collapse_plan_sha256": collapse_plan["plan_sha256"],
            "collapse_record_path": (collapse_dir / "collapse_record.json").relative_to(out).as_posix(),
            "collapse_record_sha256": RM.sha256(collapse_dir / "collapse_record.json"),
            "required_nuclides": coverage["required_microxs_nuclides"],
            "domain_count": actual_domains,
            "reaction_count": len(chain.reactions),
            "group_mean_xs_path": group_mean_rel,
            "group_mean_xs_sha256": collapse_record["output"]["sha256"],
            "coverage_revalidation": "full exact chain/material-root/reaction coverage is independently recomputed by the exporter; each activation chunk also records hash of reconstructed domain MicroXS bytes"}

    if not timed(record, "neutron_transport", step1):
        save()
        return 1
    if args.reuse_neutron_from is not None:
        record["steps"]["neutron_transport"].update({
            "neutron_transport_reused": True, "collapse_recomputed": True,
            "elapsed_scope": "source validation and new collapse only; original neutron elapsed time is excluded"})
    save()
    if args.stop_after_neutron:
        mark_neutron_only_complete(record)
        save()
        return 0

    def step2():
        if args.activation_backend == "direct":
            mgr.step2_activation(sc.TIMESTEPS, sc.SOURCE_RATES, output_dir=out / "activation", operator_kwargs={"chain_file": str(CHAIN)})
            return
        activation_plan_path = out / "activation" / "bounded-activation-plan.json"
        activation_dir = out / "activation" / "bounded-chunks"
        activation_plan = BM.prepare_activation_plan(activation_plan_path,
            collapse_plan_path=bounded_paths["plan"], collapse_record_path=bounded_paths["collapse_record"],
            microxs_path=bounded_paths["group_mean"], fluxes_path=bounded_paths["flux"],
            volumes_path=bounded_paths["volume"], materials_xml=out / "neutron_transport" / "materials.xml",
            chain_path=CHAIN, output_run_dir=activation_dir, timesteps=sc.TIMESTEPS,
            source_rates=sc.SOURCE_RATES, domain_chunk=args.activation_chunk_domains,
            time_budget_s=args.activation_time_budget,
            output_budget_bytes=int(record["bounded_resource_preflight"]["source_archive_compressed_hard_cap_bytes"]),
            disk_reserve_bytes=int(args.disk_reserve_gib * BM.GIB),
            memory_limit_bytes=int(args.microxs_memory_budget_mib * 1024 ** 2))
        activation_record = BM.run_activation_chunks(activation_plan_path)
        bounded_paths.update({"activation_plan": activation_plan_path,
                              "activation_record": activation_dir / "activation_record.json"})
        record["bounded_activation"] = {"plan_path": activation_plan_path.relative_to(out).as_posix(),
            "plan_sha256": activation_plan["plan_sha256"],
            "record_path": (activation_dir / "activation_record.json").relative_to(out).as_posix(),
            "record_sha256": RM.sha256(activation_dir / "activation_record.json"), **activation_record}

    if not timed(record, "activation", step2):
        save()
        return 1
    save()

    conservation = {}
    contributors = {}

    def step3():
        import numpy as np

        record["photon"] = {}
        for idx in args.photon_indices:
            label = sc.COOLING_LABELS[idx]
            t0 = time.time()
            source_summary_ref = None
            if args.activation_backend == "direct":
                sources = mgr.get_decay_photon_source_mesh(idx)
                total, power = inventory_emission(openmc, chain, mgr, idx)
                unclipped = 0.0
                for mat in mgr.results["activation_materials"]:
                    e = mgr.results["depletion_results"][idx].get_material(str(mat.id)).get_decay_photon_energy(clip_tolerance=0.0)
                    unclipped += e.integral() if e is not None else 0.0
            else:
                sources, source_summary = BM.build_chunked_decay_photon_sources(
                    bounded_paths["activation_plan"], bounded_paths["activation_record"], activation_mesh, idx,
                    openmc_module=openmc)
                time_dir = out / "photon_transport" / f"time_{idx}"
                time_dir.mkdir(parents=True)
                source_summary_path = time_dir / "chunked_source_summary.json"
                BM._write_json_new(source_summary_path, source_summary)
                source_summary_ref = {"path": source_summary_path.relative_to(out).as_posix(),
                                      "sha256": RM.sha256(source_summary_path)}
                total, power, unclipped = inventory_emission_chunked(
                    openmc, chain, bounded_paths["activation_plan"], bounded_paths["activation_record"], idx)
            method_source = DX.write_r2s_source_artifact(out, idx, sources) \
                if args.activation_backend == "direct" else None
            strength = source_strength(sources)
            conservation[label] = checks_conservation = sc.conservation(strength, total)
            checks_conservation["unclipped_source_check"] = sc.conservation(unclipped, total)
            checks_conservation["note"] = ("source = the manager's own MeshSource strengths (Material.get_decay_photon_energy, default "
                                           "clip_tolerance 1e-6); unclipped_source_check uses clip_tolerance 0")
            contributors[label] = sc.top_contributors(power, 10)
            if source_summary_ref:
                record.setdefault("bounded_photon_source_summaries", {})[label] = source_summary_ref
            photon.settings.source = sources
            photon.settings.batches = args.photon_max_batches
            photon.settings.particles = args.photon_particles
            photon.settings.statepoint = {}
            photon.settings.sourcepoint = {"write": False}
            run_dir = out / "photon_transport" / f"time_{idx}"
            run_dir.mkdir(parents=True, exist_ok=True)
            for old in run_dir.glob("statepoint.*.h5"):
                old.unlink()
            photon.export_to_model_xml(str(run_dir))
            if args.activation_backend == "bounded":
                model_xml_total = sum(path.stat().st_size for path in
                                      (out / "photon_transport").rglob("model.xml") if path.is_file())
                model_xml_cap = int(record["bounded_resource_preflight"]["photon_model_xml_hard_cap_bytes"])
                if model_xml_total > model_xml_cap:
                    raise ValueError(f"serialized photon model XML files use {model_xml_total} bytes, above their {model_xml_cap}-byte cap")
                output_cap = int(args.microxs_output_budget_gib * BM.GIB)
                if tree_file_bytes(out) > output_cap:
                    raise ValueError("R3 bounded artifacts exceed the declared total output budget before photon transport")
            if args.activation_backend == "bounded":
                method_source = DX.write_r2s_bounded_source_artifact(
                    out, idx, source_summary, source_summary_ref, run_dir / "model.xml")
                if tree_file_bytes(out) > int(args.microxs_output_budget_gib * BM.GIB):
                    raise ValueError("R3 bounded source manifests exceed the declared total output budget before photon transport")
            tally = photon.tallies[0]
            previous = 0.0
            aggregate_batches = []

            def consume_batch(cumulative, _batch_number):
                nonlocal previous
                total = float(cumulative.sum())
                previous, value = DS.scalar_batch_total(total, previous)
                aggregate_batches.append(value)

            run, final_sums, final_std = DS.run_live_tally(
                openmc, run_dir / "model.xml", tally.id, args.threads, args.photon_max_batches,
                args.photon_budget, consume_batch, particles_per_batch=args.photon_particles)
            if run.get("stop_reason") not in ("batch_limit", "time_budget"):
                raise RuntimeError(f"R2S photon solver ended with {run.get('stop_reason')}; dose summary refused")
            loss_status = run.get("integrity", {}).get("lost_particle_assessment", {}).get("status")
            if loss_status != "PASS":
                raise RuntimeError(f"R2S photon lost-particle assessment is {loss_status}; dose summary refused")
            if run["batches"] < 2:
                raise RuntimeError(f"photon run at {label} completed fewer than two batches; aggregate standard error unavailable")
            mean = (final_sums / run["batches"]).reshape(sc.MESH_DIMENSION[::-1]).copy()
            std = final_std.reshape(sc.MESH_DIMENSION[::-1]).copy()
            summary = DS.batch_summary(aggregate_batches)
            if not np.isclose(summary["mean"], float(mean.sum()), rtol=1.0e-10, atol=1.0e-14):
                raise RuntimeError(f"R2S aggregate batch mean does not reconcile to the tally mean at {label}")
            np.savez(out / f"dose_time_{idx}.npz", mean=mean, std=std)
            batch_path = out / f"shutdown_dose_batch_totals_time_{idx}.npz"
            np.savez_compressed(batch_path, values=np.asarray(aggregate_batches, dtype=float))
            if args.activation_backend == "bounded" and tree_file_bytes(out) > int(args.microxs_output_budget_gib * BM.GIB):
                raise ValueError("R3 bounded output artifacts exceed the declared total output budget after photon scoring")
            record.setdefault("shutdown_dose", {})[label] = {"time_index": idx, **summary,
                "unit": "pSv.cm3/s summed over the scored region",
                "uncertainty_scope": "conditional Monte Carlo photon batch sampling for this generated decay-photon source; neutron transport and activation-source uncertainty are not propagated",
                "method_specific_source": method_source,
                "batch_values_file": batch_path.name, "batch_values_sha256": RM.sha256(batch_path)}
            record["photon"][label] = {"time_index": idx, "seconds": time.time() - t0, "run": run, "batches": run["batches"],
                                       "histories": run["batches"] * args.photon_particles, "source_strength_photons_per_s": strength,
                                       "unit": "pSv.cm3/s per voxel (track length x dose coefficient x photons/s)",
                                       "sum_pSv_cm3_per_s": float(mean.sum()), "conservation": checks_conservation}
            save()
        record["conservation"] = conservation
        record["top_photon_power_nuclides_MeV_per_s"] = contributors

    ok = timed(record, "photon_transport", step3)
    record["accepted_conservation"] = all(c["pass"] for c in conservation.values()) if conservation else None
    save()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
