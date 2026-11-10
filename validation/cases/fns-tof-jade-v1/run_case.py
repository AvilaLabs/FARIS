#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded OpenMC FNS-TOF point-detector approximation runner.

The published MCNP tallies are point detectors. OpenMC has no equivalent F5
estimator, so this adapter uses spherical vacuum-cell volume flux and refuses to
seal a validation record unless the predeclared two-halving, all-bin convergence
gate passes. A passed gate qualifies only this finite-volume approximation; it
does not establish physical validation or resolve experimental covariance.

Requires OpenMC 0.15.3, an installed openmc_mcnp_adapter, the pinned public
input checkout, and an audited OpenMC cross-section library. `plan` is read-only.
`run` is explicitly opt-in and bounded by histories, wall time, and thread count.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import platform
import re
import subprocess
import sys
import shutil
from datetime import datetime, timezone
from pathlib import Path
from xml.etree import ElementTree as ET

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT))
from validation.manifest import load_manifest, manifest_sha256, verify_files  # noqa: E402
from validation.scoring import RUN_KIND, seal_run_record  # noqa: E402

from benchmark import (  # noqa: E402
    SOURCE_COMMIT, IntakeError, energy_bins, input_deck, lethargy_width,
    openmc_histogram, qualify_halvings, read_rows, sha256_bytes, tally_energy_edges,
)

OPENMC_REQUIRED = "0.15.3"
CONVERTER_SOURCE_URL = "https://github.com/openmc-dev/openmc_mcnp_adapter"
CONVERTER_COMMIT = "80fda5abeed22502e2f6e19e57a9b2a8762844a8"
CONVERTER_DISTRIBUTION_VERSION = "0.1.0"
CONVERTER_PYTHON_SOURCE_SHA256 = "a48ac7dede3729d6695f0848d15b2ddb43a9c0f1487be617cd70d0961ce633c1"
MAX_PARTICLES = 50_000_000
MAX_BATCHES = 500
MAX_THREADS = 8
MAX_WALL_SECONDS = 10_800
MAX_SOURCE_BYTES = 2_000_000
MAX_CROSS_SECTIONS_XML_BYTES = 32_000_000
MAX_MODEL_XML_BYTES = 64_000_000
MAX_IMP0_CONTROL_BYTES = 2_000_000
MAX_IMP0_CONTROL_STATEPOINT_BYTES = 256_000_000
FARIS_IMP0_PATCH_VERSION_REQUIRED = 1
FARIS_IMP0_PATCH_VERSION_SYMBOL = "faris_openmc_mcnp_imp0_patch_version"
FARIS_IMP0_PATCH_KILL_EVENTS_DATASET = "faris_mcnp_imp0_kill_events"
FARIS_IMP0_PATCH_VERSION_DATASET = "faris_mcnp_imp0_patch_version"
FARIS_IMP0_PATCH_BIRTH_KILL_DATASET = "faris_mcnp_imp0_birth_kill_events"
FARIS_IMP0_PATCH_ENTRY_KILL_DATASET = "faris_mcnp_imp0_entry_kill_events"
FARIS_OPENMC_LOST_DATASET = "faris_openmc_n_lost_particles"
STATEPOINT_CURRENT_BATCH_DATASET = "current_batch"
STATEPOINT_N_BATCHES_DATASET = "n_batches"
STATEPOINT_N_PARTICLES_DATASET = "n_particles"
STATEPOINT_N_REALIZATIONS_DATASET = "n_realizations"
AIM_PATCH_PATH = ROOT / "integrations/openmc/patches/faris-openmc-0.15.3-aim.patch"
AIM_REQUIRED_MARKERS = (b"OPENMC_APERTURE_BIAS", b"OPENMC_APERTURE_ONE_SIDED",
                        b"OPENMC_COLLISION_AIM")
AIM_SUPPORTED_CASE = "fns-tof-fe-5"
AIM_REQUIRED_TALLIES = (5, 25, 35, 45)
AIM_CONTROL_RADII_CM = (2.5, 1.25, 0.625)
AIM_CONTROL_MAX_RELATIVE_MCSE = 0.02
AIM_CONTROL_MAX_ABS_Z = 3.0
AIM_CONTROL_MIN_BETA = 0.0
AIM_CONTROL_MAX_BETA = 1.0


def require(ok: bool, why: str) -> None:
    if not ok:
        raise ValueError(why)


def isotropic_sphere_volume_average_cm2(distance_cm: float, radius_cm: float) -> float:
    """Exact vacuum flux-density average in a sphere from a unit isotropic point source."""
    distance, radius = float(distance_cm), float(radius_cm)
    require(math.isfinite(distance) and math.isfinite(radius) and distance > radius > 0.0,
            "vacuum control sphere must be positive and lie outside the point source")
    u = radius / distance
    if u < 0.1:
        # Stable series for integral_0^u t*ln((1+t)/(1-t)) dt.
        integral = math.fsum(2.0 * u ** (2 * k + 3) / ((2 * k + 1) * (2 * k + 3))
                             for k in range(20))
    else:
        integral = u + 0.5 * (u * u - 1.0) * math.log((1.0 + u) / (1.0 - u))
    value = 3.0 * distance * integral / (8.0 * math.pi * radius ** 3)
    require(math.isfinite(value) and value > 0.0, "analytic vacuum sphere flux is non-finite")
    return value


def _normalize(vector: list[float]) -> list[float]:
    length = math.sqrt(math.fsum(value * value for value in vector))
    require(math.isfinite(length) and length > 0.0, "cannot normalize zero or non-finite AIM target vector")
    return [value / length for value in vector]


def source_aperture_plan(parsed: dict, radius_cm: float, beta: float) -> dict:
    """Construct four finite rectangular AIM targets while retaining isotropic support."""
    require(math.isfinite(beta) and AIM_CONTROL_MIN_BETA < beta < AIM_CONTROL_MAX_BETA,
            "AIM beta must be strictly between zero and one to preserve physical isotropic support")
    radius = float(radius_cm)
    require(math.isfinite(radius) and radius > 0.0, "AIM aperture radius must be positive and finite")
    detectors = parsed["detectors"]
    ids = tuple(int(point["tally"]) for point in detectors)
    require(ids == AIM_REQUIRED_TALLIES,
            f"FNS-Fe-5 AIM requires all four F5 detector points {AIM_REQUIRED_TALLIES}, got {ids}")
    source = [float(value) for value in parsed["source_position_cm"]]
    half = radius / math.sqrt(2.0)
    aperture_records = []
    rect_specs = []
    max_pdf_bound = 0.0
    for point in detectors:
        center = [float(value) for value in point["position_cm"]]
        delta = [center[i] - source[i] for i in range(3)]
        distance = math.sqrt(math.fsum(value * value for value in delta))
        normal = _normalize(delta)
        reference = [1.0, 0.0, 0.0] if abs(normal[0]) < 0.9 else [0.0, 1.0, 0.0]
        u_hat = _normalize([normal[1] * reference[2] - normal[2] * reference[1],
                            normal[2] * reference[0] - normal[0] * reference[2],
                            normal[0] * reference[1] - normal[1] * reference[0]])
        v_hat = [normal[1] * u_hat[2] - normal[2] * u_hat[1],
                 normal[2] * u_hat[0] - normal[0] * u_hat[2],
                 normal[0] * u_hat[1] - normal[1] * u_hat[0]]
        u = [half * value for value in u_hat]
        v = [half * value for value in v_hat]
        area = 4.0 * half * half
        # A square with corners at radius from its centre lies within the
        # projected finite sphere. The exact proposal solid angle is analytic.
        solid_angle = 4.0 * math.atan((half * half) /
                                      (distance * math.sqrt(distance * distance + 2.0 * half * half)))
        full_sphere_fraction = 0.5 * (1.0 - math.sqrt(1.0 - (radius / distance) ** 2))
        rectangle_fraction = solid_angle / (4.0 * math.pi)
        require(rectangle_fraction > 0.0 and rectangle_fraction < full_sphere_fraction,
                f"F{point['tally']} AIM rectangle does not lie strictly inside its finite sphere aperture")
        # At the rectangle corner, p_rect is maximal for this source point.
        pdf_bound = (distance * distance + 2.0 * half * half) ** 1.5 / (area * distance)
        max_pdf_bound = max(max_pdf_bound, pdf_bound)
        rect_values = [*center, *u, *v]
        rect_specs.append(" ".join(format(value, ".17g") for value in rect_values))
        aperture_records.append({"mcnp_tally_number": point["tally"], "angle_deg": point["angle_deg"],
                                 "center_cm": center, "u_cm": u, "v_cm": v,
                                 "source_distance_cm": distance, "radius_cm": radius,
                                 "square_half_side_cm": half, "projected_square_area_cm2": area,
                                 "proposal_solid_angle_sr": solid_angle,
                                 "proposal_isotropic_probability": rectangle_fraction,
                                 "finite_sphere_solid_angle_probability": full_sphere_fraction,
                                 "rectangular_proposal_pdf_source_upper_bound_sr-1": pdf_bound,
                                 "analytic_isotropic_volume_average_flux_per_cm2_per_source":
                                     isotropic_sphere_volume_average_cm2(distance, radius)})
    aperture_spec = f"{beta:.17g} " + rect_specs[0] + " ; " + " ; ".join(rect_specs[1:])
    min_weight_bound = 1.0 / ((1.0 - beta) + 4.0 * math.pi * beta * max_pdf_bound)
    max_weight_bound = 1.0 / (1.0 - beta)
    require(0.0 < min_weight_bound <= max_weight_bound < math.inf,
            "source-aperture likelihood-ratio bounds are invalid")
    return {"method": "FARIS OpenMC 0.15.3 AIM source angular mixture",
            "beta": beta, "source_position_cm": source,
            "physical_law": "isotropic MCNP SDEF direction, unchanged",
            "proposal_law": "q(omega)=(1-beta)/(4*pi)+beta*(1/n_visible)*sum_j p_rectangle_j(omega)",
            "source_weight_correction": "p_iso/q = 1/((1-beta)+4*pi*beta*aperture_pdf)",
            "source_birth_weight_bounds": {"lower_bound": min_weight_bound,
                                             "upper_bound": max_weight_bound,
                                             "basis": "source aperture density is bounded by the maximum exact rectangle Jacobian over its four corners; positive isotropic mixture supplies the finite upper bound"},
            "environment": {"OPENMC_APERTURE_BIAS": aperture_spec,
                            "OPENMC_APERTURE_ONE_SIDED": "1"},
            "collision_aim": {"enabled": False,
                              "reason": "the FNS adapter has no qualified weight windows; the AIM patch refuses elastic-collision aiming without them"},
            "detector_apertures": aperture_records,
            "all_four_detector_points_required": True,
            "claim": "importance sampling only; no variance reduction or F5 equivalence is claimed"}


def aim_runtime_identity(runtime: dict) -> dict:
    """Bind AIM capability to the exact executable-selected libopenmc.so."""
    library = Path(runtime["linked_library_path"]).resolve(strict=True)
    payload = library.read_bytes()
    missing = [marker.decode() for marker in AIM_REQUIRED_MARKERS if marker not in payload]
    require(not missing, "selected OpenMC library lacks AIM patch support: " + ", ".join(missing))
    patch = AIM_PATCH_PATH.resolve(strict=True)
    return {"patch_source": str(patch), "patch_sha256": hash_file(patch),
            "linked_library_path": str(library), "linked_library_sha256": hash_file(library),
            "supported_markers": [marker.decode() for marker in AIM_REQUIRED_MARKERS],
            "collision_aim_available_but_disabled": True}


def canonical_json_sha256(document: dict, omit: tuple[str, ...] = ()) -> str:
    payload = {key: value for key, value in document.items() if key not in set(omit)}
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
                     allow_nan=False).encode("utf-8")
    return sha256_bytes(raw)


