#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Export prospective R3 D1S/R2S aggregate batch results to FARIS comparison records.

The records bind to common physical inputs and to per-method run records. Legacy R3 output is
refused because it lacks batch-aggregated uncertainty and required input provenance.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import os
from pathlib import Path
import shutil
import stat
import tempfile
import xml.etree.ElementTree as ET
import zipfile

import r3_scenario as sc
import r3_dose_statistics as DS

REQUEST_SCHEMA = "faris-shutdown-dose-compare-request/v0.1"
ESTIMATE_SCHEMA = "faris-shutdown-dose-estimate/v0.1"
QUANTITY = "icrp116_ap_effective_dose_rate"
UNIT = "uSv/h"
NORMALIZATION = ("all_transport_emission_into_shared_response_region; volume_average; "
                 "tracklength_flux_times_ICRP116_AP_effective_dose; source_rate_scaled; "
                 "R2S_activation_mesh_starting_discretization_unconverged")
MAX_RUN_RECORD_BYTES = 16 * 1024 * 1024
MAX_BATCH_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_BATCHES = 250_000
MAX_BASIS_ARTIFACT_BYTES = 256 * 1024 * 1024
MAX_H5M_VERTEX_NODES = 20_000_000
MAX_H5M_VERTEX_DATA_BYTES = 256 * 1024 * 1024
MAX_H5M_VERTEX_CHUNK_BYTES = 64 * 1024 * 1024


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _write_bytes(path: Path, data: bytes) -> dict:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return {"path": path.as_posix(), "sha256": sha256_bytes(data)}


def _json_bytes(value) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def _materials_xml(materials) -> str:
    """Serialize OpenMC Materials using its supported XML writer API."""
    output = io.StringIO()
    materials._write_xml(output, header=False, trailing_indent=False)
    return output.getvalue()


def _mesh_definition(mesh) -> str:
    """Serialize mesh geometry while omitting OpenMC's nonphysical reference ID."""
    element = ET.fromstring(ET.tostring(mesh.to_xml_element(), encoding="utf-8"))
    element.attrib.pop("id", None)
    return ET.tostring(element, encoding="unicode")


def _source_entry(source) -> dict:
    """Serialize a source with any referenced spatial mesh geometry bound inline."""
    element = ET.fromstring(ET.tostring(source.to_xml_element(), encoding="utf-8"))
    meshes = []
    if getattr(source, "type", None) == "mesh" and getattr(source, "mesh", None) is not None:
        meshes.append({"role": "mesh_source", "definition": _mesh_definition(source.mesh)})
        element.attrib.pop("mesh", None)
    space = getattr(source, "space", None)
    spatial_mesh = getattr(space, "mesh", None)
    if spatial_mesh is not None:
        meshes.append({"role": "spatial_distribution", "definition": _mesh_definition(spatial_mesh)})
        for child in element.iter("mesh"):
            child.text = None
            child.attrib.pop("id", None)
    return {"source_xml": ET.tostring(element, encoding="unicode"), "meshes": meshes}


def _source_entries(sources) -> list[dict]:
    if hasattr(sources, "to_xml_element"):
        sources = [sources]
    entries = [_source_entry(source) for source in sources]
    if not entries or len(entries) > MAX_BATCHES:
        raise ValueError("source artifact must contain 1..250000 source entries")
    return entries


