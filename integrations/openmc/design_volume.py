#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Measure DAGMC cell volumes with OpenMC's stochastic volume calculation (GEO-030).

This is a geometry-only initialization. Each DAGMC material name is given an inert H1 dummy
material so OpenMC can initialize the geometry. The helper does not transport particles and its
dummy material assignment is not material or transport validation.

The request includes the expected H5M digest, explicit DAGMC cell IDs from the checked facets
manifest, unique volume names, and the FARIS bounding box in SI metres. Results are written to a
JSON report after OpenMC has completed its normal `volume_1.h5` result.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import subprocess
import sys
import time
from pathlib import Path

SCHEMA = "faris-design-volume-check/v1"
REQUEST_SCHEMA = "faris-design-volume-request/v1"
DUMMY_DENSITY_G_CM3 = 1.0e-20
MAX_OPENMC_ID = (1 << 31) - 1


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def validate_request(request: dict) -> None:
    if request.get("schema") != REQUEST_SCHEMA:
        raise ValueError(f"request schema must be {REQUEST_SCHEMA}")
    if not isinstance(request.get("seed"), int) or request["seed"] < 1:
        raise ValueError("seed must be a positive integer")
    if not isinstance(request.get("samples"), int) or request["samples"] < 1000:
        raise ValueError("samples must be an integer of at least 1000")
    for name in ("h5m_sha256", "cross_sections_sha256"):
        value = request.get(name)
        if (not isinstance(value, str) or len(value) != 64
                or any(character not in "0123456789abcdefABCDEF" for character in value)):
            raise ValueError(f"{name} must be a 64-character hexadecimal sha256")
    if not request.get("targets"):
        raise ValueError("the request has no DAGMC volume targets")
    ids = [item.get("global_id") for item in request["targets"]]
    indices = [item.get("volume") for item in request["targets"]]
    if any(not isinstance(value, int) or not 0 < value <= MAX_OPENMC_ID for value in ids):
        raise ValueError("every DAGMC global cell ID must be a positive OpenMC integer ID")
    if len(set(ids)) != len(ids) or len(set(indices)) != len(indices):
        raise ValueError("DAGMC cell IDs and FARIS volume indices must each be unique")
    surfaces = request.get("surface_ids")
    if not isinstance(surfaces, list) or any(not isinstance(value, int) or not 0 < value <= MAX_OPENMC_ID for value in surfaces):
        raise ValueError("surface_ids must list positive OpenMC integer IDs")
    if len(set(surfaces)) != len(surfaces):
        raise ValueError("DAGMC surface IDs must be unique")
    for target in request["targets"]:
        for key in ("solid_id", "material_tag"):
            if not isinstance(target.get(key), str) or not target[key]:
                raise ValueError(f"each target needs a non-empty {key}")
        for key in ("cad_volume_m3", "faceted_volume_m3"):
            value = target.get(key)
            if not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
                raise ValueError(f"each target needs a finite positive {key}")
    bounds = request.get("bounds_m")
    if (not isinstance(bounds, dict) or len(bounds.get("lower_left", [])) != 3
            or len(bounds.get("upper_right", [])) != 3):
        raise ValueError("bounds_m must contain three-dimensional lower_left and upper_right")
    lo, hi = bounds["lower_left"], bounds["upper_right"]
    if any(not math.isfinite(float(a)) or not math.isfinite(float(b)) or b <= a for a, b in zip(lo, hi)):
        raise ValueError("each bounding-box upper coordinate must exceed its finite lower coordinate")


def padded_bounds_cm(bounds_m: dict) -> tuple[list[float], list[float]]:
    lo = [float(value) * 100.0 for value in bounds_m["lower_left"]]
    hi = [float(value) * 100.0 for value in bounds_m["upper_right"]]
    pad = max(1.0, 0.05 * max(b - a for a, b in zip(lo, hi)))
    return ([a - pad for a in lo], [b + pad for b in hi])