def verify_aim_control_receipt(path: Path, runtime: dict, case_id: str, parsed: dict,
                               source_deck_sha256: str, beta: float, imp0_control: dict) -> dict:
    require(path.is_file(), "a passing four-point AIM vacuum-control receipt is required before AIM production")
    require(path.stat().st_size <= MAX_IMP0_CONTROL_BYTES,
            "AIM vacuum-control receipt exceeds the bounded JSON input limit")
    document = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(document, dict), "AIM vacuum-control receipt must be a JSON object")
    require(document.get("schema") == "faris.fns-aim-vacuum-control/1.0.0"
            and document.get("status") == "PASS",
            "AIM vacuum-control receipt has wrong schema or did not pass")
    recorded_hash = document.get("receipt_sha256")
    require(isinstance(recorded_hash, str) and recorded_hash == canonical_json_sha256(document, ("receipt_sha256",)),
            "AIM vacuum-control receipt canonical SHA256 is missing or invalid")
    require(document.get("case_id") == case_id == AIM_SUPPORTED_CASE,
            "AIM vacuum-control receipt is for a different benchmark case")
    require(document.get("source_deck_sha256") == source_deck_sha256,
            "AIM vacuum-control receipt is bound to a different source deck")
    recorded_beta = document.get("beta")
    require(isinstance(recorded_beta, (int, float)) and not isinstance(recorded_beta, bool)
            and math.isfinite(recorded_beta) and recorded_beta == float(beta),
            "AIM vacuum-control beta differs from requested production beta")
    require(document.get("runtime_identity") == runtime,
            "AIM vacuum-control receipt used a different CLI, library, Python API or nuclear-data identity")
    require(document.get("imp0_control_receipt") == imp0_control,
            "AIM vacuum-control receipt is not bound to the currently selected passing imp0 control")
    levels = document.get("radius_levels")
    require(isinstance(levels, list) and [row.get("radius_cm") for row in levels] == list(AIM_CONTROL_RADII_CM),
            "AIM vacuum-control receipt omits one of the three detector radii")
    artifacts = document.get("solver_artifacts")
    require(isinstance(artifacts, list) and len(artifacts) == len(AIM_CONTROL_RADII_CM),
            "AIM vacuum-control receipt lacks all three hashed solver artifacts")
    for index, (level, artifact, radius) in enumerate(zip(levels, artifacts, AIM_CONTROL_RADII_CM, strict=True)):
        require(isinstance(level, dict) and isinstance(artifact, dict)
                and level.get("radius_cm") == radius and artifact.get("radius_cm") == radius,
                "AIM vacuum-control radius or artifact order differs")
        require(level.get("status") == "PASS"
                and set(row.get("mcnp_tally_number") for row in level.get("results", [])) == set(AIM_REQUIRED_TALLIES)
                and all(row.get("passed") is True for row in level.get("results", [])),
                "AIM vacuum-control receipt lacks a passing result for every one of the four FNS detector points")
        expected_plan = source_aperture_plan(parsed, radius, beta)
        require(level.get("source_aperture") == expected_plan,
                "AIM vacuum-control source apertures differ from current detector centers, radius or beta")
        expected_positions = {int(row["tally"]): row["position_cm"] for row in parsed["detectors"]}
        require({int(row["mcnp_tally_number"]): row["position_cm"] for row in level["results"]}
                == expected_positions,
                "AIM vacuum-control result coordinates differ from the four pinned detector points")
        require(level.get("patch_statepoint", {}).get("lost_particles") == 0
                and level.get("patch_statepoint", {}).get("intentional_kill_events") == 0
                and level.get("patch_statepoint", {}).get("source_birth_kill_events") == 0
                and level.get("patch_statepoint", {}).get("cell_entry_kill_events") == 0,
                "AIM vacuum-control receipt reports loss or MCNP importance kills")
        statepoint_path = Path(artifact.get("statepoint_path", ""))
        require(statepoint_path.is_file() and hash_file(statepoint_path) == artifact.get("statepoint_sha256"),
                f"AIM vacuum-control statepoint {index} is missing or changed")
        child = read_imp0_statepoint(statepoint_path)
        require(child == level["patch_statepoint"],
                "AIM vacuum-control record diagnostics differ from its hashed child statepoint")
    return {"path": str(path.resolve()), "sha256": hash_file(path), "receipt_sha256": recorded_hash,
            "status": "PASS", "radii_cm": list(AIM_CONTROL_RADII_CM),
            "all_four_detector_tallies": list(AIM_REQUIRED_TALLIES)}


def hash_file(path: Path, limit: int | None = None) -> str:
    size = path.stat().st_size
    if limit is not None:
        require(size <= limit, f"{path} exceeds {limit} byte intake limit")
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def source_inputs(upstream: Path, manifest: dict) -> tuple[dict, Path, dict]:
    require(manifest["source"]["commit"] == SOURCE_COMMIT, "manifest source commit is not the pinned IAEA-NDS input commit")
    require(not verify_files(manifest, upstream), "one or more external source files do not match their pinned SHA256")
    deck_rel = manifest["adapter"]["source_deck"]
    deck = upstream / deck_rel
    require(deck.stat().st_size <= MAX_SOURCE_BYTES, "MCNP source input exceeds bounded parser limit")
    parsed = input_deck(deck.read_text(encoding="utf-8"))
    by_angle = {round(item["angle_deg"], 1): item for item in parsed["detectors"]}
    return manifest, deck, {"deck": parsed, "detectors_by_angle": by_angle}


def response_table(upstream: Path, manifest: dict, table: dict) -> list[dict]:
    path = upstream / table["path_in_source"]
    data = path.read_bytes()
    require(len(data) <= MAX_SOURCE_BYTES, f"{path} exceeds bounded CSV input limit")
    require(sha256_bytes(data) == table["sha256"], f"reference CSV hash mismatch: {path}")
    rows = read_rows(data, table["path_in_source"])
    return rows


def plan_document(manifest: dict, upstream: Path, cross_sections: Path, histories: int, batches: int, threads: int) -> dict:
    require(0 < histories <= MAX_PARTICLES, f"particle count must be in [1, {MAX_PARTICLES}]")
    require(0 < batches <= MAX_BATCHES and histories % batches == 0, "batch count must be <=500 and divide particles exactly")
    require(0 < threads <= MAX_THREADS, f"thread count must be in [1, {MAX_THREADS}]")
    _, deck_path, parsed = source_inputs(upstream, manifest)
    detector_count = len(manifest["detectors"])
    source_cells = mcnp_cell_semantics(parsed["deck"])
    xml_bytes = cross_sections.read_bytes() if cross_sections.is_file() else b""
    require(len(xml_bytes) <= MAX_CROSS_SECTIONS_XML_BYTES, "cross_sections.xml exceeds bounded metadata limit")
    return {
        "case_id": manifest["case_id"], "source_commit": SOURCE_COMMIT,
        "manifest_sha256": manifest_sha256(manifest), "input_deck": str(deck_path),
        "input_sha256": hash_file(deck_path, MAX_SOURCE_BYTES),
        "detectors": len(parsed["deck"]["detectors"]), "responses": detector_count,
        "tables": len(manifest["adapter"]["tables"]), "missing_angles_deg": manifest["adapter"]["missing_table_angles_deg"],
        "mcnp_neutron_importance": {
            "source_cells": [{"mcnp_cell_id": row["mcnp_cell_id"],
                               "material_id": row["material_id"],
                               "neutron_importance": row["neutron_importance"],
                               "source_card_sha256": row["source_card_sha256"],
                               "source_region_sha256": row["source_region_sha256"]}
                              for row in source_cells],
            "runtime_patch_version_required": FARIS_IMP0_PATCH_VERSION_REQUIRED,
            "runtime_control_receipt_required": True,
            "translation": "retained zero-importance cells receive a checked typed XML marker; removed cells are accepted only for a single-halfspace vacuum boundary",
        },
        "converter_dependency": {"distribution": "openmc_mcnp_adapter",
                                  "source_url": CONVERTER_SOURCE_URL,
                                  "source_commit": CONVERTER_COMMIT,
                                  "python_source_sha256": CONVERTER_PYTHON_SOURCE_SHA256,
                                  "distribution_version": CONVERTER_DISTRIBUTION_VERSION,
                                  "license": "MIT", "required_for_run": True,
                                  "verification_status": "NOT_EVALUATED; converter itself reports no methodical V&V"},
        "execution": {"openmc_version_required": OPENMC_REQUIRED, "particles_per_radius": histories,
                      "radii_cm": [2.5, 1.25, 0.625], "batches": batches, "threads": threads,
                      "maximum_particles": MAX_PARTICLES, "maximum_threads": MAX_THREADS,
                      "maximum_wall_seconds_per_radius": MAX_WALL_SECONDS,
                      "maximum_wall_seconds_total": MAX_WALL_SECONDS * 3,
                      "qualification": "two successive half-radius transitions; every bin <=1% relative change and both endpoint MCSE <=2%; zero/rare unresolved bins block sealing"},
        "cross_sections": {"path": str(cross_sections), "exists": cross_sections.is_file(),
                           "xml_sha256": sha256_bytes(xml_bytes) if xml_bytes else None},
        "limitations": ["finite spherical volume approximates MCNP point F5 tallies; qualification is required before sealing",
                        "converter is not methodically verified; exact inputs and conversion identities are recorded",
                        "source publication describes finite detector/collimator acceptance only approximately; this model does not simulate its detailed response",
                        "experimental Error confidence and covariance are unavailable; shared scoring remains INCONCLUSIVE"],
    }


def histogram_source(openmc, parsed: dict):
    spec = openmc_histogram(parsed["source_histogram"]["edges_mev"],
                            parsed["source_histogram"]["bin_probabilities"])
    return openmc.stats.Tabular(spec["edges_ev"], spec["pdf_per_ev"], interpolation="histogram")


def require_vacuum_outer_sphere(model, parsed: dict, importance_mapping: dict | None = None) -> int:
    """Check the converted 1000 cm MCNP SO boundary is a vacuum surface."""
    import re
    expected = []
    for line in parsed["surface_cards"]:
        match = re.match(r"^\s*(\d+)\s+so\s+([-+0-9.eE]+)\s*$", line, re.I)
        if match and math.isclose(float(match.group(2)), 1000.0, rel_tol=0, abs_tol=1e-9):
            expected.append(int(match.group(1)))
    require(len(expected) == 1, "source deck must have exactly one 1000 cm outer SO surface")
    surface = model.geometry.get_all_surfaces().get(expected[0])
    require(surface is not None, f"converted geometry lacks outer MCNP surface {expected[0]}")
    if surface.boundary_type != "vacuum":
        covered_by_marker = importance_mapping and any(
            row["action"] == "kill_on_entry_marker_required"
            and expected[0] in row["source_region_surface_ids"]
            for row in importance_mapping["cells"])
        require(bool(covered_by_marker),
                f"converted outer MCNP surface {expected[0]} boundary is {surface.boundary_type!r}, and no verified imp0 cell marker covers it")
    return expected[0]


def mcnp_cell_ids(parsed: dict) -> list[int]:
    """Return source cell IDs, including cells omitted from the converted root universe."""
    ids = []
    for line in parsed["cell_cards"]:
        match = re.match(r"^\s*(\d+)\b", line)
        require(match is not None, "source cell card lacks an integer ID")
        ids.append(int(match.group(1)))
    require(ids and len(ids) == len(set(ids)), "source cell cards have no IDs or duplicate IDs")
    return ids


def mcnp_cell_semantics(parsed: dict) -> list[dict]:
    """Read cell ID/material/neutron importance from joined source card text."""
    rows = []
    for line in parsed["cell_cards"]:
        match = re.match(r"^\s*(\d+)\s+(\d+)\b", line)
        require(match is not None, "unsupported source cell-card identity syntax")
        importance = re.findall(r"\bimp\s*:\s*n\s*=\s*([-+0-9.eE]+)", line, flags=re.I)
        require(len(importance) == 1, f"source cell {match.group(1)} must declare exactly one imp:n")
        value = float(importance[0])
        require(math.isfinite(value) and value >= 0, f"source cell {match.group(1)} has invalid imp:n")
        tokens = line.split()
        material_id = int(match.group(2))
        importance_index = next(i for i, token in enumerate(tokens) if re.fullmatch(r"imp:n=.*", token, re.I))
        region_start = 2 if material_id == 0 else 3
        region_tokens = tokens[region_start:importance_index]
        require(region_tokens, f"source cell {match.group(1)} has no parseable CSG region")
        source_region_text = " ".join(region_tokens)
        signature = re.findall(r"[+-]?\s*\d+|[:()]|#", source_region_text)
        require("#" not in signature, f"source cell {match.group(1)} uses unsupported complemented-cell geometry")
        signature = [re.sub(r"\s+", "", token) for token in signature]
        require("".join(signature) == re.sub(r"\s+", "", source_region_text),
                f"source cell {match.group(1)} uses unsupported CSG syntax")
        rows.append({"mcnp_cell_id": int(match.group(1)), "material_id": int(match.group(2)),
                     "neutron_importance": value, "source_card_sha256": sha256_bytes(line.encode("utf-8")),
                     "source_region_signature": signature,
                     "source_region_sha256": sha256_bytes(" ".join(signature).encode("utf-8"))})
    ids = [row["mcnp_cell_id"] for row in rows]
    require(ids and len(ids) == len(set(ids)), "source cell cards have no IDs or duplicate IDs")
    return rows