def _open_regular(path: Path):
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError(f"input must be a regular file: {path}")
    return os.fdopen(fd, "rb")


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with _open_regular(path) as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _h5m_vertex_bounds(path: Path) -> dict:
    """Read the exact H5M node-coordinate dataset in bounded chunks and reject indirect data."""
    try:
        import h5py
        import numpy as np
    except ImportError as exc:
        raise ValueError("full-emission export requires h5py and numpy to verify H5M vertex bounds") from exc
    path = Path(path)
    with _open_regular(path) as stream:
        with h5py.File(stream, "r") as data:
            group = data
            for part in ("tstt", "nodes"):
                link = group.get(part, getlink=True)
                if not isinstance(link, h5py.HardLink):
                    raise ValueError(f"H5M vertex path contains a nonlocal HDF5 link: {part}")
                group = group[part]
                if not isinstance(group, h5py.Group):
                    raise ValueError(f"H5M vertex path component is not a group: {part}")
            link = group.get("coordinates", getlink=True)
            if not isinstance(link, h5py.HardLink):
                raise ValueError("H5M vertex coordinates must be stored in a local hard-linked dataset")
            vertices = group["coordinates"]
            if not isinstance(vertices, h5py.Dataset) or vertices.is_virtual or vertices.external:
                raise ValueError("H5M vertex coordinates must be a local, non-virtual dataset")
            if len(vertices.shape) != 2 or vertices.shape[1] != 3 or vertices.shape[0] < 1:
                raise ValueError("H5M vertex coordinates must have shape (N, 3) with N positive")
            if vertices.dtype.kind != "f" or vertices.dtype.itemsize not in (4, 8):
                raise ValueError("H5M vertex coordinates must use float32 or float64 values")
            count = int(vertices.shape[0])
            data_bytes = count * 3 * int(vertices.dtype.itemsize)
            if count > MAX_H5M_VERTEX_NODES or data_bytes > MAX_H5M_VERTEX_DATA_BYTES:
                raise ValueError("H5M vertex coordinate dataset exceeds the node-count or 256 MiB read cap")
            if vertices.chunks and math.prod(vertices.chunks) * int(vertices.dtype.itemsize) > MAX_H5M_VERTEX_CHUNK_BYTES:
                raise ValueError("H5M vertex dataset chunk exceeds the 64 MiB decompression cap")
            lower = np.full(3, math.inf, dtype=float)
            upper = np.full(3, -math.inf, dtype=float)
            max_radius = 0.0
            rows_per_chunk = max(1, min(131_072, MAX_H5M_VERTEX_CHUNK_BYTES // (3 * vertices.dtype.itemsize)))
            for first in range(0, count, rows_per_chunk):
                points = np.asarray(vertices[first:first + rows_per_chunk, :], dtype=float)
                if not np.isfinite(points).all():
                    raise ValueError("H5M vertex coordinates contain non-finite values")
                lower = np.minimum(lower, points.min(axis=0))
                upper = np.maximum(upper, points.max(axis=0))
                max_radius = max(max_radius, float(np.linalg.norm(points, axis=1).max()))
    return {"lower_left_cm": list(map(float, lower)), "upper_right_cm": list(map(float, upper)),
            "max_vertex_radius_cm": max_radius, "node_count": count, "coordinate_data_bytes": data_bytes}


def _validate_ref(name: str, ref) -> None:
    if not isinstance(ref, dict) or not isinstance(ref.get("path"), str):
        raise ValueError(f"{name} basis reference must contain a relative path")
    relative = Path(ref["path"])
    if relative.is_absolute() or ".." in relative.parts or not isinstance(ref.get("sha256"), str):
        raise ValueError(f"{name} basis reference must contain a relative path and SHA-256")
    digest = ref["sha256"]
    if len(digest) != 64 or any(c not in "0123456789abcdefABCDEF" for c in digest):
        raise ValueError(f"{name} basis reference has an invalid SHA-256")


def _read_json_record(path: Path) -> dict:
    if path.stat().st_size > MAX_RUN_RECORD_BYTES:
        raise ValueError(f"run record exceeds {MAX_RUN_RECORD_BYTES} bytes: {path}")
    with _open_regular(path) as stream:
        data = stream.read(MAX_RUN_RECORD_BYTES + 1)
    if len(data) > MAX_RUN_RECORD_BYTES:
        raise ValueError(f"run record exceeds {MAX_RUN_RECORD_BYTES} bytes: {path}")
    try:
        value = json.loads(data)
    except (json.JSONDecodeError, RecursionError) as exc:
        raise ValueError(f"invalid run record JSON in {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"run record must be a JSON object: {path}")
    return value


def write_basis_artifacts(out: Path, model, primary_source, mesh, irradiation, response_energies,
                          response_coefficients, geometry_dependencies: dict, materials_dependencies: dict) -> dict:
    """Write exact serialized R3 basis artifacts and their hashes for both methods to reference."""
    basis = out / "shutdown_dose_basis"
    geometry = _json_bytes({
        "geometry_xml": ET.tostring(model.geometry.to_xml_element(), encoding="unicode"),
        "dependencies": geometry_dependencies,
    })
    materials = _json_bytes({
        "materials_xml": _materials_xml(model.materials),
        "dependencies": materials_dependencies,
    })
    source = _json_bytes({"kind": "primary neutron source definitions", "sources": _source_entries(primary_source)})
    irradiation_bytes = _json_bytes(irradiation)
    response_bytes = _json_bytes({"quantity": QUANTITY, "coefficient_set": "ICRP-116 AP photon effective dose",
                                  "energy_eV": list(map(float, response_energies)),
                                  "coefficient_pSv_cm2": list(map(float, response_coefficients)),
                                  "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
                                  "emission_scope": "all transported photon births and descendants from the full RM-M geometry may score in this shared response region",
                                  "scored_response_region_lower_left_cm": list(mesh.lower_left),
                                  "scored_response_region_upper_right_cm": list(mesh.upper_right)})
    location_bytes = _json_bytes({"lower_left_cm": list(mesh.lower_left), "upper_right_cm": list(mesh.upper_right),
                                  "dimension": list(mesh.dimension), "voxel_cm": sc.MESH_VOXEL_CM,
                                  "aggregation": "sum of voxel track-length response; volume average over this region"})
    artifacts = {
        "geometry": _write_bytes(basis / "geometry.xml", geometry),
        "materials": _write_bytes(basis / "materials.xml", materials),
        "source": _write_bytes(basis / "primary_neutron_source.json", source),
        "irradiation": _write_bytes(basis / "irradiation.json", irradiation_bytes),
        "response": _write_bytes(basis / "response.json", response_bytes),
        "location": _write_bytes(basis / "location.json", location_bytes),
    }
    for ref in artifacts.values():
        ref["path"] = Path(ref["path"]).relative_to(out).as_posix()
    return artifacts


def write_d1s_source_artifact(out: Path, time_index: int, parent_names: list[str], factors: dict[str, float],
                              chain_sha256: str) -> dict:
    """Record D1S's method-specific decay-parent source factors separately from the primary neutron source."""
    payload = {"kind": "D1S reaction-site decay photons", "time_index": time_index,
               "chain_sha256": chain_sha256, "parent_nuclides": parent_names,
               "time_correction_factor_by_parent": {name: float(factors[name]) for name in parent_names}}
    path = out / "run" / f"d1s-photon-source-time-{time_index}.json"
    data = _json_bytes(payload)
    _write_bytes(path, data)
    return {"path": path.relative_to(out).as_posix(), "sha256": sha256_bytes(data)}


def write_r2s_source_artifact(out: Path, time_index: int, sources) -> dict:
    """Serialize each actual generated R2S MeshSource with its referenced mesh definition."""
    data = _json_bytes({"kind": "R2S generated mesh decay-photon sources", "time_index": time_index,
                        "sources": _source_entries(sources)})
    path = out / "photon_transport" / f"time_{time_index}" / "decay-photon-source.json"
    _write_bytes(path, data)
    return {"path": path.relative_to(out).as_posix(), "sha256": sha256_bytes(data)}


def _file_ref(export_dir: Path, source_dir: Path, ref: dict, target_name: str | None = None) -> dict:
    src = source_dir / ref["path"]
    digest = _sha256_file(src)
    if digest.lower() != ref["sha256"].lower():
        raise ValueError(f"basis artifact hash mismatch: {src}")
    target = export_dir / "artifacts" / (target_name or Path(ref["path"]).name)
    target.parent.mkdir(parents=True, exist_ok=True)
    check = hashlib.sha256()
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o600)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError(f"export target must be a regular file: {target}")
    try:
        with _open_regular(src) as source, os.fdopen(fd, "wb") as dest:
            for block in iter(lambda: source.read(1 << 20), b""):
                check.update(block)
                dest.write(block)
    except Exception:
        try:
            os.close(fd)
        except OSError:
            pass
        raise
    copied = {"path": target.relative_to(export_dir).as_posix(), "sha256": check.hexdigest()}
    if copied["sha256"].lower() != digest.lower():
        raise ValueError(f"basis artifact changed while being copied: {src}")
    return {"path": copied["path"], "sha256": digest}


def _verify_basis_files(root: Path, basis: dict) -> None:
    for name, ref in basis.items():
        path = root / ref["path"]
        if path.stat().st_size > MAX_BASIS_ARTIFACT_BYTES:
            raise ValueError(f"{name} basis artifact exceeds size limit: {path}")
        digest = _sha256_file(path)
        if digest.lower() != str(ref["sha256"]).lower():
            raise ValueError(f"{name} basis artifact hash mismatch: {path}")


def _activation_domain(root: Path, record: dict) -> dict:
    ref = record.get("activation_domain")
    _validate_ref("R2S full-geometry activation domain", ref)
    path = root / ref["path"]
    if _sha256_file(path).lower() != ref["sha256"].lower():
        raise ValueError("R2S activation-domain artifact hash mismatch")
    domain = _read_json_record(path)
    if domain.get("schema") != "faris-r3-activation-domain/v1" or \
            domain.get("source_problem_protocol") != sc.FULL_EMISSION_PROTOCOL:
        raise ValueError("R2S activation-domain artifact is not the full-emission protocol")
    for field in ("photon_source_scope", "emission_transport_scope", "source_rate_normalization",
                  "discretization_limit"):
        if not isinstance(domain.get(field), str) or not domain[field]:
            raise ValueError(f"R2S activation-domain artifact lacks {field}")
    geometry = domain.get("geometry")
    if not isinstance(geometry, dict):
        raise ValueError("R2S activation-domain artifact lacks its H5M geometry identity")
    h5m_path = Path(geometry.get("h5m_path", ""))
    card_path = Path(geometry.get("model_card_path", ""))
    if not h5m_path.is_absolute() or not card_path.is_absolute():
        raise ValueError("R2S activation domain must bind absolute H5M and model-card files")
    h5m_hash = geometry.get("h5m_sha256")
    card_hash = geometry.get("model_card_sha256")
    if _sha256_file(h5m_path).lower() != str(h5m_hash).lower():
        raise ValueError("R2S H5M hash differs from its activation-domain record")
    if _sha256_file(card_path).lower() != str(card_hash).lower():
        raise ValueError("R2S model-card hash differs from its activation-domain record")
    card = _read_json_record(card_path)
    if card.get("schema") != "faris-rm-m-model-card/v1" or \
            card.get("outputs", {}).get("h5m_sha256", "").lower() != str(h5m_hash).lower():
        raise ValueError("R2S model card does not pin the recorded H5M bytes")
    radius = float(geometry.get("model_card_bounding_sphere_radius_cm", 0.0))
    if radius != float(card.get("bounding_sphere_radius", {}).get("value", 0.0)):
        raise ValueError("R2S activation domain changed the model-card containment radius")
    actual_lower = geometry.get("h5m_vertex_lower_left_cm")
    actual_upper = geometry.get("h5m_vertex_upper_right_cm")
    if not math.isfinite(radius) or radius <= 0.0 or not isinstance(actual_lower, list) or not isinstance(actual_upper, list):
        raise ValueError("R2S H5M vertex extents or model-card containment radius are invalid")
    vertices = _h5m_vertex_bounds(h5m_path)
    if _sha256_file(h5m_path).lower() != str(h5m_hash).lower():
        raise ValueError("R2S H5M changed while its node coordinates were being inspected")
    if actual_lower != vertices["lower_left_cm"] or actual_upper != vertices["upper_right_cm"]:
        raise ValueError("R2S activation-domain vertex bounds differ from the actual H5M node coordinates")
    recorded_vertex_radius = geometry.get("h5m_max_vertex_radius_cm")
    if recorded_vertex_radius != vertices["max_vertex_radius_cm"]:
        raise ValueError("R2S activation-domain vertex radius differs from the actual H5M node coordinates")
    if vertices["max_vertex_radius_cm"] > radius:
        raise ValueError("R2S H5M vertex bounds exceed the model-card containment sphere")
    activation = domain.get("activation_mesh")
    if not isinstance(activation, dict):
        raise ValueError("R2S activation-domain artifact lacks its mesh definition")
    expected = sc.activation_grid_spec(actual_lower, actual_upper, float(activation.get("cell_cm", 0.0)))
    for key in ("lower_left_cm", "upper_right_cm", "dimension", "cell_count", "estimated_mesh_array_bytes"):
        if activation.get(key) != expected[key]:
            raise ValueError(f"R2S activation mesh {key} does not match the verified H5M bounds and cell size")
    if not isinstance(activation.get("mesh_xml"), str):
        raise ValueError("R2S activation mesh lacks its serialized OpenMC definition")
    mesh_xml = ET.fromstring(activation["mesh_xml"])
    mesh_lower = list(map(float, (mesh_xml.findtext("lower_left") or "").split()))
    mesh_upper = list(map(float, (mesh_xml.findtext("upper_right") or "").split()))
    mesh_dim = list(map(int, (mesh_xml.findtext("dimension") or "").split()))
    if mesh_lower != expected["lower_left_cm"] or mesh_upper != expected["upper_right_cm"] or \
            mesh_dim != expected["dimension"] or "id" in mesh_xml.attrib:
        raise ValueError("serialized R2S activation mesh XML does not match its bounds and dimensions")
    response = domain.get("response_mesh")
    if not isinstance(response, dict) or response.get("lower_left_cm") != record.get("mesh", {}).get("lower_left_cm") or \
            response.get("upper_right_cm") != record.get("mesh", {}).get("upper_right_cm") or \
            response.get("dimension") != record.get("mesh", {}).get("dimension"):
        raise ValueError("R2S activation-domain record confuses its activation and dose-response meshes")
    domain["_reference"] = ref
    return domain


def _verify_file_identity(name: str, identity: dict) -> None:
    if not isinstance(identity, dict) or not isinstance(identity.get("path"), str) or \
            not isinstance(identity.get("sha256"), str):
        raise ValueError(f"{name} file identity is unavailable")
    path = Path(identity["path"])
    if not path.is_absolute() or len(identity["sha256"]) != 64 or \
            any(c not in "0123456789abcdefABCDEF" for c in identity["sha256"]):
        raise ValueError(f"{name} file identity is invalid")
    if _sha256_file(path).lower() != identity["sha256"].lower():
        raise ValueError(f"{name} file hash does not match the run record: {path}")


def _verify_loss_assessment(name: str, losses: dict, expected_histories: int) -> None:
    if not isinstance(losses, dict) or losses.get("status") != "PASS":
        status = losses.get("status") if isinstance(losses, dict) else "MISSING"
        raise ValueError(f"{name} lost-particle assessment is {status}; trusted export requires PASS")
    lost = losses.get("lost_particles")
    if losses.get("histories") != expected_histories or not isinstance(lost, int) or lost < 0 or \
            losses.get("fraction_limit") != 1.0e-6 or lost > 1.0e-6 * expected_histories:
        raise ValueError(f"{name} lost-particle count/history evidence exceeds or lacks the 1e-6 gate")


def _verify_tally_integrity(method: str, label: str, record: dict, summary: dict,
                            model_path: Path, expected_histories: int) -> dict:
    if method == "d1s":
        run = record.get("run")
    else:
        photon = record.get("photon", {}).get(label)
        run = photon.get("run") if isinstance(photon, dict) else None
    integrity = run.get("integrity") if isinstance(run, dict) else None
    if not isinstance(integrity, dict):
        raise ValueError(f"{method} {label} has no pre-C-API integrity record")
    model_identity = integrity.get("solver_model")
    if not isinstance(model_identity, dict) or model_identity.get("sha256") is None:
        raise ValueError(f"{method} {label} lacks a solver model hash recorded before C API initialization")
    if Path(model_identity.get("path", "")).resolve() != model_path.resolve():
        raise ValueError(f"{method} {label} pre-C-API model path does not match its method model")
    _verify_file_identity(f"{method} {label} solver model", model_identity)
    if method == "d1s":
        try:
            model_xml = ET.parse(model_path).getroot()
        except ET.ParseError as exc:
            raise ValueError(f"D1S {label} solver model XML is invalid") from exc
        if any((node.get("type") or "").lower() in {"meshborn", "mesh_born"}
               for node in model_xml.iter("filter")):
            raise ValueError("D1S model contains MeshBornFilter and is a legacy emission-domain diagnostic")
    library = integrity.get("c_api_library")
    _verify_file_identity(f"{method} {label} C API library", library)
    losses = integrity.get("lost_particle_assessment")
    if not isinstance(losses, dict) or losses.get("status") != "PASS":
        status = losses.get("status") if isinstance(losses, dict) else "MISSING"
        raise ValueError(f"{method} {label} lost-particle assessment is {status}; trusted export requires PASS")
    if losses.get("histories") != expected_histories or losses.get("histories") != run.get("histories"):
        raise ValueError(f"{method} {label} lost-particle history count does not match completed tally histories")
    _verify_loss_assessment(f"{method} {label}", losses, expected_histories)
    if summary.get("batches") != run.get("batches"):
        raise ValueError(f"{method} {label} histories do not reconcile with completed batches")
    settings = record.get("settings", {})
    max_batches = settings.get("max_batches") if method == "d1s" else settings.get("photon_max_batches")
    stop_reason = run.get("stop_reason")
    if stop_reason == "batch_limit":
        if run.get("solver_status") != 1 or run.get("batches") != max_batches:
            raise ValueError(f"{method} {label} batch-limit completion lacks matching solver status/count")
    elif stop_reason == "time_budget":
        if run.get("solver_status") != 0:
            raise ValueError(f"{method} {label} time-budget stop has a non-normal solver status")
    else:
        raise ValueError(f"{method} {label} solver stop reason {stop_reason} is not an accepted completion")
    return integrity


def _load_run(method: str, root: Path) -> tuple[dict, dict]:
    record_name = "d1s_record.json" if method == "d1s" else "r2s_record.json"
    record = _read_json_record(root / record_name)
    expected_kind = "r3-d1s" if method == "d1s" else "r3-r2s"
    if record.get("kind") != expected_kind:
        raise ValueError(f"{root} is not a supported R3 {method} run")
    if record.get("openmc_version") != "0.15.3":
        raise ValueError(f"{method} run must record OpenMC 0.15.3")
    if record.get("source_problem_protocol") != sc.FULL_EMISSION_PROTOCOL:
        legacy = "legacy MeshBornFilter diagnostic" if method == "d1s" else "legacy response-mesh activation diagnostic"
        raise ValueError(f"{method} {legacy} is not equivalent to the full-emission reference protocol; export refused")
    settings = record.get("settings")
    if not isinstance(settings, dict):
        raise ValueError(f"{method} run lacks settings needed to reconcile transport histories")
    if method == "r2s":
        runtime = record.get("runtime_identity")
        if not isinstance(runtime, dict) or runtime.get("openmc_version") != "0.15.3":
            raise ValueError("R2S run lacks runtime C API/CLI identities")
        _verify_file_identity("R2S C API library", runtime.get("c_api_library"))
        _verify_file_identity("R2S neutron-manager sibling CLI", runtime.get("cli_executable"))
        neutron_integrity = record.get("neutron_transport_integrity")
        if not isinstance(neutron_integrity, dict) or neutron_integrity.get("status") != "PASS":
            raise ValueError("R2S neutron transport integrity is NOT_EVALUATED; trusted comparison export refused")
        neutron_model = neutron_integrity.get("solver_model")
        if not isinstance(neutron_model, dict):
            raise ValueError("R2S neutron transport model hash was not recorded before transport")
        expected_neutron_model = root / "neutron_transport" / "model.xml"
        if Path(neutron_model.get("path", "")).resolve() != expected_neutron_model.resolve():
            raise ValueError("R2S neutron transport model identity points outside the manager's recorded output")
        _verify_file_identity("R2S neutron transport model", neutron_model)
        neutron_library = neutron_integrity.get("c_api_library")
        if neutron_library != runtime.get("c_api_library"):
            raise ValueError("R2S neutron transport C API identity differs from its run-start library identity")
        neutron_histories = settings.get("neutron_batches", 0) * settings.get("neutron_particles_per_batch", 0)
        if not isinstance(neutron_histories, int) or neutron_histories < 1 or neutron_integrity.get("histories") != neutron_histories:
            raise ValueError("R2S neutron history count does not match recorded particles and batches")
        _verify_loss_assessment("R2S neutron transport", neutron_integrity.get("lost_particle_assessment"), neutron_histories)
    mesh = record.get("mesh")
    if not isinstance(settings, dict) or list(map(list, settings.get("timesteps", []))) != list(map(list, sc.TIMESTEPS)):
        raise ValueError(f"{method} run irradiation schedule does not match the R3 scenario")
    expected_rates = sc.SOURCE_RATES
    reported_rates = settings.get("source_rates") if method == "d1s" else settings.get("source_rates_n_s")
    if reported_rates != expected_rates:
        raise ValueError(f"{method} run source-rate schedule does not match the R3 scenario")
    if not isinstance(mesh, dict) or mesh.get("lower_left_cm") != list(sc.MESH_LOWER_CM) or \
            mesh.get("upper_right_cm") != list(sc.MESH_UPPER_CM) or mesh.get("dimension") != list(sc.MESH_DIMENSION):
        raise ValueError(f"{method} run scored region does not match the R3 scenario")
    if method == "d1s" and settings.get("born_filter", "missing") is not None:
        raise ValueError("D1S full-emission protocol requires no MeshBornFilter")
    if method == "d1s" and settings.get("dose_scope") != \
            "all transported photon births and descendants; score only in the response mesh":
        raise ValueError("D1S run does not record the full emission scope")
    if method == "r2s":
        record["_activation_domain_data"] = _activation_domain(root, record)
    basis = record.get("shutdown_dose_basis")
    if not isinstance(basis, dict) or set(basis) != {"geometry", "materials", "source", "irradiation", "response", "location"}:
        raise ValueError(f"{method} run lacks the complete shutdown-dose physical basis; legacy output refused")
    for name, ref in basis.items():
        _validate_ref(name, ref)
    chain_path = Path(record.get("chain", ""))
    if not chain_path.is_absolute():
        chain_path = root / chain_path
    chain_hash = record.get("chain_sha256")
    if not isinstance(chain_hash, str) or len(chain_hash) != 64 or any(c not in "0123456789abcdefABCDEF" for c in chain_hash):
        raise ValueError(f"{method} run lacks a valid activation/decay chain hash")
    if chain_path.stat().st_size > MAX_BASIS_ARTIFACT_BYTES:
        raise ValueError(f"{method} activation/decay chain exceeds the size limit")
    if _sha256_file(chain_path).lower() != chain_hash.lower():
        raise ValueError(f"{method} activation/decay chain hash does not match its file")
    irradiation = _read_json_record(root / basis["irradiation"]["path"])
    if irradiation.get("chain_sha256", "").lower() != chain_hash.lower() or \
            Path(irradiation.get("chain_path", "")).resolve() != chain_path.resolve() or \
            list(map(list, irradiation.get("timesteps", []))) != list(map(list, sc.TIMESTEPS)) or \
            irradiation.get("source_rates_neutrons_s") != sc.SOURCE_RATES:
        raise ValueError(f"{method} hashed irradiation basis does not match the recorded chain and R3 schedule")
    if not isinstance(record.get("shutdown_dose"), dict):
        raise ValueError(f"{method} run lacks batch-aggregated shutdown-dose summaries; legacy output refused")
    return record, basis


def _batch_summary(method: str, root: Path, idx: int, summary: dict) -> tuple[dict, dict]:
    import numpy as np

    relative = summary.get("batch_values_file")
    expected_hash = summary.get("batch_values_sha256")
    if not isinstance(relative, str) or Path(relative).name != relative or not isinstance(expected_hash, str):
        raise ValueError(f"{method} cooling index {idx} lacks a bounded batch-values artifact reference")
    if len(expected_hash) != 64 or any(c not in "0123456789abcdefABCDEF" for c in expected_hash):
        raise ValueError(f"{method} batch-values artifact has an invalid SHA-256")
    path = root / relative
    if path.stat().st_size > MAX_BATCH_ARCHIVE_BYTES:
        raise ValueError(f"{method} batch-values artifact exceeds size limit: {path}")
    digest = _sha256_file(path)
    if digest.lower() != expected_hash.lower():
        raise ValueError(f"{method} batch-values artifact hash mismatch: {path}")
    with _open_regular(path) as archive_bytes, zipfile.ZipFile(archive_bytes) as zipped:
        if sum(member.file_size for member in zipped.infolist()) > MAX_BATCH_ARCHIVE_BYTES:
            raise ValueError(f"{method} batch-values archive expands beyond the size limit")
    with _open_regular(path) as archive_bytes, np.load(archive_bytes, allow_pickle=False) as archive:
        key = f"time_{idx}" if method == "d1s" else "values"
        if key not in archive:
            raise ValueError(f"{method} batch-values artifact has no {key} array")
        expected_keys = {f"time_{i}" for i in sc.COOLING_LABELS} if method == "d1s" else {"values"}
        if set(archive.files) != expected_keys:
            raise ValueError(f"{method} batch-values archive has unexpected arrays")
        values = archive[key].astype(float, copy=False).reshape(-1)
    if values.size > MAX_BATCHES:
        raise ValueError(f"{method} batch-values array exceeds {MAX_BATCHES} entries")
    computed = DS.batch_summary(values)
    if int(summary.get("batches", -1)) != computed["batches"]:
        raise ValueError(f"{method} batch count does not match its batch-values artifact")
    for field in ("mean", "standard_error"):
        declared = float(summary[field])
        if not math.isclose(declared, computed[field], rel_tol=1.0e-10, abs_tol=1.0e-14):
            raise ValueError(f"{method} {field} does not match its batch-values artifact")
    return computed, {"path": relative, "sha256": digest}


def _export_records_into(d1s_dir: Path, r2s_dir: Path, out_dir: Path) -> dict:
    d1s, d1s_basis = _load_run("d1s", d1s_dir)
    r2s, r2s_basis = _load_run("r2s", r2s_dir)
    _verify_basis_files(d1s_dir, d1s_basis)
    _verify_basis_files(r2s_dir, r2s_basis)
    for key in d1s_basis:
        if d1s_basis[key]["sha256"].lower() != r2s_basis[key]["sha256"].lower():
            raise ValueError(f"D1S and R2S {key} input bytes differ")
    refs = {key: _file_ref(out_dir, d1s_dir, ref) for key, ref in d1s_basis.items()}
    source_run_refs = {}
    for method, root in (("d1s", d1s_dir), ("r2s", r2s_dir)):
        record_name = "d1s_record.json" if method == "d1s" else "r2s_record.json"
        source_run_refs[method] = _file_ref(
            out_dir, root, {"path": record_name, "sha256": _sha256_file(root / record_name)},
            f"original-{method}-run.json")
    if d1s["chain_sha256"].lower() != r2s["chain_sha256"].lower():
        raise ValueError("D1S and R2S activation/decay chain inputs differ")
    d1s_chain_path = Path(d1s["chain"])
    if not d1s_chain_path.is_absolute():
        d1s_chain_path = d1s_dir / d1s_chain_path
    chain_ref = _file_ref(out_dir, d1s_dir, {"path": str(d1s_chain_path), "sha256": d1s["chain_sha256"]},
                          "activation-decay-chain.xml")
    r2s_domain = r2s["_activation_domain_data"]
    r2s_domain_ref = _file_ref(out_dir, r2s_dir, r2s["activation_domain"], "r2s-full-activation-domain.json")
    r2s_h5m_ref = _file_ref(out_dir, r2s_dir,
                            {"path": r2s_domain["geometry"]["h5m_path"],
                             "sha256": r2s_domain["geometry"]["h5m_sha256"]},
                            "r2s-activation-geometry.h5m")
    r2s_card_ref = _file_ref(out_dir, r2s_dir,
                             {"path": r2s_domain["geometry"]["model_card_path"],
                              "sha256": r2s_domain["geometry"]["model_card_sha256"]},
                             "r2s-activation-model-card.json")
    request = {"schema": REQUEST_SCHEMA, "case_id": "R3-RM-M", "points": []}
    for idx, label in sc.COOLING_LABELS.items():
        time_s = float(sc.COOLING_SECONDS[idx])
        pair = {"cooling_time_s": time_s, "covariance": {"kind": "unknown"}}
        for method, root, run in (("d1s", d1s_dir, d1s), ("r2s", r2s_dir, r2s)):
            summary = run["shutdown_dose"].get(label)
            expected_scope = ("Monte Carlo transport batch sampling only; excludes nuclear-data and model uncertainty"
                              if method == "d1s" else
                              "conditional Monte Carlo photon batch sampling for this generated decay-photon source; neutron transport and activation-source uncertainty are not propagated")
            if not isinstance(summary, dict) or summary.get("uncertainty_scope") != expected_scope:
                raise ValueError(f"{method} {label} lacks the required uncertainty scope")
            if summary.get("unit") != "pSv.cm3/s summed over the scored region" or summary.get("time_index") != idx:
                raise ValueError(f"{method} {label} response unit or cooling-time index does not match the R3 scenario")
            computed, batch_ref = _batch_summary(method, root, idx, summary)
            batch_ref = _file_ref(out_dir, root, batch_ref, f"{method}-{idx}-batch-totals.npz")
            method_source = summary.get("method_specific_source")
            _validate_ref(f"{method} method-specific photon source", method_source)
            source_ref = _file_ref(out_dir, root, method_source, f"{method}-{idx}-photon-source{Path(method_source['path']).suffix}")
            mean = float(computed["mean"])
            se = float(computed["standard_error"])
            batches = int(computed["batches"])
            if batches < 2 or not math.isfinite(mean) or not math.isfinite(se) or mean < 0 or se < 0:
                raise ValueError(f"{method} {label} has invalid or unavailable batch statistics")
            value = sc.dose_rate_usv_per_h(mean, sc.mesh_volume_cm3())
            standard_error = sc.dose_rate_usv_per_h(se, sc.mesh_volume_cm3())
            if not math.isfinite(value) or not math.isfinite(standard_error):
                raise ValueError(f"{method} {label} converts to a non-finite estimate")
            streams = [f"openmc-{run['openmc_version']}-{method}-seed-{run['seed'] if method == 'd1s' else run['settings']['seeds'][1]}"]
            method_model_path = root / "run" / "model.xml" if method == "d1s" else root / "photon_transport" / f"time_{idx}" / "model.xml"
            if not method_model_path.is_file():
                raise ValueError(f"{method} {label} has no method-specific solver model: {method_model_path}")
            run_details = run.get("run", {}) if method == "d1s" else run.get("photon", {}).get(label, {})
            particles_per_batch = (run.get("particles_per_batch") if method == "d1s" else
                                   run.get("settings", {}).get("photon_particles_per_batch"))
            if not isinstance(particles_per_batch, int) or particles_per_batch < 1:
                raise ValueError(f"{method} {label} lacks its recorded particles per batch")
            expected_histories = int(run_details.get("batches", 0)) * particles_per_batch
            integrity = _verify_tally_integrity(method, label, run, summary, method_model_path, expected_histories)
            method_model_ref = _file_ref(
                out_dir, root,
                {"path": method_model_path.relative_to(root).as_posix(),
                 "sha256": integrity["solver_model"]["sha256"]},
                f"{method}-{idx}-solver-model.xml")
            run_payload = {
                "schema": "faris-shutdown-dose-run/v0.1", "method": method, "cooling_time_s": time_s,
                "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
                "emission_scope": "all emitted photons and descendants in the full RM-M geometry; dose is scored in the shared response region",
                "random_streams": streams, "estimate": {"value": value, "standard_error": standard_error},
                "response": {"quantity": QUANTITY, "unit": UNIT, "normalization": NORMALIZATION},
                "artifacts": {f"{key}_sha256": ref["sha256"] for key, ref in refs.items()},
                "source_run_record": source_run_refs[method],
                "activation_decay_chain": chain_ref,
                "r2s_full_geometry_activation_domain": {
                    "domain_record": r2s_domain_ref, "geometry_h5m": r2s_h5m_ref,
                    "model_card": r2s_card_ref,
                    "mesh": r2s_domain["activation_mesh"],
                    "discretization_limit": r2s_domain["discretization_limit"],
                },
                "method_specific_solver_model": method_model_ref,
                "c_api_library": integrity["c_api_library"],
                "lost_particle_assessment": integrity["lost_particle_assessment"],
                "method_specific_photon_source": {**source_ref,
                    "kind": "D1S reaction-site parent factors" if method == "d1s" else "R2S generated mesh photon source",
                    "note": "Method-specific photon-source artifact is distinct from the shared primary neutron source basis."},
                "batch_statistics": {"batches": batches, "batch_values": batch_ref,
                                     "source_scope": summary["uncertainty_scope"],
                                     "method": "per-batch cumulative tally differences aggregated over all spatial bins before standard error"},
            }
            run_name = f"{method}-{idx}-run.json"
            run_data = _json_bytes(run_payload)
            run_ref = _write_bytes(out_dir / run_name, run_data)
            estimate_payload = {
                "schema": ESTIMATE_SCHEMA, "method": method, "cooling_time_s": time_s,
                "quantity": QUANTITY, "unit": UNIT, "normalization": NORMALIZATION,
                "location": refs["location"], "estimate": {"value": value, "standard_error": standard_error},
                "geometry": refs["geometry"], "materials": refs["materials"], "source": refs["source"],
                "irradiation": refs["irradiation"], "response": refs["response"],
                "run_record": {"path": run_name, "sha256": run_ref["sha256"]}, "random_streams": streams,
            }
            estimate_name = f"{method}-{idx}-estimate.json"
            estimate_ref = _write_bytes(out_dir / estimate_name, _json_bytes(estimate_payload))
            pair[method] = {"path": estimate_name, "sha256": estimate_ref["sha256"]}
        request["points"].append(pair)
    request_path = out_dir / "shutdown-dose-request.json"
    request_path.write_bytes(_json_bytes(request))
    return request


def export_records(d1s_dir: Path, r2s_dir: Path, out_dir: Path) -> dict:
    """Build into a staging directory and atomically publish to a new output path."""
    if os.path.lexists(out_dir):
        raise ValueError(f"export output already exists; choose a new directory: {out_dir}")
    out_dir.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{out_dir.name}.partial-", dir=out_dir.parent))
    try:
        request = _export_records_into(d1s_dir, r2s_dir, stage)
        os.replace(stage, out_dir)
        return request
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--d1s", required=True, type=Path)
    parser.add_argument("--r2s", required=True, type=Path)
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        request = export_records(args.d1s, args.r2s, args.out_dir)
    except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError, zipfile.BadZipFile, RecursionError) as exc:
        parser.error(str(exc))
    print(json.dumps({"request": str(args.out_dir / "shutdown-dose-request.json"), "cooling_points": len(request["points"]),
                      "covariance": "unknown"}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