def dagmc_material_name(material_tag: str) -> str:
    """Map a FARIS H5M NAME tag to OpenMC's DAGMC material name."""
    if not material_tag.startswith("mat:") or material_tag == "mat:":
        raise ValueError(f"invalid FARIS DAGMC material tag: {material_tag!r}")
    return material_tag[4:]


def resolve_dummy_material_names(target_tags, dagmc_material_names) -> list[str]:
    """Validate target tags against OpenMC's view and initialize every H5M material."""
    expected = {dagmc_material_name(tag) for tag in target_tags}
    actual = sorted(set(dagmc_material_names))
    if not expected.issubset(actual):
        missing = sorted(expected - set(actual))
        raise ValueError(f"FARIS material tags are absent from OpenMC DAGMC material names: {missing}")
    if not actual or any(not isinstance(name, str) or not name for name in actual):
        raise ValueError("OpenMC DAGMC material names must be non-empty strings")
    return actual


def bounding_sphere_radius(bounds_lo: list[float], bounds_hi: list[float]) -> float:
    """Return a padded origin-centered sphere containing every bounding-box corner."""
    max_abs = [max(abs(lo), abs(hi)) for lo, hi in zip(bounds_lo, bounds_hi)]
    return 1.05 * math.sqrt(sum(value * value for value in max_abs))


def selected_cli_version(openmc_exec: Path) -> str:
    completed = subprocess.run(
        [str(openmc_exec), "--version"], capture_output=True, text=True,
        timeout=10, check=True,
    )
    match = re.search(r"^OpenMC version ([^\r\n]+)$", completed.stdout, re.MULTILINE)
    if not match:
        raise RuntimeError("selected OpenMC CLI did not report a recognizable version")
    return match.group(1).strip()