def verified_cell_actions(source_rows: list[dict], source_regions: dict[int, str],
                          converted_cells: dict[int, object], surfaces: dict[int, object]) -> list[dict]:
    """Join MCNP cells to converted cells by ID and prove the zero-importance action."""
    mappings = []
    for source in source_rows:
        cell_id = source["mcnp_cell_id"]
        importance = source["neutron_importance"]
        region = source_regions.get(cell_id)
        require(region is not None, f"converter source parse lacks MCNP cell {cell_id}")
        require("#" not in region, f"MCNP cell {cell_id} uses an unsupported complement region in the zero-importance mapping")
        converter_signature = [re.sub(r"\s+", "", token)
                               for token in re.findall(r"[+-]?\s*\d+|[:()]|#", region)]
        require("".join(converter_signature) == re.sub(r"\s+", "", region),
                f"pinned converter source parse uses unsupported CSG syntax at cell {cell_id}")
        require(converter_signature == source["source_region_signature"],
                f"MCNP cell {cell_id} source CSG tokens differ from the pinned converter parse")
        region_surface_ids = sorted({int(token) for token in re.findall(r"(?<![A-Za-z])[-+]?\s*(\d+)", region)})
        converted = converted_cells.get(cell_id)
        row = {**source, "openmc_cell_id": cell_id if converted is not None else None,
               "converter_region_sha256": sha256_bytes(region.encode("utf-8")),
               "source_region_surface_ids": region_surface_ids}
        if converted is not None:
            fill = converted.fill
            if source["material_id"] == 0:
                require(fill is None, f"MCNP void cell {cell_id} converted with a non-void fill")
                row["openmc_material_id"] = 0
            else:
                require(fill is not None and getattr(fill, "id", None) == source["material_id"],
                        f"MCNP cell {cell_id} material ID differs from converted OpenMC fill")
                row["openmc_material_id"] = int(fill.id)
            row["action"] = "kill_on_entry_marker_required" if importance == 0 else "retained_live_cell"
        else:
            require(importance == 0 and source["material_id"] == 0,
                    f"MCNP nonzero-importance or material cell {cell_id} is missing from converted geometry")
            simple_surface = re.fullmatch(r"\s*([+-]?\s*\d+)\s*", region)
            require(simple_surface is not None,
                    f"removed MCNP zero-importance cell {cell_id} is not a single-halfspace vacuum mapping")
            surface_id = abs(int(simple_surface.group(1).replace(" ", "")))
            surface = surfaces.get(surface_id)
            require(surface is not None and surface.boundary_type == "vacuum",
                    f"removed MCNP zero-importance cell {cell_id} surface {surface_id} is not a converted vacuum boundary")
            row["openmc_cell_id"] = None
            row["openmc_material_id"] = None
            row["vacuum_boundary_surface_id"] = surface_id
            row["action"] = "single_halfspace_vacuum_boundary"
        mappings.append(row)
    return mappings


def verified_importance_mapping(model, deck_path: Path, parsed: dict) -> dict:
    """Require an ID/material/region-backed action for every source cell."""
    try:
        from openmc_mcnp_adapter.parse import parse as parse_mcnp
    except ImportError as exc:
        raise ValueError("pinned openmc_mcnp_adapter parser is required for source-cell mapping") from exc
    adapter_cells, _adapter_surfaces, _adapter_data = parse_mcnp(str(deck_path))
    source_rows = mcnp_cell_semantics(parsed)
    require(len(source_rows) == len(adapter_cells),
            "independent source-card parser and pinned converter parser disagree on cell count")
    source_regions = {}
    by_id = {}
    for cell in adapter_cells:
        cell_id = int(cell["id"])
        by_id[cell_id] = cell
        source_regions[cell_id] = str(cell["region"])
    for row in source_rows:
        cell = by_id.get(row["mcnp_cell_id"])
        require(cell is not None and int(cell["material"]) == row["material_id"],
                f"source-cell ID/material differs between independent and converter parsers at cell {row['mcnp_cell_id']}")
        parsed_imp = float(cell.get("parameters", {}).get("imp:n", "nan"))
        require(math.isfinite(parsed_imp) and math.isclose(parsed_imp, row["neutron_importance"], rel_tol=0, abs_tol=0),
                f"source-cell importance differs between independent and converter parsers at cell {row['mcnp_cell_id']}")
    actions = verified_cell_actions(source_rows, source_regions,
                                    model.geometry.get_all_cells(), model.geometry.get_all_surfaces())
    source_point = tuple(float(x) for x in parsed["source_position_cm"])
    located = model.geometry.find(source_point)
    require(located, "converted geometry cannot locate the declared source point")
    source_cell_id = int(located[-1].id)
    source_row = next((row for row in actions if row["mcnp_cell_id"] == source_cell_id), None)
    require(source_row is not None and source_row["neutron_importance"] > 0,
            f"declared source is located in MCNP zero-importance cell {source_cell_id}; patched source-birth semantics are required")
    return {"cells": actions, "source_cell_id": source_cell_id,
            "source_birth_importance": source_row["neutron_importance"],
            "required_patch_marker_cell_ids": [row["mcnp_cell_id"] for row in actions
                                               if row["action"] == "kill_on_entry_marker_required"]}


def parse_ldd_openmc_library(output: str) -> Path:
    """Find the one libopenmc selected by the dynamic loader for the CLI."""
    found = []
    for line in output.splitlines():
        tokens = line.strip().split()
        if not tokens:
            continue
        if tokens[0].startswith("libopenmc.so") and len(tokens) >= 3 and tokens[1] == "=>" and tokens[2] != "not":
            found.append(Path(tokens[2]))
        elif tokens[0].startswith("/") and "libopenmc.so" in tokens[0]:
            found.append(Path(tokens[0]))
    unique = list(dict.fromkeys(found))
    require(len(unique) == 1, f"selected OpenMC CLI must resolve exactly one libopenmc.so, found {len(unique)}")
    return unique[0].resolve(strict=True)


def require_imp0_patch_version(version: int) -> None:
    require(version == FARIS_IMP0_PATCH_VERSION_REQUIRED,
            f"selected OpenMC runtime reports MCNP imp0 patch version {version}, expected {FARIS_IMP0_PATCH_VERSION_REQUIRED}")


def selected_openmc_runtime_identity(executable: Path, env: dict[str, str],
                                     runner=subprocess.run, python: str = sys.executable) -> dict:
    """Prove the selected CLI resolves a linked library with the required patch symbol."""
    require(not env.get("LD_PRELOAD") and not env.get("LD_AUDIT"),
            "LD_PRELOAD/LD_AUDIT can override the linked-library proof; clear them before the FNS run")
    library_path = env.get("LD_LIBRARY_PATH", "")
    if library_path:
        entries = library_path.split(":")
        require(all(entry and Path(entry).is_absolute() for entry in entries),
                "LD_LIBRARY_PATH entries must be absolute and nonempty for the child-library proof")
    linked = runner(["ldd", str(executable)], capture_output=True, text=True, env=env)
    require(linked.returncode == 0, f"could not resolve selected OpenMC CLI shared libraries: {linked.stderr[-1000:]}")
    library = parse_ldd_openmc_library(linked.stdout)
    probe = ("import ctypes,sys; lib=ctypes.CDLL(sys.argv[1]); "
             f"print(ctypes.c_int.in_dll(lib, {FARIS_IMP0_PATCH_VERSION_SYMBOL!r}).value)")
    result = runner([python, "-c", probe, str(library)], capture_output=True, text=True, env=env)
    require(result.returncode == 0,
            "selected OpenMC CLI's linked libopenmc lacks the FARIS MCNP imp0 capability symbol")
    try:
        version = int(result.stdout.strip())
    except ValueError as exc:
        raise ValueError("selected OpenMC library returned a malformed MCNP imp0 capability version") from exc
    require_imp0_patch_version(version)
    return {"executable_path": str(executable.resolve(strict=True)),
            "executable_sha256": hash_file(executable.resolve(strict=True)),
            "linked_library_path": str(library), "linked_library_sha256": hash_file(library),
            "patch_version_symbol": FARIS_IMP0_PATCH_VERSION_SYMBOL,
            "patch_version": version, "capability_source": "ctypes probe of ldd-resolved libopenmc.so"}


def scalar_int_dataset(value, name: str) -> int:
    """Decode one scalar or one-element integer HDF5 dataset without coercion."""
    shape = tuple(value.shape)
    require(shape in {(), (1,)}, f"statepoint dataset {name} must contain exactly one scalar")
    raw = value[()] if shape == () else value[0]
    if hasattr(raw, "item"):
        raw = raw.item()
    require(isinstance(raw, int) and not isinstance(raw, bool),
            f"statepoint dataset {name} must be an integer")
    return raw


def read_imp0_statepoint(path: Path) -> dict:
    """Read the child solver's dedicated imp0 patch datasets from its statepoint."""
    try:
        import h5py
    except ImportError as exc:
        raise ValueError("h5py is required to verify MCNP imp:n=0 statepoint evidence") from exc
    with h5py.File(path, "r") as statepoint:
        require(FARIS_IMP0_PATCH_VERSION_DATASET in statepoint,
                "statepoint omits child MCNP imp0 patch-version dataset")
        require(FARIS_IMP0_PATCH_KILL_EVENTS_DATASET in statepoint,
                "statepoint omits child MCNP imp0 intentional-kill dataset")
        require(FARIS_IMP0_PATCH_BIRTH_KILL_DATASET in statepoint,
                "statepoint omits child MCNP imp0 source-birth kill dataset")
        require(FARIS_IMP0_PATCH_ENTRY_KILL_DATASET in statepoint,
                "statepoint omits child MCNP imp0 entry kill dataset")
        require(FARIS_OPENMC_LOST_DATASET in statepoint,
                "statepoint omits the child OpenMC lost-particle counter")
        for name in (STATEPOINT_CURRENT_BATCH_DATASET, STATEPOINT_N_BATCHES_DATASET,
                     STATEPOINT_N_PARTICLES_DATASET, STATEPOINT_N_REALIZATIONS_DATASET):
            require(name in statepoint, f"statepoint omits standard run metadata dataset {name}")
        version = scalar_int_dataset(statepoint[FARIS_IMP0_PATCH_VERSION_DATASET], FARIS_IMP0_PATCH_VERSION_DATASET)
        kills = scalar_int_dataset(statepoint[FARIS_IMP0_PATCH_KILL_EVENTS_DATASET], FARIS_IMP0_PATCH_KILL_EVENTS_DATASET)
        birth_kills = scalar_int_dataset(statepoint[FARIS_IMP0_PATCH_BIRTH_KILL_DATASET], FARIS_IMP0_PATCH_BIRTH_KILL_DATASET)
        entry_kills = scalar_int_dataset(statepoint[FARIS_IMP0_PATCH_ENTRY_KILL_DATASET], FARIS_IMP0_PATCH_ENTRY_KILL_DATASET)
        lost_particles = scalar_int_dataset(statepoint[FARIS_OPENMC_LOST_DATASET], FARIS_OPENMC_LOST_DATASET)
        current_batch = scalar_int_dataset(statepoint[STATEPOINT_CURRENT_BATCH_DATASET], STATEPOINT_CURRENT_BATCH_DATASET)
        n_batches = scalar_int_dataset(statepoint[STATEPOINT_N_BATCHES_DATASET], STATEPOINT_N_BATCHES_DATASET)
        n_particles = scalar_int_dataset(statepoint[STATEPOINT_N_PARTICLES_DATASET], STATEPOINT_N_PARTICLES_DATASET)
        n_realizations = scalar_int_dataset(statepoint[STATEPOINT_N_REALIZATIONS_DATASET], STATEPOINT_N_REALIZATIONS_DATASET)
        raw_version = statepoint.attrs.get("openmc_version")
        require(raw_version is not None, "statepoint omits its OpenMC solver-version attribute")
        if hasattr(raw_version, "tolist"):
            raw_version = raw_version.tolist()
        require(isinstance(raw_version, (list, tuple)) and len(raw_version) == 3,
                "statepoint OpenMC version attribute must be a three-part tuple")
        version_parts = []
        for part in raw_version:
            if hasattr(part, "item"):
                part = part.item()
            require(isinstance(part, int) and not isinstance(part, bool),
                    "statepoint OpenMC version components must be integers")
            version_parts.append(part)
        solver_version = ".".join(str(part) for part in version_parts)
    require_imp0_patch_version(version)
    require(kills >= 0 and birth_kills >= 0 and entry_kills >= 0 and lost_particles >= 0,
            "statepoint imp0 or lost-particle counters cannot be negative")
    require(kills == birth_kills + entry_kills,
            "statepoint total MCNP imp0 kills differ from birth+entry counters")
    return {"patch_version": version, "intentional_kill_events": kills,
            "source_birth_kill_events": birth_kills, "cell_entry_kill_events": entry_kills,
            "lost_particles": lost_particles, "current_batch": current_batch,
            "n_batches": n_batches, "n_particles": n_particles,
            "n_realizations": n_realizations, "solver_version": solver_version,
            "source": "child statepoint datasets; imp0 kills and normal lost count are distinct"}


