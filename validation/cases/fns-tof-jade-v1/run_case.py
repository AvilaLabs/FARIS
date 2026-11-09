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


def require(ok: bool, why: str) -> None:
    if not ok:
        raise ValueError(why)


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
    xml_bytes = cross_sections.read_bytes() if cross_sections.is_file() else b""
    require(len(xml_bytes) <= MAX_CROSS_SECTIONS_XML_BYTES, "cross_sections.xml exceeds bounded metadata limit")
    return {
        "case_id": manifest["case_id"], "source_commit": SOURCE_COMMIT,
        "manifest_sha256": manifest_sha256(manifest), "input_deck": str(deck_path),
        "input_sha256": hash_file(deck_path, MAX_SOURCE_BYTES),
        "detectors": len(parsed["deck"]["detectors"]), "responses": detector_count,
        "tables": len(manifest["adapter"]["tables"]), "missing_angles_deg": manifest["adapter"]["missing_table_angles_deg"],
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


def require_vacuum_outer_sphere(model, parsed: dict) -> int:
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
    require(surface.boundary_type == "vacuum",
            f"converted outer MCNP surface {expected[0]} boundary is {surface.boundary_type!r}, expected vacuum")
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
                                  "sampled_host_containment_directions": len(dirs)}
    for host_id, spheres in spheres_by_host.items():
        host = cells[host_id]
        for sphere in spheres:
            host.region = host.region & +sphere
    model.geometry.determine_paths()
    model.geometry.get_all_cells()
    return result


def tally_value(mean: float, std: float, factor: float, lo_mev: float, hi_mev: float) -> dict:
    width = lethargy_width(lo_mev, hi_mev)
    values = (float(mean), float(std), float(factor))
    require(all(math.isfinite(v) for v in values), "tally mean, standard deviation and FM must be finite")
    require(values[0] >= 0 and values[1] >= 0 and values[2] > 0,
            "flux mean and standard deviation must be nonnegative and FM positive")
    value = values[0] * values[2] / width
    u_mc = values[1] * values[2] / width
    require(math.isfinite(value) and math.isfinite(u_mc), "normalized tally result is non-finite")
    return {"value": value, "u_mc": u_mc}


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
    require_vacuum_outer_sphere(model, parsed)
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


def result_levels(openmc, manifest: dict, tables: list[dict], statepoint: Path, radius: float) -> dict:
    results = {}
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
                results[key] = tally_value(mean, sd, point["mcnp_fm"], ref["lo_mev"], ref["hi_mev"])
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

    work = args.work_dir.expanduser().resolve()
    fresh_dir(work)
    levels = []
    artifacts = []
    for index, radius in enumerate((2.5, 1.25, 0.625)):
        folder = work / f"radius-{radius:g}cm"
        folder.mkdir()
        model, finite_cells = model_for_radius(openmc, deck, upstream, manifest, parsed, radius,
                                               args.batches, args.particles, args.seed + index)
        model.export_to_model_xml(str(folder / "model.xml"))
        cmd = [str(executable_path)]
        env = {**os.environ, "OMP_NUM_THREADS": str(args.threads), "OPENBLAS_NUM_THREADS": "1"}
        try:
            completed = subprocess.run(cmd, cwd=folder, env=env, capture_output=True, text=True, timeout=args.timeout)
        except subprocess.TimeoutExpired as exc:
            def as_text(value):
                return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else (value or "")
            (folder / "openmc.log").write_text(as_text(exc.stdout) + as_text(exc.stderr), encoding="utf-8")
            raise ValueError(f"OpenMC exceeded the {args.timeout}-second per-radius timeout; partial output is in {folder / 'openmc.log'}") from exc
        (folder / "openmc.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
        require(completed.returncode == 0, f"OpenMC failed for radius {radius:g} cm; see {folder / 'openmc.log'}")
        statepoints = sorted(folder.glob("statepoint.*.h5"))
        require(len(statepoints) == 1, f"expected one statepoint for radius {radius:g} cm")
        bins = result_levels(openmc, manifest, parsed["detectors"], statepoints[0], radius)
        levels.append({"radius_cm": radius, "bins": bins})
        artifacts.append({"radius_cm": radius,
                          "model_xml": {"path": str(folder / "model.xml"), "sha256": hash_file(folder / "model.xml")},
                          "statepoint": {"path": str(statepoints[0]), "sha256": hash_file(statepoints[0])},
                          "openmc_log": {"path": str(folder / "openmc.log"), "sha256": hash_file(folder / "openmc.log")},
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
                         {"library_sha256": library["sha256"], "code_version": f"OpenMC-{openmc.__version__}/Python-{platform.python_version()}",
                          "adapter_sha256": adapter_identity["sha256"]}, results,
                         {"adapter": "FNS-TOF spherical finite-volume approximation",
                          "adapter_code": adapter_identity, "converter": conv, "cross_sections": library,
                          "openmc_executable": {"path": str(executable_path), "sha256": openmc_exe_sha256},
                          "python_version": platform.python_version(),
                          "source_deck_sha256": hash_file(deck), "source_histogram": parsed["source_histogram"],
                          "detector_mapping": parsed["detectors"], "particles_per_radius": args.particles,
                          "response_mapping_sha256": sha256_bytes(json.dumps(response_map, sort_keys=True, separators=(",", ":")).encode()),
                          "batches": args.batches, "threads": args.threads, "seed_by_radius": [args.seed + i for i in range(3)],
                          "radii_cm": [v["radius_cm"] for v in levels], "radius_levels": levels,
                          "solver_artifacts": artifacts,
                          "finite_point_approximation_qualification": qualification,
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
    run.add_argument("--particles", type=int, default=100_000)
    run.add_argument("--batches", type=int, default=100)
    run.add_argument("--threads", type=int, default=1)
    run.add_argument("--seed", type=int, default=1)
    run.add_argument("--timeout", type=int, default=3600, help=f"timeout per radius, maximum {MAX_WALL_SECONDS} seconds")
    run.add_argument("--run-id")
    run.add_argument("--i-have-the-cpu", action="store_true")
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "plan":
            manifest = load_manifest(args.manifest)
            doc = plan_document(manifest, args.upstream.expanduser().resolve(), args.cross_sections.expanduser(), args.particles, args.batches, args.threads)
            print(json.dumps(doc, indent=2, sort_keys=True))
            return 0
        return command_run(args)
    except (ValueError, OSError, subprocess.SubprocessError, IntakeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