def calculate(request: dict, h5m: Path, cross_sections: Path, openmc_exec: Path, out_dir: Path,
              request_sha256: str) -> dict:
    import openmc

    started = time.monotonic()
    source_hash = sha256_file(h5m)
    if source_hash != request["h5m_sha256"]:
        raise ValueError("the H5M sha256 does not match the request")
    cross_sections_hash = sha256_file(cross_sections)
    if cross_sections_hash != request["cross_sections_sha256"]:
        raise ValueError("cross_sections.xml sha256 does not match the request")
    openmc_cli_hash = sha256_file(openmc_exec)
    python_executable_hash = sha256_file(Path(sys.executable))
    cli_version = selected_cli_version(openmc_exec)
    if cli_version != openmc.__version__:
        raise RuntimeError(
            f"selected OpenMC CLI version {cli_version} does not match Python API {openmc.__version__}"
        )

    dagmc = openmc.DAGMCUniverse(str(h5m.resolve()))
    material_names = resolve_dummy_material_names(
        (target["material_tag"] for target in request["targets"]), dagmc.material_names,
    )
    dummy_materials = []
    for number, name in enumerate(material_names, start=1):
        material = openmc.Material(material_id=number, name=name)
        material.add_nuclide("H1", 1.0)
        material.set_density("g/cm3", DUMMY_DENSITY_G_CM3)
        dummy_materials.append(material)

    cell_ids = [int(target["global_id"]) for target in request["targets"]]
    surface_ids = [int(value) for value in request["surface_ids"]]
    if any(value <= 0 for value in surface_ids):
        raise ValueError("every DAGMC surface global ID must be positive")
    outer_cell_id = max(10_000, max(cell_ids) + 1)
    outer_surface_id = max(100_000, max(surface_ids, default=0) + 1)
    if outer_cell_id in cell_ids or outer_surface_id in surface_ids:
        raise ValueError("the bounding cell or surface ID collides with a DAGMC ID")
    if max(outer_cell_id, outer_surface_id) > (1 << 31) - 1:
        raise ValueError("DAGMC global IDs leave no safe OpenMC ID for the bounding cell and surface")

    bbox_lo, bbox_hi = padded_bounds_cm(request["bounds_m"])
    radius = bounding_sphere_radius(bbox_lo, bbox_hi)
    sphere = openmc.Sphere(surface_id=outer_surface_id, r=radius, boundary_type="vacuum")
    outer = openmc.Cell(cell_id=outer_cell_id, fill=dagmc, region=-sphere)
    materials = openmc.Materials(dummy_materials)
    materials.cross_sections = str(cross_sections.resolve())
    model = openmc.Model(geometry=openmc.Geometry([outer]), materials=materials)
    model.settings.seed = request["seed"]
    calculation = openmc.VolumeCalculation(
        [openmc.Cell(cell_id=cell_id) for cell_id in cell_ids],
        samples=request["samples"], lower_left=bbox_lo, upper_right=bbox_hi,
    )
    model.settings.volume_calculations = [calculation]
    model.calculate_volumes(threads=1, openmc_exec=str(openmc_exec.resolve()), cwd=out_dir, output=True)
    volume_file = out_dir / "volume_1.h5"
    if not volume_file.is_file():
        raise RuntimeError("OpenMC completed without writing volume_1.h5")
    measured = openmc.VolumeCalculation.from_hdf5(volume_file)
    if measured.samples != request["samples"]:
        raise RuntimeError(f"OpenMC recorded {measured.samples} samples, expected {request['samples']}")
    if set(measured.volumes) != set(cell_ids):
        raise RuntimeError("OpenMC volume result cell IDs do not match the requested DAGMC IDs")

    rows = []
    for target in request["targets"]:
        estimate = measured.volumes[int(target["global_id"])]
        rows.append({
            "volume": int(target["volume"]),
            "global_id": int(target["global_id"]),
            "solid_id": target["solid_id"],
            "material_tag": target["material_tag"],
            "cad_volume_cm3": float(target["cad_volume_m3"]) * 1.0e6,
            "faceted_volume_cm3": float(target["faceted_volume_m3"]) * 1.0e6,
            "openmc_volume_cm3": float(estimate.nominal_value),
            "standard_error_cm3": float(estimate.std_dev),
            "samples": int(measured.samples),
        })
    if sha256_file(h5m) != source_hash:
        raise RuntimeError("the source H5M changed during the geometry-only calculation")
    if sha256_file(cross_sections) != cross_sections_hash:
        raise RuntimeError("cross_sections.xml changed during the geometry-only calculation")
    if sha256_file(openmc_exec) != openmc_cli_hash:
        raise RuntimeError("the OpenMC CLI changed during the geometry-only calculation")
    if sha256_file(Path(sys.executable)) != python_executable_hash:
        raise RuntimeError("the OpenMC Python executable changed during the geometry-only calculation")
    return {
        "schema": SCHEMA,
        "scope": "DAGMC geometry-only stochastic cell volumes; inert H1 dummy materials initialize geometry; no material or transport physics validation",
        "source_h5m_sha256": source_hash,
        "cross_sections_sha256": cross_sections_hash,
        "request_sha256": request_sha256,
        "volume_result_sha256": sha256_file(volume_file),
        "samples": int(measured.samples),
        "seed": int(request["seed"]),
        "unit": "cm3",
        "bounds_cm": {"lower_left": bbox_lo, "upper_right": bbox_hi},
        "dummy_material_names": material_names,
        "versions": {"openmc_python_package_version": openmc.__version__,
                     "openmc_selected_cli_version": cli_version,
                     "python_runtime_version": sys.version.split()[0],
                     "python_executable": str(Path(sys.executable).resolve()),
                     "python_executable_sha256": python_executable_hash,
                     "openmc_cli": str(openmc_exec.resolve()),
                     "openmc_cli_sha256": openmc_cli_hash},
        "seconds": time.monotonic() - started,
        "rows": rows,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--h5m", type=Path, required=True)
    parser.add_argument("--cross-sections", type=Path, required=True)
    parser.add_argument("--openmc-exec", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args(argv)
    request_bytes = args.request.read_bytes()
    request = json.loads(request_bytes)
    validate_request(request)
    result = calculate(
        request, args.h5m, args.cross_sections, args.openmc_exec, args.out_dir,
        hashlib.sha256(request_bytes).hexdigest(),
    )
    args.report.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "complete", "volumes": len(result["rows"]), "samples": result["samples"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