def require_imp0_control_statepoint_metadata(statepoint: dict, case_name: str) -> None:
    """Require exact one-batch control metadata; particles are not realizations."""
    expected = {"current_batch": 1, "n_batches": 1, "n_particles": 1000,
                "n_realizations": 1, "solver_version": OPENMC_REQUIRED}
    for key, value in expected.items():
        require(statepoint.get(key) == value,
                f"MCNP imp0 control {case_name} statepoint {key}={statepoint.get(key)!r}, expected {value!r}")


def imp0_control_receipt_sha256(document: dict) -> str:
    """Hash the canonical control JSON with its self-digest field excluded."""
    unsigned = dict(document)
    unsigned.pop("receipt_sha256", None)
    canonical = json.dumps(unsigned, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    return sha256_bytes(canonical)


def verify_imp0_control_receipt(path: Path, runtime: dict) -> dict:
    """Require the small source-birth/entry/photon/unmarked control on this exact runtime."""
    require(path.is_file(), "the patched OpenMC MCNP imp0 runtime-control receipt is required before any FNS transport")
    require(path.stat().st_size <= MAX_IMP0_CONTROL_BYTES,
            "MCNP imp0 control receipt exceeds the bounded JSON input size")
    receipt = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(receipt, dict), "MCNP imp0 control receipt must be a JSON object")
    receipt_sha256 = receipt.get("receipt_sha256")
    require(isinstance(receipt_sha256, str) and re.fullmatch(r"[0-9a-f]{64}", receipt_sha256) is not None
            and receipt_sha256 == imp0_control_receipt_sha256(receipt),
            "MCNP imp0 control canonical receipt SHA256 is missing or invalid")
    require(receipt.get("schema") == "faris.openmc-mcnp-imp0-control/1.0.0" and receipt.get("status") == "PASS",
            "MCNP imp0 runtime-control receipt has wrong schema or did not pass")
    recorded = receipt.get("runtime", {})
    require(isinstance(recorded, dict), "MCNP imp0 control runtime identity must be a JSON object")
    require(Path(recorded.get("selected_cli_path", "")).resolve() == Path(runtime["executable_path"]),
            "MCNP imp0 control used a different OpenMC CLI path")
    require(recorded.get("selected_cli_sha256") == runtime["executable_sha256"],
            "MCNP imp0 control used a different OpenMC CLI binary")
    require(Path(recorded.get("selected_linked_library_path", "")).resolve() == Path(runtime["linked_library_path"]),
            "MCNP imp0 control used a different linked libopenmc path")
    require(recorded.get("selected_linked_library_sha256") == runtime["linked_library_sha256"],
            "MCNP imp0 control used a different linked libopenmc binary")
    require(recorded.get("faris_patch_version_symbol") == runtime["patch_version"],
            "MCNP imp0 control patch capability version differs from the selected runtime")
    require(recorded.get("openmc_python_version") == OPENMC_REQUIRED,
            "MCNP imp0 control used a different OpenMC Python API version")
    require(Path(recorded.get("python_path", "")).resolve() == Path(sys.executable).resolve()
            and recorded.get("python_sha256") == hash_file(Path(sys.executable).resolve(strict=True)),
            "MCNP imp0 control used a different Python interpreter")
    require(Path(recorded.get("openmc_python_module_path", "")).resolve()
            == Path(runtime["openmc_python_module_path"])
            and recorded.get("openmc_python_module_sha256") == runtime["openmc_python_module_sha256"],
            "MCNP imp0 control used a different OpenMC Python module")
    control_xs_path = recorded.get("cross_sections_path")
    require(isinstance(control_xs_path, str), "MCNP imp0 control omits its cross_sections.xml path")
    control_xs = Path(control_xs_path).resolve(strict=True)
    require(control_xs.is_file() and control_xs.stat().st_size <= MAX_CROSS_SECTIONS_XML_BYTES
            and hash_file(control_xs, MAX_CROSS_SECTIONS_XML_BYTES) == recorded.get("cross_sections_sha256"),
            "MCNP imp0 control cross_sections.xml is missing or differs from its recorded hash")
    require(control_xs == Path(runtime["cross_sections_path"])
            and recorded["cross_sections_sha256"] == runtime["cross_sections_xml_sha256"],
            "MCNP imp0 control used a different cross_sections.xml index")
    cases = {}
    control_cases = receipt.get("cases", [])
    require(isinstance(control_cases, list), "MCNP imp0 control cases must be a JSON array")
    for row in control_cases:
        require(isinstance(row, dict), "MCNP imp0 control case must be a JSON object")
        case = row.get("case", {})
        require(isinstance(case, dict), "MCNP imp0 control case identity must be a JSON object")
        name = case.get("name")
        require(name not in cases, f"MCNP imp0 control repeats case {name!r}")
        cases[name] = row
    expected = {
        "entry_neutron": (1000, 0, 1000),
        "source_birth_neutron": (1000, 1000, 0),
        "marked_photon": (0, 0, 0),
        "unmarked_neutron": (0, 0, 0),
    }
    require(set(cases) == set(expected), "MCNP imp0 control must contain all four required cases")
    for name, (total, birth, entry) in expected.items():
        row = cases[name]
        require(isinstance(row.get("statepoint"), str), f"MCNP imp0 control statepoint path is invalid for {name}")
        statepoint = Path(row["statepoint"])
        if not statepoint.is_absolute():
            statepoint = path.parent / statepoint
        require(statepoint.is_file() and statepoint.stat().st_size <= MAX_IMP0_CONTROL_STATEPOINT_BYTES
                and hash_file(statepoint) == row.get("statepoint_sha256"),
                f"MCNP imp0 control statepoint missing or hash mismatch for {name}")
        diagnostics = row.get("diagnostics", {})
        require(isinstance(diagnostics, dict), f"MCNP imp0 control diagnostics must be a JSON object for {name}")
        child = read_imp0_statepoint(statepoint)
        require_imp0_control_statepoint_metadata(child, name)
        require(diagnostics.get("faris_mcnp_imp0_patch_version") == runtime["patch_version"],
                f"MCNP imp0 statepoint version differs for {name}")
        require(diagnostics.get("faris_mcnp_imp0_kill_events") == total
                and diagnostics.get("faris_mcnp_imp0_birth_kill_events") == birth
                and diagnostics.get("faris_mcnp_imp0_entry_kill_events") == entry,
                f"MCNP imp0 child kill-counter control failed for {name}")
        require(diagnostics.get("faris_openmc_n_lost_particles") == 0,
                f"MCNP imp0 control reports lost particles for {name}")
        require(child["patch_version"] == diagnostics["faris_mcnp_imp0_patch_version"]
                and child["intentional_kill_events"] == diagnostics["faris_mcnp_imp0_kill_events"]
                and child["source_birth_kill_events"] == diagnostics["faris_mcnp_imp0_birth_kill_events"]
                and child["cell_entry_kill_events"] == diagnostics["faris_mcnp_imp0_entry_kill_events"]
                and child["lost_particles"] == diagnostics["faris_openmc_n_lost_particles"],
                f"MCNP imp0 control diagnostics do not match child statepoint datasets for {name}")
        report_metadata = {"statepoint_current_batch": child["current_batch"],
                            "statepoint_n_batches": child["n_batches"],
                            "statepoint_n_particles": child["n_particles"],
                            "statepoint_n_realizations": child["n_realizations"],
                            "solver_version": child["solver_version"]}
        for field, value in report_metadata.items():
            require(row.get(field) == value,
                    f"MCNP imp0 control report {field} differs from child statepoint for {name}")
        geometry_hash = row.get("geometry_xml_sha256", "")
        require(isinstance(geometry_hash, str) and re.fullmatch(r"[0-9a-f]{64}", geometry_hash) is not None,
                f"MCNP imp0 control lacks final geometry XML hash for {name}")
        control_expected = row.get("expected", {})
        require(isinstance(control_expected, dict), f"MCNP imp0 control expected values must be a JSON object for {name}")
        require(control_expected.get("kill") == total and control_expected.get("birth") == birth
                and control_expected.get("entry") == entry,
                f"MCNP imp0 control expected counters differ for {name}")
        flux = row.get("cell_flux_means_per_source", [])
        expected_flux = control_expected.get("flux", [])
        current = row.get("middle_surface_current_mean_per_source")
        expected_current = control_expected.get("current")
        require(isinstance(flux, list) and isinstance(expected_flux, list) and len(flux) == len(expected_flux) == 3,
                f"MCNP imp0 control lacks all three cell-flux values for {name}")
        try:
            flux_ok = all(math.isfinite(float(value)) and math.isfinite(float(target))
                          and abs(float(value) - float(target)) <= 1.0e-10
                          for value, target in zip(flux, expected_flux, strict=True))
            current_ok = (current is not None and expected_current is not None
                          and math.isfinite(float(current)) and math.isfinite(float(expected_current))
                          and abs(float(current) - float(expected_current)) <= 1.0e-10)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"MCNP imp0 control contains nonnumeric measurements for {name}") from exc
        require(flux_ok, f"MCNP imp0 control live-cell flux checks failed for {name}")
        require(current_ok, f"MCNP imp0 control surface-current check failed for {name}")
    return {"path": str(path.resolve()), "sha256": hash_file(path),
            "schema": receipt["schema"], "status": receipt["status"],
            "matched_runtime": {"executable_sha256": runtime["executable_sha256"],
                                "linked_library_sha256": runtime["linked_library_sha256"],
                                "patch_version": runtime["patch_version"],
                                "openmc_python_module_sha256": runtime["openmc_python_module_sha256"]},
            "cross_sections_path": str(control_xs),
            "cross_sections_sha256": recorded["cross_sections_sha256"],
            "case_names": sorted(cases)}


def load_verified_importance_sidecar(path: Path, expected_sha256: str, source_deck_sha256: str) -> dict:
    raw = path.read_bytes()
    require(sha256_bytes(raw) == expected_sha256, "MCNP importance sidecar SHA256 mismatch")
    document = json.loads(raw)
    require(document.get("schema") == "faris.fns-importance-map/1.0.0",
            "unsupported MCNP importance sidecar schema")
    require(document.get("source_deck_sha256") == source_deck_sha256,
            "MCNP importance sidecar is bound to a different source deck")
    ids = [row["mcnp_cell_id"] for row in document.get("cells", [])
           if row.get("action") == "kill_on_entry_marker_required"]
    require(ids and len(ids) == len(set(ids)), "MCNP importance sidecar has no unique marker targets")
    return document


def inject_importance_xml_markers(geometry_xml: Path, sidecar_path: Path, case_id: str,
                                  source_deck_sha256: str, converter: dict,
                                  importance_mapping: dict) -> dict:
    """Write/read-back-check the cell mapping, then inject typed XML markers."""
    marker_ids = importance_mapping["required_patch_marker_cell_ids"]
    require(marker_ids, "no retained imp:n=0 cells require the patched OpenMC marker")
    cells = importance_mapping["cells"]
    payload = {"schema": "faris.fns-importance-map/1.0.0", "case_id": case_id,
               "source_deck_sha256": source_deck_sha256,
               "converter": {"source_commit": converter["source_commit"],
                             "python_source_sha256": converter["python_source_sha256"]},
               "source_cell_id": importance_mapping["source_cell_id"],
               "source_birth_importance": importance_mapping["source_birth_importance"],
               "cells": cells}
    raw = (json.dumps(payload, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode("utf-8")
    require(not sidecar_path.exists(), f"refusing to overwrite MCNP importance sidecar: {sidecar_path}")
    sidecar_path.write_bytes(raw)
    sidecar_sha256 = sha256_bytes(raw)
    checked = load_verified_importance_sidecar(sidecar_path, sidecar_sha256, source_deck_sha256)
    checked_markers = {row["mcnp_cell_id"] for row in checked["cells"]
                       if row["action"] == "kill_on_entry_marker_required"}
    require(checked_markers == set(marker_ids), "checked sidecar marker set differs from the verified converter mapping")
    require(geometry_xml.is_file() and geometry_xml.stat().st_size <= MAX_MODEL_XML_BYTES,
            "exported geometry.xml is missing or exceeds its bounded size")
    unmarked_sha256 = hash_file(geometry_xml, MAX_MODEL_XML_BYTES)
    tree = ET.parse(geometry_xml)
    require(tree.getroot().tag == "geometry", "OpenMC export did not produce standard geometry.xml")
    nodes = {}
    for node in tree.getroot().iter("cell"):
        raw_id = node.get("id")
        require(raw_id is not None, "geometry.xml cell has no ID")
        cell_id = int(raw_id)
        require(cell_id not in nodes, f"geometry.xml repeats cell ID {cell_id}")
        nodes[cell_id] = node
    for cell_id in marker_ids:
        node = nodes.get(cell_id)
        require(node is not None, f"geometry.xml lacks verified MCNP zero-importance cell {cell_id}")
        require(node.get("region") is not None, f"marked OpenMC cell {cell_id} lacks a CSG region")
        require(node.get("faris_neutron_importance") is None,
                f"OpenMC cell {cell_id} already contains an unreviewed FARIS marker")
        node.set("faris_neutron_importance", "0")
    ET.indent(tree, space="  ")
    tree.write(geometry_xml, encoding="utf-8", xml_declaration=True)
    final_sha256 = hash_file(geometry_xml, MAX_MODEL_XML_BYTES)
    verify_tree = ET.parse(geometry_xml)
    final_markers = {int(node.get("id")) for node in verify_tree.getroot().iter("cell")
                     if node.get("faris_neutron_importance") is not None}
    require(final_markers == set(marker_ids), "final geometry.xml marker set differs from the checked sidecar")
    for node in verify_tree.getroot().iter("cell"):
        if node.get("faris_neutron_importance") is not None:
            require(node.get("faris_neutron_importance") == "0", "unexpected FARIS neutron-importance marker value")
    return {"sidecar_path": str(sidecar_path), "sidecar_sha256": sidecar_sha256,
            "pre_injection_geometry_sha256": unmarked_sha256,
            "final_geometry_path": str(geometry_xml), "final_geometry_sha256": final_sha256,
            "marker_cell_ids": sorted(marker_ids)}


def energy_edges_for_table(upstream: Path, table: dict, parsed: dict) -> tuple[list[float], list[dict]]:
    rows = response_table(upstream, {}, table)
    fine = not table["coarse"]
    bins = energy_bins(rows, parsed["fine_energy_upper_edges_mev"] if fine else None, fine)
    return tally_energy_edges(bins), bins


def detector_cell_geometry(openmc, model, points: list[dict], radius: float,
                           first_cell_id: int | None = None) -> dict[int, object]:
    """Subtract exact centered detector spheres from their containing void cells."""
    cells = model.geometry.get_all_cells()
    next_cell = max(max(cells, default=0) + 1, first_cell_id or 1)
    next_surface = max(model.geometry.get_all_surfaces(), default=0) + 1
    result = {}
    spheres_by_host: dict[int, list[object]] = {}
    centers = [tuple(float(x) for x in p["position_cm"]) for p in points]
    for i, a in enumerate(centers):
        for b in centers[i + 1:]:
            separation = math.sqrt(math.fsum((a[k] - b[k]) ** 2 for k in range(3)))
            require(separation > 2.0 * radius, "finite detector spheres overlap at this radius")
    for point in points:
        xyz = tuple(float(x) for x in point["position_cm"])
        # Geometry.find() uses the converted CSG itself and avoids positional or
        # cell-order guesses when selecting the surrounding void region.
        found = model.geometry.find(xyz)
        hosts = [found[-1]] if found and found[-1] in cells.values() else []
        require(len(hosts) == 1, f"F{point['tally']} detector center maps to {len(hosts)} host cells")
        host = hosts[0]
        require(host.fill is None, f"F{point['tally']} detector center is in a material/universe-filled cell")
        # Check a deterministic set of interior directions against the converted
        # cell map. This catches a detector volume clipped by a nearby boundary;
        # it is a topology sanity check, not a proof of geometry equivalence.
        dirs = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1),
                (1, 1, 1), (1, 1, -1), (1, -1, 1), (-1, 1, 1),
                (-1, -1, 1), (-1, 1, -1), (1, -1, -1), (-1, -1, -1)]
        norm = math.sqrt(3.0)
        for direction in dirs:
            scale = 0.95 * radius / (norm if direction[0] and direction[1] and direction[2] else 1.0)
            sample = tuple(xyz[k] + scale * direction[k] for k in range(3))
            found_sample = model.geometry.find(sample)
            require(found_sample and found_sample[-1].id == host.id,
                    f"F{point['tally']} radius-{radius:g}cm sphere is clipped by a converted geometry boundary")
        sphere = openmc.Sphere(surface_id=next_surface, x0=xyz[0], y0=xyz[1], z0=xyz[2], r=radius)
        next_surface += 1
        cell = openmc.Cell(cell_id=next_cell, name=f"f{point['tally']}_finite_detector_r{radius:g}cm", region=-sphere)
        next_cell += 1
        cell.fill = None
        spheres_by_host.setdefault(host.id, []).append(sphere)
        model.geometry.root_universe.add_cell(cell)
        result[point["tally"]] = {"cell": cell, "host_cell_id": host.id,
                                  "position_cm": list(xyz), "radius_cm": radius,
                                  "volume_cm3": sphere_volume_cm3(radius),
                                  "sampled_host_containment_directions": len(dirs)}
    for host_id, spheres in spheres_by_host.items():
        host = cells[host_id]
        for sphere in spheres:
            host.region = host.region & +sphere
    model.geometry.determine_paths()
    model.geometry.get_all_cells()
    return result


def sphere_volume_cm3(radius_cm: float) -> float:
    """Exact analytic volume for the finite spherical cell tally, in cm^3."""
    radius = float(radius_cm)
    require(math.isfinite(radius) and radius > 0,
            "finite-volume detector radius must be positive and finite")
    volume = (4.0 / 3.0) * math.pi * radius * radius * radius
    require(math.isfinite(volume) and volume > 0,
            "finite-volume detector sphere volume must be positive and finite")
    return volume


def tally_value(mean: float, std: float, factor: float, lo_mev: float, hi_mev: float,
                volume_cm3: float, radius_cm: float) -> dict:
    """Convert integrated OpenMC tracklength flux to volume-averaged flux density.

    OpenMC cell-filter flux is an integral of track length over the cell
    (particle-cm/source). The JADE/MCNP response is flux per area, so both the
    mean and standard deviation must be divided by the exact sphere volume
    before applying the deck FM and lethargy-width conversion.
    """
    width = lethargy_width(lo_mev, hi_mev)
    values = (float(mean), float(std), float(factor), float(volume_cm3), float(radius_cm))
    require(all(math.isfinite(v) for v in values), "tally mean, standard deviation, FM and sphere geometry must be finite")
    require(values[0] >= 0 and values[1] >= 0 and values[2] > 0 and values[3] > 0 and values[4] > 0,
            "flux mean and standard deviation must be nonnegative and FM positive")
    require(math.isclose(sphere_volume_cm3(values[4]), values[3], rel_tol=1e-12, abs_tol=0.0),
            "finite-volume detector volume differs from the analytic sphere volume")
    flux_mean = values[0] / values[3]
    flux_std = values[1] / values[3]
    value = flux_mean * values[2] / width
    u_mc = flux_std * values[2] / width
    require(math.isfinite(value) and math.isfinite(u_mc), "normalized tally result is non-finite")
    return {"value": value, "u_mc": u_mc,
            "openmc_tracklength_mean_particle_cm_per_source": values[0],
            "openmc_tracklength_std_dev_particle_cm_per_source": values[1],
            "finite_volume_normalization": {
                "geometry": "analytic full-sphere cell; host containment diagnostics are recorded separately",
                "radius_cm": values[4], "volume_cm3": values[3],
                "flux_mean_particle_per_cm2_per_source_before_FM": flux_mean,
                "flux_std_dev_particle_per_cm2_per_source_before_FM": flux_std,
                "energy_normalization": "per unit lethargy",
                "mcnp_FM": values[2],
                "raw_score_units": "particle-cm per source particle",
                "normalized_flux_units": "particle/cm^2 per source particle"}}


def validate_response_mapping(manifest: dict, deck: dict) -> dict[str, dict]:
    """Verify that every declared result joins via the deck FC angle and tally ID."""
    points = {round(row["angle_deg"], 1): row for row in deck["detectors"]}
    require(len(points) == len(deck["detectors"]), "source deck has duplicate angle labels")
    mapped: dict[str, dict] = {}
    for table in manifest["adapter"]["tables"]:
        angle = round(table["angle_deg"], 1)
        require(angle in points, f"reference angle {angle:g} has no exact FC-labeled tally")
        point = points[angle]
        require(point["tally"] == table["tally_number"], f"angle {angle:g} maps to a different tally ID")
        require(math.isclose(point["mcnp_fm"], table["mcnp_FM"], rel_tol=0, abs_tol=1e-12),
                f"angle {angle:g} FM differs from the input deck")
        for row in table["detector_ids"]:
            did = row["detector_id"]
            require(did not in mapped, f"duplicate response mapping for {did}")
            mapped[did] = {"tally": point["tally"], "angle_deg": angle,
                           "coarse": bool(table["coarse"]), "bin_index": row["bin_index"],
                           "lo_mev": row["lo_mev"], "hi_mev": row["hi_mev"],
                           "mcnp_fm": point["mcnp_fm"]}
    declared = {d["id"] for d in manifest["detectors"]}
    require(set(mapped) == declared, "response mapping does not exactly cover every declared manifest detector")
    return mapped


def model_for_radius(openmc, deck_path: Path, upstream: Path, manifest: dict, parsed: dict,
                     radius: float, batches: int, histories: int, seed: int):
    try:
        from openmc_mcnp_adapter import mcnp_to_model
    except ImportError as exc:
        raise ValueError("install openmc-dev/openmc_mcnp_adapter at a pinned commit before running") from exc
    # Each radius is rebuilt as a separate OpenMC model. Reset global object-ID
    # registries before importing the next one; old, already-exported models are
    # not reused. This prevents cross-model ID warnings without hiding warnings.
    openmc.reset_auto_ids()
    model = mcnp_to_model(str(deck_path))
    importance_mapping = verified_importance_mapping(model, deck_path, parsed)
    require_vacuum_outer_sphere(model, parsed, importance_mapping)
    detector_cells = detector_cell_geometry(
        openmc, model, parsed["detectors"], radius,
        first_cell_id=max(mcnp_cell_ids(parsed)) + 1)
    src = openmc.IndependentSource(space=openmc.stats.Point(parsed["source_position_cm"]),
                                   angle=openmc.stats.Isotropic(), energy=histogram_source(openmc, parsed))
    model.settings = openmc.Settings()
    model.settings.run_mode = "fixed source"
    model.settings.source = [src]
    model.settings.particles = histories // batches
    model.settings.batches = batches
    model.settings.seed = seed
    model.settings.max_lost_particles = 100
    tallies = openmc.Tallies()
    by_angle = {round(d["angle_deg"], 1): d for d in parsed["detectors"]}
    filter_cache = {}
    for table in manifest["adapter"]["tables"]:
        point = by_angle[round(table["angle_deg"], 1)]
        edges, _ = energy_edges_for_table(upstream, table, parsed)
        tally = openmc.Tally(name=f"f{point['tally']}_{'coarse' if table['coarse'] else 'fine'}")
        cell = detector_cells[point["tally"]]["cell"]
        cell_key = ("cell", cell.id)
        if cell_key not in filter_cache:
            filter_cache[cell_key] = openmc.CellFilter(cell)
        energy_edges_ev = tuple(float(e * 1e6) for e in edges)
        energy_key = ("energy", energy_edges_ev)
        if energy_key not in filter_cache:
            filter_cache[energy_key] = openmc.EnergyFilter(energy_edges_ev)
        # Reuse equal filters across tallies. OpenMC's XML writer then emits a
        # single filter ID without renumbering equal-but-distinct objects.
        tally.filters = [filter_cache[cell_key], filter_cache[energy_key]]
        tally.scores = ["flux"]
        tallies.append(tally)
    model.tallies = tallies
    return model, detector_cells


def vacuum_aim_control_model(openmc, parsed: dict, radius_cm: float,
                             batches: int, histories: int, seed: int):
    """Build a no-material vacuum control with the actual four FNS detector points."""
    require(tuple(int(point["tally"]) for point in parsed["detectors"]) == AIM_REQUIRED_TALLIES,
            "vacuum AIM control requires exact FNS-Fe-5 four-detector coverage")
    openmc.reset_auto_ids()
    source_xyz = tuple(float(x) for x in parsed["source_position_cm"])
    positions = [tuple(float(x) for x in point["position_cm"]) for point in parsed["detectors"]]
    distances = [math.sqrt(math.fsum((p[k] - source_xyz[k]) ** 2 for k in range(3))) for p in positions]
    require(all(distance > radius_cm for distance in distances),
            "vacuum AIM detector sphere overlaps the point source")
    for i, p in enumerate(positions):
        for q in positions[i + 1:]:
            separation = math.sqrt(math.fsum((p[k] - q[k]) ** 2 for k in range(3)))
            require(separation > 2.0 * radius_cm, "vacuum AIM finite-detector spheres overlap")

    outer_radius = max(distances) + 10.0 * radius_cm
    outer = openmc.Sphere(surface_id=1, x0=source_xyz[0], y0=source_xyz[1], z0=source_xyz[2],
                          r=outer_radius, boundary_type="vacuum")
    spheres = [openmc.Sphere(surface_id=i + 2, x0=p[0], y0=p[1], z0=p[2], r=radius_cm)
               for i, p in enumerate(positions)]
    world_region = -outer
    for sphere in spheres:
        world_region &= +sphere
    cells = [openmc.Cell(cell_id=1, name="analytic_vacuum_world", region=world_region)]
    detector_cells = {}
    tallies = openmc.Tallies()
    for point, sphere in zip(parsed["detectors"], spheres, strict=True):
        tally_id = int(point["tally"])
        cell = openmc.Cell(cell_id=tally_id + 100, name=f"analytic_f{tally_id}_sphere",
                           fill=None, region=-sphere)
        cells.append(cell)
        detector_cells[tally_id] = {"cell": cell,
                                    "analytic_flux_cm-2_per_source":
                                        isotropic_sphere_volume_average_cm2(
                                            distances[len(detector_cells)], radius_cm)}
        tally = openmc.Tally(name=f"aim_vacuum_f{tally_id}")
        tally.filters = [openmc.CellFilter(cell)]
        tally.scores = ["flux"]
        tallies.append(tally)

    geometry = openmc.Geometry(openmc.Universe(cells=cells))
    require(all(geometry.find(p)[-1].id == detector_cells[int(point["tally"])] ["cell"].id
                for point, p in zip(parsed["detectors"], positions, strict=True)),
            "vacuum AIM control point-to-cell locator failed")
    source = openmc.IndependentSource(space=openmc.stats.Point(source_xyz),
                                      angle=openmc.stats.Isotropic(),
                                      energy=histogram_source(openmc, parsed))
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.source = [source]
    settings.particles = histories // batches
    settings.batches = batches
    settings.seed = seed
    # OpenMC requires a positive cutoff; the post-run patched statepoint gate
    # below still requires the observed lost-particle count to be exactly zero.
    settings.max_lost_particles = 1
    settings.statepoint = {"batches": [batches]}
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(),
                         settings=settings, tallies=tallies)
    return model, detector_cells


def evaluate_vacuum_aim_control(detector_rows: list[dict], patch_statepoint: dict,
                                max_relative_mcse: float = AIM_CONTROL_MAX_RELATIVE_MCSE,
                                max_abs_z: float = AIM_CONTROL_MAX_ABS_Z) -> dict:
    """Compare all four weighted vacuum cell-flux estimates with the analytic sphere mean."""
    by_tally = {int(row["mcnp_tally_number"]): row for row in detector_rows}
    require(set(by_tally) == set(AIM_REQUIRED_TALLIES),
            "analytic AIM control must retain all four FNS-Fe-5 detector responses")
    require(patch_statepoint["lost_particles"] == 0
            and patch_statepoint["intentional_kill_events"] == 0,
            "vacuum AIM control observed particle loss or intentional importance kills")
    results = []
    for tally_id in AIM_REQUIRED_TALLIES:
        row = by_tally[tally_id]
        values = (row["raw_tracklength_mean_particle_cm_per_source"],
                  row["raw_tracklength_std_dev_particle_cm_per_source"],
                  row["sphere_volume_cm3"], row["analytic_flux_cm-2_per_source"])
        require(all(math.isfinite(float(value)) for value in values),
                f"vacuum F{tally_id} control has non-finite score or geometry")
        mean, sd, volume, expected = map(float, values)
        require(mean > 0.0 and sd >= 0.0 and volume > 0.0 and expected > 0.0,
                f"vacuum F{tally_id} control has a zero or negative flux score")
        estimate, u_mc = mean / volume, sd / volume
        relative_mcse = u_mc / estimate
        relative_bias = estimate / expected - 1.0
        z = (estimate - expected) / u_mc if u_mc else (0.0 if estimate == expected else None)
        passed = relative_mcse <= max_relative_mcse and z is not None and abs(z) <= max_abs_z
        results.append({**row, "estimated_flux_cm-2_per_source": estimate,
                        "u_mc_cm-2_per_source": u_mc,
                        "relative_mcse": relative_mcse, "relative_bias": relative_bias,
                        "analytic_z_score": z, "passed": passed})
    return {"status": "PASS" if all(row["passed"] for row in results) else "FAIL",
            "criteria": {"all_four_detector_tallies": list(AIM_REQUIRED_TALLIES),
                         "relative_mcse_max": max_relative_mcse,
                         "absolute_analytic_z_max": max_abs_z,
                         "lost_particles": 0, "intentional_importance_kills": 0},
            "patch_counters": patch_statepoint,
            "results": results,
            "scope": "vacuum source-mixture and cell-volume estimator control only; no F5 equivalence, transport validation or production finite-volume qualification"}


def result_levels(openmc, manifest: dict, tables: list[dict], statepoint: Path, radius: float) -> dict:
    results = {}
    volume = sphere_volume_cm3(radius)
    with openmc.StatePoint(str(statepoint)) as sp:
        tallies = {t.name: t for t in sp.tallies.values()}
        by_angle = {round(t["angle_deg"], 1): t for t in tables}
        for table in manifest["adapter"]["tables"]:
            angle_key = round(table["angle_deg"], 1)
            tally_name = f"f{table['tally_number']}_{'coarse' if table['coarse'] else 'fine'}"
            tally = tallies.get(tally_name)
            require(tally is not None, f"statepoint lacks expected tally {tally_name}")
            means = tally.mean.reshape(-1).tolist()
            stds = tally.std_dev.reshape(-1).tolist()
            rows = table["detector_ids"]
            require(len(means) == len(stds) == len(rows), f"{tally_name} shape differs from pinned reference bins")
            point = by_angle[angle_key]
            for ref, mean, sd in zip(rows, means, stds, strict=True):
                key = ref["detector_id"]
                results[key] = tally_value(mean, sd, point["mcnp_fm"], ref["lo_mev"], ref["hi_mev"],
                                           volume, radius)
    return results


def tally_identity_mapping(model) -> list[dict]:
    """Bind source MCNP F5 tally numbers to the actual OpenMC tally/filter IDs."""
    rows = []
    seen = set()
    for tally in model.tallies:
        match = re.fullmatch(r"f(\d+)_(fine|coarse)", tally.name or "")
        require(match is not None, f"unexpected generated OpenMC tally name {tally.name!r}")
        source_id, resolution = int(match.group(1)), match.group(2)
        key = (source_id, resolution)
        require(key not in seen, f"duplicate generated tally mapping for F{source_id} {resolution}")
        seen.add(key)
        rows.append({"mcnp_tally_number": source_id, "energy_resolution": resolution,
                     "openmc_tally_id": tally.id,
                     "filters": [{"openmc_filter_id": filt.id,
                                  "type": type(filt).__name__}
                                 for filt in tally.filters]})
    require(rows, "generated OpenMC model has no tallies")
    return sorted(rows, key=lambda row: (row["mcnp_tally_number"], row["energy_resolution"]))


def converter_identity() -> dict:
    from openmc_mcnp_adapter import openmc_conversion
    package_dir = Path(openmc_conversion.__file__).resolve().parent
    modules = {str(path.relative_to(package_dir)): hash_file(path)
               for path in sorted(package_dir.rglob("*.py")) if path.is_file()}
    canonical = json.dumps(modules, sort_keys=True, separators=(",", ":")).encode()
    source_sha256 = sha256_bytes(canonical)
    require(source_sha256 == CONVERTER_PYTHON_SOURCE_SHA256,
            "installed openmc_mcnp_adapter Python sources do not match the pinned commit")
    distribution_version = importlib.metadata.version("openmc_mcnp_adapter")
    require(distribution_version == CONVERTER_DISTRIBUTION_VERSION,
            "installed openmc_mcnp_adapter version differs from the pinned source")
    return {"source_url": CONVERTER_SOURCE_URL, "source_commit": CONVERTER_COMMIT,
            "package_dir": str(package_dir), "python_source_sha256": source_sha256,
            "python_module_hashes": modules, "distribution_version": distribution_version,
            "verification_status": "NOT_EVALUATED"}


def adapter_code_identity() -> dict:
    files = {path.name: hash_file(path) for path in (Path(__file__).resolve(), HERE / "benchmark.py")}
    canonical = json.dumps(files, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return {"sha256": sha256_bytes(canonical), "files": files}


def library_identity(xml: Path) -> dict:
    require(xml.stat().st_size <= MAX_CROSS_SECTIONS_XML_BYTES, "cross_sections.xml exceeds bounded metadata limit")
    root = ET.parse(xml).getroot()
    files = {"cross_sections.xml": hash_file(xml)}
    for entry in root.findall("library"):
        raw = entry.get("path")
        require(raw is not None, "cross_sections.xml has a library entry without a path")
        path = Path(raw).expanduser()
        if not path.is_absolute():
            path = xml.parent / path
        path = path.resolve(strict=True)
        files[str(path)] = hash_file(path)
    canonical = json.dumps(files, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return {"sha256": sha256_bytes(canonical), "files": files,
            "basis": "canonical SHA256 map of cross_sections.xml and every referenced nuclear-data file"}


def make_record(manifest: dict, run_id: str, identity: dict, results: dict, context: dict) -> dict:
    record = {"schema": "faris.validation-run/1.0.0", "kind": RUN_KIND, "case_id": manifest["case_id"],
              "run_id": run_id, "run_date": datetime.now(timezone.utc).date().isoformat(),
              "manifest_sha256": manifest_sha256(manifest), "identity": identity,
              "results": [{"detector_id": k, **v} for k, v in sorted(results.items())], "execution": context}
    return seal_run_record(record)


def fresh_dir(path: Path) -> None:
    require(not path.exists(), f"refusing to reuse existing work directory: {path}")
    path.mkdir(parents=True)


def solver_environment(base: dict[str, str], threads: int, aperture: dict | None = None) -> dict[str, str]:
    """Ensure the analog default cannot inherit AIM settings from the caller."""
    names = ("OPENMC_APERTURE_BIAS", "OPENMC_APERTURE_ONE_SIDED", "OPENMC_COLLISION_AIM",
             "OPENMC_WW_SKIP_VOID", "OPENMC_SPLIT_BEFORE_COLLISION")
    result = {key: value for key, value in base.items() if key not in names}
    result["OMP_NUM_THREADS"] = str(threads)
    result["OPENBLAS_NUM_THREADS"] = "1"
    if aperture is not None:
        result.update(aperture["environment"])
    return result


def command_aim_vacuum_control(args) -> int:
    require(args.i_have_the_cpu, "aim-vacuum-control runs three bounded OpenMC vacuum jobs; pass --i-have-the-cpu")
    require(args.case == AIM_SUPPORTED_CASE, "the AIM vacuum-control adapter is restricted to FNS-Fe-5")
    require(0 < args.particles <= MAX_PARTICLES, f"--particles must be in [1, {MAX_PARTICLES}]")
    require(0 < args.batches <= MAX_BATCHES and args.particles % args.batches == 0,
            "--batches must be <=500 and divide --particles exactly")
    require(0 < args.threads <= MAX_THREADS, f"--threads must be in [1, {MAX_THREADS}]")
    require(0 < args.timeout <= MAX_WALL_SECONDS, f"--timeout must be in [1, {MAX_WALL_SECONDS}] seconds per radius")
    manifest = load_manifest(args.manifest)
    require(manifest["case_id"] == AIM_SUPPORTED_CASE,
            "AIM vacuum-control requires the pinned four-point FNS-Fe-5 manifest")
    require(math.isfinite(args.aim_aperture_beta) and 0.0 < args.aim_aperture_beta < 1.0,
            "AIM beta must be strictly between zero and one to preserve physical isotropic support")
    upstream = args.upstream.expanduser().resolve(strict=True)
    manifest, deck_path, parsed_wrapper = source_inputs(upstream, manifest)
    parsed = parsed_wrapper["deck"]
    validate_response_mapping(manifest, parsed)
    require(tuple(point["tally"] for point in parsed["detectors"]) == AIM_REQUIRED_TALLIES,
            "pinned FNS-Fe-5 deck no longer has exactly all four supported detector points")
    cross_sections = args.cross_sections.expanduser().resolve(strict=True)
    library_identity(cross_sections)
    os.environ["OPENMC_CROSS_SECTIONS"] = str(cross_sections)
    try:
        import openmc
    except ImportError as exc:
        raise ValueError("OpenMC Python bindings are not installed") from exc
    require(openmc.__version__ == OPENMC_REQUIRED, f"expected OpenMC {OPENMC_REQUIRED}, got {openmc.__version__}")
    executable = shutil.which(args.openmc)
    require(executable is not None, f"OpenMC executable not found: {args.openmc}")
    executable_path = Path(executable).resolve(strict=True)
    base_env = {key: value for key, value in os.environ.items() if key not in (
        "OPENMC_APERTURE_BIAS", "OPENMC_APERTURE_ONE_SIDED", "OPENMC_COLLISION_AIM",
        "OPENMC_WW_SKIP_VOID", "OPENMC_SPLIT_BEFORE_COLLISION")}
    base_env.update({"OMP_NUM_THREADS": str(args.threads), "OPENBLAS_NUM_THREADS": "1"})
    runtime = selected_openmc_runtime_identity(executable_path, base_env)
    openmc_module_path = Path(openmc.__file__).resolve(strict=True)
    runtime.update({"openmc_python_version": openmc.__version__,
                    "python_executable_path": str(Path(sys.executable).resolve(strict=True)),
                    "openmc_python_module_path": str(openmc_module_path),
                    "openmc_python_module_sha256": hash_file(openmc_module_path),
                    "cross_sections_path": str(cross_sections),
                    "cross_sections_xml_sha256": hash_file(cross_sections, MAX_CROSS_SECTIONS_XML_BYTES)})
    imp0_control = verify_imp0_control_receipt(args.imp0_control.expanduser().resolve(strict=True), runtime)
    runtime["aim_capability"] = aim_runtime_identity(runtime)
    source_hash = hash_file(deck_path, MAX_SOURCE_BYTES)

    work = args.work_dir.expanduser().resolve()
    fresh_dir(work)
    radius_levels = []
    artifacts = []
    for index, radius in enumerate(AIM_CONTROL_RADII_CM):
        folder = work / f"aim-vacuum-radius-{radius:g}cm"
        folder.mkdir()
        require(hash_file(executable_path) == runtime["executable_sha256"],
                "selected OpenMC CLI changed after AIM control runtime preflight")
        require(hash_file(Path(runtime["linked_library_path"])) == runtime["linked_library_sha256"],
                "selected linked OpenMC library changed after AIM control runtime preflight")
        aperture = source_aperture_plan(parsed, radius, args.aim_aperture_beta)
        model, detector_cells = vacuum_aim_control_model(
            openmc, parsed, radius, args.batches, args.particles, args.seed + index)
        model.export_to_xml(directory=str(folder))
        (folder / "aim-source-plan.json").write_text(
            json.dumps(aperture, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
        env = solver_environment(base_env, args.threads, aperture)
        try:
            completed = subprocess.run([str(executable_path)], cwd=folder, env=env,
                                       capture_output=True, text=True, timeout=args.timeout)
        except subprocess.TimeoutExpired as exc:
            def as_text(value):
                return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else (value or "")
            (folder / "openmc.log").write_text(as_text(exc.stdout) + as_text(exc.stderr), encoding="utf-8")
            raise ValueError(f"AIM vacuum control exceeded {args.timeout} seconds at radius {radius:g} cm") from exc
        log_path = folder / "openmc.log"
        log_path.write_text(completed.stdout + completed.stderr, encoding="utf-8")
        require(completed.returncode == 0, f"OpenMC AIM vacuum control failed at radius {radius:g} cm; see {log_path}")
        statepoints = sorted(folder.glob("statepoint.*.h5"))
        require(len(statepoints) == 1, f"expected one AIM vacuum statepoint at radius {radius:g} cm")
        statepoint = statepoints[0]
        patch_diagnostics = read_imp0_statepoint(statepoint)
        require(patch_diagnostics["patch_version"] == runtime["patch_version"],
                "AIM vacuum child statepoint came from a different MCNP imp0 patch runtime")
        require(patch_diagnostics["source_birth_kill_events"] == 0
                and patch_diagnostics["cell_entry_kill_events"] == 0,
                "vacuum AIM control unexpectedly invoked MCNP importance killing")
        detector_rows = []
        with openmc.StatePoint(str(statepoint)) as sp:
            for point in parsed["detectors"]:
                tally_id = int(point["tally"])
                tally = sp.get_tally(name=f"aim_vacuum_f{tally_id}")
                mean = float(tally.mean.ravel()[0])
                std = float(tally.std_dev.ravel()[0])
                cell_info = detector_cells[tally_id]
                volume = sphere_volume_cm3(radius)
                detector_rows.append({"mcnp_tally_number": tally_id,
                                      "angle_deg": point["angle_deg"],
                                      "position_cm": point["position_cm"],
                                      "radius_cm": radius, "sphere_volume_cm3": volume,
                                      "raw_tracklength_mean_particle_cm_per_source": mean,
                                      "raw_tracklength_std_dev_particle_cm_per_source": std,
                                      "analytic_flux_cm-2_per_source": cell_info["analytic_flux_cm-2_per_source"]})
        evaluated = evaluate_vacuum_aim_control(detector_rows, patch_diagnostics)
        level = {"radius_cm": radius, "status": evaluated["status"],
                 "criteria": evaluated["criteria"], "results": evaluated["results"],
                 "patch_statepoint": patch_diagnostics,
                 "source_aperture": aperture}
        radius_levels.append(level)
        artifacts.append({"radius_cm": radius,
                          "model_xml_sha256": hash_file(folder / "model.xml") if (folder / "model.xml").is_file() else None,
                          "geometry_xml_sha256": hash_file(folder / "geometry.xml") if (folder / "geometry.xml").is_file() else None,
                          "materials_xml_sha256": hash_file(folder / "materials.xml") if (folder / "materials.xml").is_file() else None,
                          "settings_xml_sha256": hash_file(folder / "settings.xml"),
                          "tallies_xml_sha256": hash_file(folder / "tallies.xml"),
                          "statepoint_path": str(statepoint), "statepoint_sha256": hash_file(statepoint),
                          "log_path": str(log_path), "log_sha256": hash_file(log_path),
                          "aim_plan_sha256": hash_file(folder / "aim-source-plan.json"),
                          "patch_statepoint": patch_diagnostics})
        (folder / "aim-vacuum-results.json").write_text(
            json.dumps(level, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")

    passed = all(level["status"] == "PASS" for level in radius_levels)
    document = {"schema": "faris.fns-aim-vacuum-control/1.0.0",
                "status": "PASS" if passed else "FAIL", "case_id": manifest["case_id"],
                "manifest_sha256": manifest_sha256(manifest),
                "source_deck_path": str(deck_path), "source_deck_sha256": source_hash,
                "cross_sections_path": str(cross_sections),
                "cross_sections_xml_sha256": runtime["cross_sections_xml_sha256"],
                "runtime_identity": runtime, "imp0_control_receipt": imp0_control,
                "beta": args.aim_aperture_beta, "radii_cm": list(AIM_CONTROL_RADII_CM),
                "histories_per_radius": args.particles, "batches": args.batches,
                "threads": args.threads, "seed_by_radius": [args.seed + i for i in range(3)],
                "radius_levels": radius_levels, "solver_artifacts": artifacts,
                "acceptance": {"all_four_detector_points_at_all_three_radii": True,
                               "analytic_cell_flux_bias": "absolute z-score <= 3 and relative MCSE <= 2% for each detector/radius",
                               "zero_lost_particles_and_zero_importance_kills": True,
                               "source_likelihood_ratio_bound": "0 < p_iso/q <= 1/(1-beta); per-plan finite positive lower bound also recorded",
                               "source_direction_law": "isotropic law retained through a positive (1-beta) mixture component"},
                "scope": "weighted isotropic point-source vacuum control for aperture sampling and finite-cell volume normalization; it does not test material transport, finite-volume convergence in FNS, or MCNP F5 equivalence"}
    document["receipt_sha256"] = canonical_json_sha256(document)
    receipt_path = work / "fns-aim-vacuum-control.json"
    receipt_path.write_text(json.dumps(document, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    if not passed:
        raise ValueError(f"AIM vacuum analytic control failed; inspect {receipt_path}")
    print(json.dumps({"status": document["status"], "receipt": str(receipt_path),
                      "receipt_sha256": document["receipt_sha256"],
                      "all_four_detectors": list(AIM_REQUIRED_TALLIES),
                      "radii_cm": list(AIM_CONTROL_RADII_CM)}, indent=2))
    return 0


def command_run(args) -> int:
    require(args.i_have_the_cpu, "run starts three bounded OpenMC jobs; pass --i-have-the-cpu")
    require(0 < args.particles <= MAX_PARTICLES, f"--particles must be in [1, {MAX_PARTICLES}]")
    require(0 < args.batches <= MAX_BATCHES and args.particles % args.batches == 0, "--batches must be <=500 and divide --particles exactly")
    require(0 < args.threads <= MAX_THREADS, f"--threads must be in [1, {MAX_THREADS}]")
    require(0 < args.timeout <= MAX_WALL_SECONDS, f"--timeout must be in [1, {MAX_WALL_SECONDS}] seconds per radius")
    manifest = load_manifest(args.manifest)
    upstream = args.upstream.expanduser().resolve(strict=True)
    manifest, deck, parsed_wrapper = source_inputs(upstream, manifest)
    parsed = parsed_wrapper["deck"]
    response_map = validate_response_mapping(manifest, parsed)
    require((args.aim_aperture_beta is None) == (args.aim_vacuum_control is None),
            "AIM production requires both --aim-aperture-beta and a passing --aim-vacuum-control receipt")
    if args.aim_aperture_beta is not None:
        require(manifest["case_id"] == AIM_SUPPORTED_CASE,
                "source aperture AIM is currently qualified only for the four-point FNS-Fe-5 case")
        require(math.isfinite(args.aim_aperture_beta) and 0.0 < args.aim_aperture_beta < 1.0,
                "AIM beta must be strictly between zero and one to preserve physical isotropic support")
        source_aperture_plan(parsed, AIM_CONTROL_RADII_CM[0], args.aim_aperture_beta)
    require(args.work_dir.expanduser().resolve() != args.record.expanduser().resolve() if args.record else True,
            "record path must differ from the work directory")
    cross_sections = args.cross_sections.expanduser().resolve(strict=True)
    library = library_identity(cross_sections)
    os.environ["OPENMC_CROSS_SECTIONS"] = str(cross_sections)
    try:
        import openmc
    except ImportError as exc:
        raise ValueError("OpenMC Python bindings are not installed") from exc
    require(openmc.__version__ == OPENMC_REQUIRED, f"expected OpenMC {OPENMC_REQUIRED}, got {openmc.__version__}")
    executable = shutil.which(args.openmc)
    require(executable is not None, f"OpenMC executable not found: {args.openmc}")
    executable_path = Path(executable).resolve(strict=True)
    openmc_exe_sha256 = hash_file(executable_path)
    try:
        conv = converter_identity()
    except (ImportError, importlib.metadata.PackageNotFoundError) as exc:
        raise ValueError("openmc_mcnp_adapter must be installed from a pinned source checkout") from exc

    solver_env = solver_environment(os.environ, args.threads)
    runtime = selected_openmc_runtime_identity(executable_path, solver_env)
    require(runtime["executable_sha256"] == openmc_exe_sha256,
            "OpenMC CLI changed between discovery and linked-library capability check")
    openmc_module_path = Path(openmc.__file__).resolve(strict=True)
    runtime["openmc_python_module_path"] = str(openmc_module_path)
    runtime["openmc_python_module_sha256"] = hash_file(openmc_module_path)
    runtime["cross_sections_path"] = str(cross_sections)
    runtime["cross_sections_xml_sha256"] = hash_file(cross_sections, MAX_CROSS_SECTIONS_XML_BYTES)
    control_receipt = verify_imp0_control_receipt(args.imp0_control.expanduser().resolve(strict=True), runtime)
    aim_control_receipt = None
    if args.aim_aperture_beta is not None:
        runtime["aim_capability"] = aim_runtime_identity(runtime)
        aim_control_receipt = verify_aim_control_receipt(
            args.aim_vacuum_control.expanduser().resolve(strict=True), runtime,
            manifest["case_id"], parsed, hash_file(deck, MAX_SOURCE_BYTES), args.aim_aperture_beta,
            control_receipt)

    work = args.work_dir.expanduser().resolve()
    fresh_dir(work)
    levels = []
    artifacts = []
    for index, radius in enumerate((2.5, 1.25, 0.625)):
        folder = work / f"radius-{radius:g}cm"
        folder.mkdir()
        require(hash_file(executable_path) == runtime["executable_sha256"],
                "selected OpenMC CLI changed after the imp0 control preflight")
        require(hash_file(Path(runtime["linked_library_path"])) == runtime["linked_library_sha256"],
                "selected linked libopenmc changed after the imp0 control preflight")
        model, finite_cells = model_for_radius(openmc, deck, upstream, manifest, parsed, radius,
                                               args.batches, args.particles, args.seed + index)
        aim_aperture = (source_aperture_plan(parsed, radius, args.aim_aperture_beta)
                        if args.aim_aperture_beta is not None else None)
        importance_mapping = verified_importance_mapping(model, deck, parsed)
        model.export_to_xml(directory=str(folder))
        importance_xml = inject_importance_xml_markers(
            folder / "geometry.xml", folder / "mcnp-importance-map.json", manifest["case_id"],
            hash_file(deck, MAX_SOURCE_BYTES), conv, importance_mapping)
        cmd = [str(executable_path)]
        try:
            radius_env = solver_environment(os.environ, args.threads, aim_aperture)
            completed = subprocess.run(cmd, cwd=folder, env=radius_env,
                                       capture_output=True, text=True, timeout=args.timeout)
        except subprocess.TimeoutExpired as exc:
            def as_text(value):
                return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else (value or "")
            (folder / "openmc.log").write_text(as_text(exc.stdout) + as_text(exc.stderr), encoding="utf-8")
            raise ValueError(f"OpenMC exceeded the {args.timeout}-second per-radius timeout; partial output is in {folder / 'openmc.log'}") from exc
        (folder / "openmc.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
        require(completed.returncode == 0, f"OpenMC failed for radius {radius:g} cm; see {folder / 'openmc.log'}")
        statepoints = sorted(folder.glob("statepoint.*.h5"))
        require(len(statepoints) == 1, f"expected one statepoint for radius {radius:g} cm")
        patch_statepoint = read_imp0_statepoint(statepoints[0])
        require(patch_statepoint["patch_version"] == runtime["patch_version"],
                "child statepoint MCNP imp0 version differs from the selected CLI library")
        bins = result_levels(openmc, manifest, parsed["detectors"], statepoints[0], radius)
        levels.append({"radius_cm": radius, "bins": bins})
        artifacts.append({"radius_cm": radius,
                          "geometry_xml": {"path": str(folder / "geometry.xml"),
                                           "sha256": importance_xml["final_geometry_sha256"],
                                           "pre_injection_sha256": importance_xml["pre_injection_geometry_sha256"]},
                          "importance_map": importance_xml,
                          "materials_xml": {"path": str(folder / "materials.xml"), "sha256": hash_file(folder / "materials.xml")},
                          "settings_xml": {"path": str(folder / "settings.xml"), "sha256": hash_file(folder / "settings.xml")},
                          "tallies_xml": {"path": str(folder / "tallies.xml"), "sha256": hash_file(folder / "tallies.xml")},
                          "statepoint": {"path": str(statepoints[0]), "sha256": hash_file(statepoints[0])},
                          "openmc_log": {"path": str(folder / "openmc.log"), "sha256": hash_file(folder / "openmc.log")},
                          "imp0_child_statepoint_diagnostics": patch_statepoint,
                          "source_aperture_bias": aim_aperture,
                          "imp0_mapping": importance_mapping,
                          "openmc_tally_mapping": tally_identity_mapping(model)})
        artifacts[-1]["finite_detector_cells"] = {
            str(tally): {"openmc_detector_cell_id": info["cell"].id,
                         "openmc_host_cell_id": info["host_cell_id"],
                         **{k: v for k, v in info.items() if k not in {"cell", "host_cell_id"}}}
            for tally, info in finite_cells.items()
        }
        (folder / "radius-results.json").write_text(json.dumps(levels[-1], indent=2, sort_keys=True, allow_nan=False) + "\n")

    qualification = qualify_halvings(levels)
    (work / "finite-detector-qualification.json").write_text(json.dumps(qualification, indent=2, sort_keys=True) + "\n")
    require(qualification["qualified"], "finite detector failed frozen two-halving gate; no FARIS validation record was sealed; inspect finite-detector-qualification.json")
    results = levels[-1]["bins"]
    require(set(results) == {d["id"] for d in manifest["detectors"]}, "run output does not include every declared response row")
    adapter_identity = adapter_code_identity()
    record = make_record(manifest, args.run_id or f"openmc-{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')}",
                         {"library_sha256": library["sha256"], "code_version": f"OpenMC-{openmc.__version__}/FARIS-imp0-{runtime['patch_version']}/Python-{platform.python_version()}",
                          "adapter_sha256": adapter_identity["sha256"]}, results,
                         {"adapter": "FNS-TOF spherical finite-volume approximation",
                          "adapter_code": adapter_identity, "converter": conv, "cross_sections": library,
                          "openmc_executable": {"path": str(executable_path), "sha256": openmc_exe_sha256},
                          "patched_openmc_runtime": runtime, "imp0_control_receipt": control_receipt,
                          "aim_vacuum_control_receipt": aim_control_receipt,
                          "python_version": platform.python_version(),
                          "source_deck_sha256": hash_file(deck), "source_histogram": parsed["source_histogram"],
                          "detector_mapping": parsed["detectors"], "particles_per_radius": args.particles,
                          "response_mapping_sha256": sha256_bytes(json.dumps(response_map, sort_keys=True, separators=(",", ":")).encode()),
                          "batches": args.batches, "threads": args.threads, "seed_by_radius": [args.seed + i for i in range(3)],
                          "radii_cm": [v["radius_cm"] for v in levels], "radius_levels": levels,
                          "tally_normalization": {
                              "raw_openmc_flux_units": "particle-cm per source particle",
                              "spatial_normalization": "divide cell-filter tracklength mean and standard deviation by exact analytic sphere volume 4*pi*r^3/3",
                              "energy_normalization": "divide by ln(E_hi/E_lo) for per-unit-lethargy responses",
                              "mcnp_FM": "apply the exact source-deck multiplier once after spatial normalization",
                              "normalized_flux_units": "particle/cm^2 per source particle per unit lethargy"},
                          "solver_artifacts": artifacts,
                          "finite_point_approximation_qualification": qualification,
                          "variance_reduction": {"enabled": aim_aperture is not None,
                              "method": "exact-weight isotropic/rectangular source-angle mixture" if aim_aperture else "analog isotropic source",
                              "source_weight_receipt": "per-radius proposal, finite support and likelihood-ratio bounds are recorded" if aim_aperture else "analog source weight 1",
                              "collision_aim": "disabled; no qualified FNS weight windows exist"},
                          "scope": "finite-volume convergence only; does not establish exact F5 equivalence or physical validation",
                          "reference_uncertainty": "source-relative errors retained; confidence interpretation and cross-bin covariance not provided"})
    target = args.record.expanduser().resolve() if args.record else work / f"{manifest['case_id']}.run.json"
    require(not target.exists(), f"record already exists; refusing overwrite: {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(target.suffix + ".tmp")
    tmp.write_text(json.dumps(record, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    tmp.replace(target)
    print(json.dumps({"case_id": manifest["case_id"], "record": str(target),
                      "record_sha256": record["record_sha256"], "library_sha256": library["sha256"],
                      "finite_detector_qualified": True,
                      "verdict_policy": "this finite-volume gate is not physical validation; experimental covariance remains unknown"}, indent=2))
    return 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = root.add_subparsers(dest="command", required=True)
    plan = sub.add_parser("plan", help="verify inputs and show the bounded run plan; no OpenMC import or solver")
    plan.add_argument("--upstream", type=Path, required=True)
    plan.add_argument("--manifest", type=Path, required=True)
    plan.add_argument("--cross-sections", type=Path, required=True)
    plan.add_argument("--particles", type=int, default=100_000)
    plan.add_argument("--batches", type=int, default=100)
    plan.add_argument("--threads", type=int, default=1)
    run = sub.add_parser("run", help="run the three-radius transport qualification and seal only if it passes")
    run.add_argument("--upstream", type=Path, required=True)
    run.add_argument("--manifest", type=Path, required=True)
    run.add_argument("--cross-sections", type=Path, required=True)
    run.add_argument("--work-dir", type=Path, required=True)
    run.add_argument("--record", type=Path)
    run.add_argument("--openmc", default="openmc", help="OpenMC executable on PATH")
    run.add_argument("--imp0-control", type=Path, required=True,
                     help="passed four-case entry/source-birth/photon/unmarked patch control receipt for this exact CLI and linked library")
    run.add_argument("--aim-aperture-beta", type=float,
                     help="opt into exact-weight isotropic/rectangular source-angle mixture; requires a passing --aim-vacuum-control")
    run.add_argument("--aim-vacuum-control", type=Path,
                     help="PASS receipt from the matching four-detector, three-radius analytic vacuum AIM control")
    run.add_argument("--particles", type=int, default=100_000)
    run.add_argument("--batches", type=int, default=100)
    run.add_argument("--threads", type=int, default=1)
    run.add_argument("--seed", type=int, default=1)
    run.add_argument("--timeout", type=int, default=3600, help=f"timeout per radius, maximum {MAX_WALL_SECONDS} seconds")
    run.add_argument("--run-id")
    run.add_argument("--i-have-the-cpu", action="store_true")
    aim = sub.add_parser("aim-vacuum-control", help="run analytic all-void source-weight and finite-volume control before AIM production")
    aim.add_argument("--upstream", type=Path, required=True)
    aim.add_argument("--manifest", type=Path, required=True)
    aim.add_argument("--cross-sections", type=Path, required=True)
    aim.add_argument("--work-dir", type=Path, required=True)
    aim.add_argument("--openmc", default="openmc", help="AIM-patched OpenMC executable on PATH")
    aim.add_argument("--imp0-control", type=Path, required=True,
                     help="passing four-case imp0 control receipt for this exact CLI/library/Python/data identity")
    aim.add_argument("--aim-aperture-beta", type=float, required=True,
                     help="positive source-aperture mixture fraction; must be <1 to preserve isotropic support")
    aim.add_argument("--particles", type=int, default=1_000_000)
    aim.add_argument("--batches", type=int, default=100)
    aim.add_argument("--threads", type=int, default=1)
    aim.add_argument("--seed", type=int, default=1701)
    aim.add_argument("--timeout", type=int, default=3600, help=f"timeout per radius, maximum {MAX_WALL_SECONDS} seconds")
    aim.add_argument("--i-have-the-cpu", action="store_true")
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "plan":
            manifest = load_manifest(args.manifest)
            doc = plan_document(manifest, args.upstream.expanduser().resolve(), args.cross_sections.expanduser(), args.particles, args.batches, args.threads)
            print(json.dumps(doc, indent=2, sort_keys=True))
            return 0
        if args.command == "aim-vacuum-control":
            return command_aim_vacuum_control(args)
        return command_run(args)
    except (ValueError, OSError, subprocess.SubprocessError, IntakeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
