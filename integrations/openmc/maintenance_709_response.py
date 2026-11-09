#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded OpenMC 709-group response and spatial photon transport stages.

The neutron response is scored once per (mesh element, material) in integrated flux group bins.
Rust runs the matching ACTINV requests and returns checked activation results and emitters; the
second stage performs one full-geometry photon transport run per cooling point.
"""
from __future__ import annotations

import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import stat
import time
import xml.etree.ElementTree as ET
import zipfile

import numpy as np

MAX_NEUTRON_GROUPS = 709
MAX_MATERIAL_BINS = 5000
MAX_MESH_VOXELS = 3_000_000
MAX_OUTPUT_BYTES = 256 * 1024 * 1024
MAX_OPENMC_TALLY_BYTES = 256 * 1024 * 1024
MAX_TOTAL_STAGE_BYTES = 512 * 1024 * 1024
MAX_H5M_VERTEX_NODES = 20_000_000
MAX_H5M_VERTEX_DATA_BYTES = 256 * 1024 * 1024
MAX_H5M_VERTEX_CHUNK_BYTES = 64 * 1024 * 1024
LOST_PARTICLE_FRACTION_LIMIT = 1e-6

NEUTRON_REQUEST_SCHEMA = "faris-maintenance-709-neutron-request/v0.1"
NEUTRON_RECEIPT_SCHEMA = "faris-maintenance-709-neutron-response-receipt/v0.1"
PHOTON_REQUEST_SCHEMA = "faris-maintenance-709-photon-request/v0.1"
PHOTON_RECEIPT_SCHEMA = "faris-maintenance-709-photon-receipt/v0.1"
ACTIVATION_BATCHES_SCHEMA = "faris-maintenance-actinv-event-batches/v0.1"
ACTIVATION_RESULT_SCHEMA = "faris-activation-result/v0.1"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _open_regular(path: Path):
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError(f"input must be a regular file: {path}")
    return os.fdopen(fd, "rb")


def checked_ref(ref: dict, label: str, maximum_bytes: int = MAX_OUTPUT_BYTES) -> Path:
    if not isinstance(ref, dict) or set(ref) != {"path", "sha256"}:
        raise ValueError(f"{label} reference must contain exactly path and sha256")
    path = Path(ref["path"]).resolve(strict=True)
    if not path.is_file() or path.stat().st_size > maximum_bytes:
        raise ValueError(f"{label} must be a regular file no larger than {maximum_bytes} bytes")
    if sha256_file(path).lower() != str(ref["sha256"]).lower():
        raise ValueError(f"{label} SHA-256 mismatch")
    return path


def _load_response_npz(path: Path) -> dict[str, np.ndarray]:
    expected = {"element_indices.npy": MAX_MATERIAL_BINS * 4,
                "material_ids.npy": MAX_MATERIAL_BINS * 4,
                "material_volumes_cm3.npy": MAX_MATERIAL_BINS * 8,
                "integrated_track_length_cm_per_source.npy": MAX_MATERIAL_BINS * MAX_NEUTRON_GROUPS * 8}
    with zipfile.ZipFile(path) as archive:
        infos = archive.infolist()
        if {info.filename for info in infos} != set(expected):
            raise ValueError("709 response NPZ has an unexpected array catalogue")
        if any(info.file_size > expected[info.filename] for info in infos) or \
                sum(info.file_size for info in infos) > MAX_TOTAL_STAGE_BYTES:
            raise ValueError("709 response NPZ exceeds its bounded decompressed-array size")
    with np.load(path, allow_pickle=False) as archive:
        return {key: archive[key] for key in ("element_indices", "material_ids", "material_volumes_cm3",
                                              "integrated_track_length_cm_per_source")}


def _finite(value, label, minimum=None):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError(f"{label} must be finite")
    value = float(value)
    if minimum is not None and value < minimum:
        raise ValueError(f"{label} must be >= {minimum}")
    return value


def validate_mesh(mesh: dict) -> tuple[list[float], list[float], list[int], float]:
    if not isinstance(mesh, dict) or set(mesh) != {"lower_left_cm", "upper_right_cm", "dimension"}:
        raise ValueError("activation_mesh must contain lower_left_cm, upper_right_cm and dimension")
    lower = [_finite(x, "activation mesh lower bound") for x in mesh["lower_left_cm"]]
    upper = [_finite(x, "activation mesh upper bound") for x in mesh["upper_right_cm"]]
    dims = mesh["dimension"]
    if len(lower) != 3 or len(upper) != 3 or len(dims) != 3:
        raise ValueError("activation mesh bounds and dimensions must be three-vectors")
    if any(isinstance(d, bool) or not isinstance(d, int) or d < 1 for d in dims):
        raise ValueError("activation mesh dimensions must be positive integers")
    voxels = math.prod(dims)
    if voxels > MAX_MESH_VOXELS:
        raise ValueError(f"activation mesh exceeds the {MAX_MESH_VOXELS} voxel memory bound")
    if any(a >= b for a, b in zip(lower, upper)):
        raise ValueError("activation mesh upper bounds must exceed lower bounds")
    voxel_cm3 = math.prod((upper[i] - lower[i]) / dims[i] for i in range(3))
    # MeshMaterialVolumes currently stores up to four material IDs and volumes per voxel.
    mmv_bytes = voxels * 4 * (4 + 8)
    tally_bytes = MAX_MATERIAL_BINS * MAX_NEUTRON_GROUPS * 3 * 8
    response_bytes = MAX_MATERIAL_BINS * MAX_NEUTRON_GROUPS * 8
    if mmv_bytes + tally_bytes + response_bytes > MAX_TOTAL_STAGE_BYTES:
        raise ValueError("activation mesh plus 709-group tally exceeds the 512 MiB worker memory bound")
    if tally_bytes > MAX_OPENMC_TALLY_BYTES:
        raise ValueError("predicted OpenMC tally allocation exceeds 256 MiB")
    return lower, upper, dims, voxel_cm3


def h5m_vertex_bounds(path: Path) -> dict:
    """Read bounded, local MOAB node coordinates used to prove activation mesh coverage."""
    try:
        import h5py
    except ImportError as exc:
        raise ValueError("709-group response requires h5py for checked H5M coverage") from exc
    with _open_regular(path) as stream, h5py.File(stream, "r") as data:
        group = data
        for name in ("tstt", "nodes"):
            link = group.get(name, getlink=True)
            if not isinstance(link, h5py.HardLink):
                raise ValueError(f"H5M node path contains a nonlocal HDF5 link: {name}")
            group = group[name]
            if not isinstance(group, h5py.Group):
                raise ValueError(f"H5M node path component is not a group: {name}")
        link = group.get("coordinates", getlink=True)
        if not isinstance(link, h5py.HardLink):
            raise ValueError("H5M node coordinates must be a local hard-linked dataset")
        nodes = group["coordinates"]
        if not isinstance(nodes, h5py.Dataset) or nodes.is_virtual or nodes.external:
            raise ValueError("H5M node coordinates must be a local non-virtual dataset")
        if len(nodes.shape) != 2 or nodes.shape[1] != 3 or nodes.shape[0] < 1:
            raise ValueError("H5M node coordinate shape must be (N, 3), N positive")
        if nodes.dtype.kind != "f" or nodes.dtype.itemsize not in (4, 8):
            raise ValueError("H5M node coordinates must be float32 or float64")
        count = int(nodes.shape[0])
        data_bytes = count * 3 * int(nodes.dtype.itemsize)
        if count > MAX_H5M_VERTEX_NODES or data_bytes > MAX_H5M_VERTEX_DATA_BYTES:
            raise ValueError("H5M node coordinates exceed the bounded node count or 256 MiB raw data cap")
        if nodes.chunks and math.prod(nodes.chunks) * int(nodes.dtype.itemsize) > MAX_H5M_VERTEX_CHUNK_BYTES:
            raise ValueError("H5M coordinate chunk exceeds the 64 MiB decompression cap")
        lower = np.full(3, math.inf)
        upper = np.full(3, -math.inf)
        stride = max(1, min(131_072, MAX_H5M_VERTEX_CHUNK_BYTES // (3 * nodes.dtype.itemsize)))
        for start in range(0, count, stride):
            points = np.asarray(nodes[start:start + stride], dtype=np.float64)
            if not np.isfinite(points).all():
                raise ValueError("H5M node coordinates contain non-finite values")
            lower = np.minimum(lower, points.min(axis=0))
            upper = np.maximum(upper, points.max(axis=0))
    return {"lower_left_cm": lower.tolist(), "upper_right_cm": upper.tolist(), "node_count": count,
            "coordinate_data_bytes": data_bytes}


def validate_mesh_covers_geometry(mesh: dict, vertices: dict) -> None:
    lower, upper, _, _ = validate_mesh(mesh)
    if any(a > b for a, b in zip(lower, vertices["lower_left_cm"])) or \
            any(a < b for a, b in zip(upper, vertices["upper_right_cm"])):
        raise ValueError("activation mesh does not cover the exact H5M node-coordinate bounds")


def source_kernel_identity(model_xml: Path) -> dict:
    """Hash and validate the source definitions actually serialized in the input model XML."""
    root = ET.parse(model_xml).getroot()
    settings = root.find("settings")
    if settings is None:
        raise ValueError("OpenMC model has no settings/source section")
    sources = settings.findall("source")
    if not sources:
        raise ValueError("OpenMC model has no authored neutron source")
    referenced_meshes = set()
    total_strength = 0.0
    for source in sources:
        kind = source.attrib.get("type", "independent").lower()
        if kind not in {"independent", "mesh"}:
            raise ValueError(f"OpenMC source type {kind!r} is not a data-defined source")
        if any(k.lower() in source.attrib for k in ("library", "module", "callable", "python_module")):
            raise ValueError("OpenMC source XML contains an executable plugin reference")
        strength = _finite(float(source.attrib.get("strength", "nan")), "neutron source strength", 0.0)
        total_strength += strength
        if kind == "mesh":
            mesh_id = source.attrib.get("mesh")
            if mesh_id is None:
                raise ValueError("mesh neutron source lacks its referenced mesh ID")
            referenced_meshes.add(mesh_id)
            nested = source.findall("source")
            if not nested or any(s.attrib.get("particle", "neutron") != "neutron" for s in nested):
                raise ValueError("mesh neutron source must contain data-defined neutron source bins")
            nested_strength = math.fsum(_finite(float(s.attrib.get("strength", "nan")),
                                                "mesh-bin neutron strength", 0.0) for s in nested)
            if not math.isclose(nested_strength, strength, rel_tol=1e-12, abs_tol=1e-15):
                raise ValueError("mesh source strength does not reconcile to its actual source-bin strengths")
        elif source.attrib.get("particle", "neutron") != "neutron":
            raise ValueError("fixed-source kernel must emit neutrons")
    if not math.isclose(total_strength, 1.0, rel_tol=1e-12, abs_tol=1e-15):
        raise ValueError(f"OpenMC neutron source kernel must have unit total strength; found {total_strength}")
    mesh_nodes = []
    for mesh_node in settings.findall("mesh"):
        if mesh_node.attrib.get("id") in referenced_meshes:
            mesh_nodes.append(mesh_node)
    if {node.attrib.get("id") for node in mesh_nodes} != referenced_meshes:
        raise ValueError("OpenMC source references a mesh with no in-model definition")
    basis_xml = "<basis>" + "".join(ET.tostring(node, encoding="unicode") for node in sources + mesh_nodes) + "</basis>"
    canonical = ET.canonicalize(xml_data=basis_xml, strip_text=True)
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    return {"kernel_sha256": digest, "unit_strength": total_strength, "source_types": [s.attrib.get("type", "independent") for s in sources]}


def material_response_rows(volumes, active_material_ids: set[int]) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Return the exact OpenMC element-major x-fastest mesh/material bins and scored volumes."""
    materials = np.asarray(volumes._materials)
    material_volumes = np.asarray(volumes._volumes, dtype=np.float64)
    if materials.shape != material_volumes.shape or materials.ndim != 2:
        raise ValueError("OpenMC MeshMaterialVolumes arrays have incompatible shapes")
    rows, slots = np.where((materials > 0) & np.isin(materials, list(active_material_ids)))
    ids = materials[rows, slots].astype(np.int32, copy=True)
    vals = material_volumes[rows, slots].astype(np.float64, copy=True)
    elems = rows.astype(np.int32, copy=True)
    if not len(elems):
        raise ValueError("active installed components have no positive OpenMC mesh/material volumes")
    if len(elems) > MAX_MATERIAL_BINS:
        raise ValueError(f"active mesh/material bins exceed {MAX_MATERIAL_BINS}; use a coarser activation mesh")
    if np.any(~np.isfinite(vals)) or np.any(vals <= 0.0):
        raise ValueError("OpenMC material-volume bins must be finite and positive")
    pairs = np.column_stack((elems, ids))
    if len(np.unique(pairs, axis=0)) != len(elems):
        raise ValueError("OpenMC material-volume response repeats an element/material bin")
    return elems, ids, vals


def make_neutron_response_tally(openmc, mesh, elements: np.ndarray, material_ids: np.ndarray,
                                energy_edges_ev: list[float]):
    """Construct exact 709-group integrated track-length tally, no activation-chain dependency."""
    edges = [_finite(x, "FISPACT neutron energy edge", 0.0) for x in energy_edges_ev]
    if len(edges) != MAX_NEUTRON_GROUPS + 1 or any(b <= a for a, b in zip(edges, edges[1:])):
        raise ValueError("ACTINV response requires exactly 710 strictly ascending FISPACT-709 energy edges")
    bins = np.column_stack((elements, material_ids)).astype(np.int32, copy=False)
    mesh_material = openmc.MeshMaterialFilter(mesh, bins)
    tally = openmc.Tally(name="faris_activation_709_integrated_track_length")
    tally.filters = [mesh_material, openmc.EnergyFilter(edges)]
    tally.scores = ["flux"]
    return tally


def integrated_track_length_mean(capi_results, batch_count: int, bin_count: int,
                                 group_count: int = MAX_NEUTRON_GROUPS) -> np.ndarray:
    """Convert OpenMC CAPI sums to per-source-particle integrated track length."""
    if isinstance(batch_count, bool) or not isinstance(batch_count, int) or batch_count < 2:
        raise ValueError("at least two completed neutron batches are required")
    values = np.asarray(capi_results, dtype=np.float64)
    expected = (bin_count * group_count, 1, 3)
    if values.shape != expected:
        raise ValueError(f"OpenMC CAPI 709 tally shape {values.shape} differs from expected {expected}")
    totals = values[:, 0, 1]
    if not np.isfinite(totals).all() or np.any(totals < 0):
        raise ValueError("OpenMC 709 tally sums must be finite and nonnegative")
    return (totals / batch_count).reshape((bin_count, group_count), order="C")


def _activation_group_edges(library_path: Path, index_path: Path, library_ref: dict) -> tuple[list[float], str]:
    try:
        import numpy as np
    except ImportError as exc:
        raise ValueError("numpy is required to read ACTINV group boundaries") from exc
    with zipfile.ZipFile(library_path) as archive:
        infos = archive.infolist()
        bounds = [info for info in infos if info.filename == "bounds.npy"]
        if len(bounds) != 1 or bounds[0].file_size > 1_000_000:
            raise ValueError("ACTINV NPZ must contain one bounded bounds.npy group-edge array")
    with np.load(library_path, allow_pickle=False) as data:
        edges = np.asarray(data["bounds"], dtype=np.float64)
    if edges.shape != (MAX_NEUTRON_GROUPS + 1,) or not np.isfinite(edges).all() or np.any(np.diff(edges) <= 0):
        raise ValueError("ACTINV neutron library needs 710 finite ascending group boundaries")
    index = json.loads(index_path.read_bytes())
    if not isinstance(index, dict) or index.get("sha256_npz", "").lower() != library_ref["sha256"].lower():
        raise ValueError("ACTINV group index does not bind the checked NPZ bytes")
    boundary_hash = hashlib.sha256(b"ACTINV-GROUP-BOUNDARIES-v1\0" + edges.astype("<f8").tobytes()).hexdigest()
    if index.get("group_boundary_sha256", "").lower() != boundary_hash:
        raise ValueError("ACTINV index group-boundary hash differs from the actual 709 neutron edges")
    return edges.tolist(), boundary_hash


def _source_reference(request: dict, key: str) -> dict:
    ref = request.get(key)
    if not isinstance(ref, dict) or set(ref) != {"path", "sha256"}:
        raise ValueError(f"request {key} reference is invalid")
    return ref


def _validate_stage_request(request: dict, schema: str) -> dict:
    if not isinstance(request, dict) or request.get("schema") != schema:
        raise ValueError(f"request schema must be {schema}")
    if not isinstance(request.get("request_id"), str) or not request["request_id"].strip():
        raise ValueError("request_id must be nonempty")
    base_fields = {"schema", "request_id", "model", "geometry", "cross_sections", "component_material_map",
                   "activation_mesh", "event", "sampling"}
    stage_fields = ({"stage", "history_sha256", "history_artifact", "source_schedules",
                     "activation_library_npz", "activation_library_index", "source_groups",
                     "base_source_rate_n_s", "photon_energy_edges_ev", "response_location_artifacts"} if schema == NEUTRON_REQUEST_SCHEMA else
                    {"stage", "neutron_response_receipt", "activation_results",
                     "response_mesh", "emitters_by_cooling"})
    if set(request) != base_fields | stage_fields:
        raise ValueError(f"request fields do not exactly match {schema}")
    expected_stage = "neutron" if schema == NEUTRON_REQUEST_SCHEMA else "photon"
    if request["stage"] != expected_stage:
        raise ValueError(f"request stage must be {expected_stage}")
    for key in ("model", "geometry", "cross_sections", "component_material_map"):
        checked_ref(_source_reference(request, key), key)
    validate_mesh(request["activation_mesh"])
    event = request["event"]
    event_fields = ({"design", "class", "component", "replacement_k", "shutdown_s", "cooling_s", "location_id", "location_sha256"} if schema == NEUTRON_REQUEST_SCHEMA else
                    {"design", "class", "component", "replacement_k", "shutdown_s", "location_id", "location_sha256", "cooling_s"})
    if not isinstance(event, dict) or set(event) != event_fields:
        raise ValueError("event fields do not match the exact shutdown-dose stage schema")
    for key in ("design", "class", "component", "location_id"):
        if not isinstance(event[key], str) or not event[key].strip():
            raise ValueError(f"event {key} must be nonempty text")
    if isinstance(event["replacement_k"], bool) or not isinstance(event["replacement_k"], int) or event["replacement_k"] < 1:
        raise ValueError("event replacement_k must be a positive integer")
    if not isinstance(event["location_sha256"], str) or len(event["location_sha256"]) != 64:
        raise ValueError("event location_sha256 must be a SHA-256")
    try:
        bytes.fromhex(event["location_sha256"])
    except ValueError as exc:
        raise ValueError("event location_sha256 must be a SHA-256") from exc
    _finite(event["shutdown_s"], "shutdown time", 0.0)
    cooling = event["cooling_s"]
    if not isinstance(cooling, list) or not 1 <= len(cooling) <= 512:
        raise ValueError("cooling grid must contain 1 to 512 values")
    cooling = [_finite(x, "cooling time", 0.0) for x in cooling]
    if any(c <= 0 for c in cooling) or any(b <= a for a, b in zip(cooling, cooling[1:])):
        raise ValueError("cooling grid must be positive and strictly ascending")
    sampling = request["sampling"]
    expected_sampling = {"neutron_particles", "neutron_batches", "photon_particles", "photon_batches",
                         "seed", "threads", "mat_vol_samples", "activation_groups", "run_budget_s"}
    if not isinstance(sampling, dict) or set(sampling) != expected_sampling:
        raise ValueError("sampling does not match the bounded 709-group worker fields")
    if sampling["activation_groups"] not in {"VITAMIN-J-42", "CCFE-709", "FISPACT-709"}:
        raise ValueError("activation_groups must name a supported recorded group structure")
    for key, max_value in (("neutron_particles", 1_000_000), ("photon_particles", 1_000_000),
                           ("neutron_batches", 100_000), ("photon_batches", 100_000),
                           ("threads", 64), ("mat_vol_samples", 10_000_000), ("seed", 2**31 - 1)):
        value = sampling[key]
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= max_value:
            raise ValueError(f"{key} must be an integer in [1, {max_value}]")
    if sampling["neutron_batches"] < 2 or sampling["photon_batches"] < 2:
        raise ValueError("both transport stages require at least two batches")
    budget = _finite(sampling["run_budget_s"], "run budget", 1.0)
    if budget > 86400:
        raise ValueError("run budget may not exceed 86400 seconds")
    if schema == NEUTRON_REQUEST_SCHEMA:
        if not isinstance(request["history_sha256"], str) or len(request["history_sha256"]) != 64:
            raise ValueError("history_sha256 must be a lowercase SHA-256")
        try:
            bytes.fromhex(request["history_sha256"])
        except ValueError as exc:
            raise ValueError("history_sha256 must be a lowercase SHA-256") from exc
        history = checked_ref(request["history_artifact"], "Rust-authored history", MAX_OUTPUT_BYTES)
        if sha256_file(history).lower() != request["history_sha256"].lower():
            raise ValueError("history bytes do not match the request history SHA-256")
        schedules_path = checked_ref(request["source_schedules"], "component irradiation schedules", MAX_OUTPUT_BYTES)
        schedules = json.loads(schedules_path.read_bytes())
        if schedules.get("schema") != "faris-r2s-installation-schedules/v0.1" or \
                schedules.get("history_sha256", "").lower() != request["history_sha256"].lower() or \
                schedules.get("shutdown_s") != event["shutdown_s"]:
            raise ValueError("component irradiation schedules do not bind this exact event history and shutdown")
        if set(schedules.get("schedules", {})) != {group.get("component") for group in request["source_groups"]}:
            raise ValueError("component irradiation schedule artifact and source groups differ")
        if event["component"] not in schedules["schedules"]:
            raise ValueError("replacement event component is absent from the authenticated schedule bundle")
        if not isinstance(request["source_groups"], list) or not request["source_groups"] or len(request["source_groups"]) > 10_000:
            raise ValueError("source_groups must contain 1 to 10000 installed components")
        group_components = []
        for group in request["source_groups"]:
            if not isinstance(group, dict) or set(group) != {"component", "history_sha256", "installation_s", "schedule"}:
                raise ValueError("each source group must bind component, history, installation and schedule")
            if not isinstance(group["component"], str) or not group["component"]:
                raise ValueError("source group component must be nonempty text")
            _finite(group["installation_s"], "component installation time", 0.0)
            if not isinstance(group["schedule"], list) or not group["schedule"]:
                raise ValueError("source group irradiation schedule must be a nonempty list")
            group_components.append(group["component"])
            if group.get("history_sha256", "").lower() != request["history_sha256"].lower():
                raise ValueError("source group history identity differs from the exact history artifact")
        if len(group_components) != len(set(group_components)):
            raise ValueError("source group components must be unique")
        _finite(request["base_source_rate_n_s"], "base source rate", 0.0)
        if request["base_source_rate_n_s"] <= 0:
            raise ValueError("base source rate must be positive")
        if not isinstance(request["source_groups"], list) or not request["source_groups"]:
            raise ValueError("source_groups must include each installed component")
        location_refs = request["response_location_artifacts"]
        if not isinstance(location_refs, dict) or set(location_refs) != {event["location_id"]}:
            raise ValueError("response_location_artifacts must bind the one event response location")
        checked_ref(location_refs[event["location_id"]], "task response location", 64 * 1024)
        if location_refs[event["location_id"]]["sha256"].lower() != event["location_sha256"].lower():
            raise ValueError("event response-location SHA differs from its artifact reference")
        checked_ref(_source_reference(request, "activation_library_npz"), "ACTINV neutron library", 256 * 1024 * 1024)
        checked_ref(_source_reference(request, "activation_library_index"), "ACTINV neutron index", 16 * 1024 * 1024)
        photon_edges = request["photon_energy_edges_ev"]
        if not isinstance(photon_edges, list) or len(photon_edges) < 3 or len(photon_edges) > 10001:
            raise ValueError("photon_energy_edges_ev must contain 3 to 10001 values")
        photon_edges = [_finite(x, "photon energy edge", 0.0) for x in photon_edges]
        if any(b <= a for a, b in zip(photon_edges, photon_edges[1:])):
            raise ValueError("photon energy edges must be strictly ascending")
    else:
        checked_ref(_source_reference(request, "neutron_response_receipt"), "neutron response receipt", 16 * 1024 * 1024)
        if not isinstance(request["activation_results"], list) or not request["activation_results"]:
            raise ValueError("activation_results must bind one result per active component")
        response = request["response_mesh"]
        if not isinstance(response, dict) or set(response) != {"artifact", "lower_left_cm", "upper_right_cm", "dimension"}:
            raise ValueError("response_mesh must bind one location artifact, bounds and dimensions")
        checked_ref(response["artifact"], "task response location", 64 * 1024)
        lower = [_finite(x, "task response lower bound") for x in response["lower_left_cm"]]
        upper = [_finite(x, "task response upper bound") for x in response["upper_right_cm"]]
        if len(lower) != 3 or len(upper) != 3 or any(a >= b for a, b in zip(lower, upper)):
            raise ValueError("task response bounds must be ordered finite 3-vectors")
        if response["dimension"] not in ([1, 1, 1], (1, 1, 1)):
            raise ValueError("task response location must contain exactly one spatial bin")
        if request["event"].get("location_sha256", "").lower() != response["artifact"]["sha256"].lower():
            raise ValueError("event location SHA differs from task response artifact")
    return request


def _checked_model_paths(request: dict) -> tuple[Path, Path, Path, dict]:
    model_path = checked_ref(request["model"], "OpenMC model XML")
    geometry_path = checked_ref(request["geometry"], "DAGMC H5M")
    xs_path = checked_ref(request["cross_sections"], "neutron cross-sections XML")
    component_map_path = checked_ref(request["component_material_map"], "component material map")
    root = ET.parse(model_path).getroot()
    geometry_nodes = root.findall(".//dagmc/filename")
    if len(geometry_nodes) != 1 or (model_path.parent / (geometry_nodes[0].text or "")).resolve() != geometry_path:
        raise ValueError("model XML must reference exactly the hash-verified DAGMC geometry")
    component_map = json.loads(component_map_path.read_bytes())
    if not isinstance(component_map, dict) or set(component_map) != {"schema", "geometry_sha256", "materials"} or \
            component_map.get("schema") != "faris-cad-component-material-map/v0.1" or \
            component_map.get("geometry_sha256", "").lower() != request["geometry"]["sha256"].lower():
        raise ValueError("component-material map does not bind the checked H5M")
    materials = component_map.get("materials")
    if not isinstance(materials, dict) or not materials:
        raise ValueError("component-material map must contain material IDs for installed components")
    seen_material_ids = set()
    for component, ids in materials.items():
        if not isinstance(component, str) or not component or not isinstance(ids, list) or not ids:
            raise ValueError("component-material map needs a nonempty positive ID list per component")
        for material_id in ids:
            if isinstance(material_id, bool) or not isinstance(material_id, int) or material_id <= 0 or material_id in seen_material_ids:
                raise ValueError("component material IDs must be positive integers unique across CAD components")
            seen_material_ids.add(material_id)
    return model_path, geometry_path, xs_path, component_map


def _library_identity(openmc) -> dict:
    library_path = Path(openmc.lib.core._dll._name).resolve()
    if not library_path.is_file():
        raise ValueError("loaded OpenMC C API library is not a regular file")
    return {"path": str(library_path), "sha256": sha256_file(library_path)}


def _lost_particles(openmc) -> int:
    try:
        value = int(ctypes.c_int.in_dll(openmc.lib._dll, "n_lost_particles").value)
    except (AttributeError, OSError, TypeError, ValueError) as exc:
        raise RuntimeError("OpenMC C API lost-particle counter is unavailable") from exc
    if value < 0:
        raise RuntimeError("OpenMC C API returned a negative lost-particle count")
    return value


def run_live_tally(openmc, model_xml: Path, tally_id: int, particles: int, batches_max: int,
                   threads: int, budget_s: float) -> dict:
    """Run a CAPI tally in bounded batches, recording actual statuses and particle losses."""
    model_xml = Path(model_xml).resolve()
    model_hash = sha256_file(model_xml)
    library = _library_identity(openmc)
    previous_cwd = Path.cwd()
    started = time.monotonic()
    initialized = simulation_started = False
    batches = histories = lost = 0
    status = None
    tally_results = None
    batch_values = []
    previous_sums = None
    stop_reason = "solver_stop"
    try:
        os.chdir(model_xml.parent)
        openmc.lib.init(["--threads", str(threads), str(model_xml)])
        initialized = True
        openmc.lib.simulation_init()
        simulation_started = True
        tally = openmc.lib.tallies[tally_id]
        while batches < batches_max:
            if batches >= 2 and time.monotonic() - started >= budget_s:
                stop_reason = "time_budget"
                break
            status = int(openmc.lib.next_batch())
            observed_batches = int(tally.num_realizations)
            if observed_batches != batches + 1:
                raise RuntimeError("OpenMC tally realization counter did not advance one batch")
            batches = observed_batches
            current_sums = np.asarray(tally.results[:, 0, 1], dtype=np.float64).reshape(-1).copy()
            if not np.isfinite(current_sums).all():
                raise RuntimeError("OpenMC tally cumulative sums contain non-finite values")
            if previous_sums is not None:
                delta = current_sums - previous_sums
                if not np.isfinite(delta).all() or np.any(delta < -1e-12):
                    raise RuntimeError("OpenMC cumulative tally sums moved backwards")
                batch_values.append(delta)
            else:
                batch_values.append(current_sums)
            previous_sums = current_sums
            histories = batches * particles
            lost = _lost_particles(openmc)
            if lost > LOST_PARTICLE_FRACTION_LIMIT * histories:
                raise RuntimeError(f"OpenMC lost-particle fraction exceeds {LOST_PARTICLE_FRACTION_LIMIT:g} ({lost}/{histories})")
            if status == 1:
                if batches != batches_max:
                    raise RuntimeError(f"OpenMC reached maximum batches early ({batches}/{batches_max})")
                stop_reason = "batch_limit"
                break
            if status != 0:
                raise RuntimeError(f"OpenMC C API returned unsupported batch status {status}")
            if batches == batches_max:
                raise RuntimeError("OpenMC reached the configured batch count without its maximum-batch status")
        if batches >= batches_max and status == 1:
            stop_reason = "batch_limit"
        tally_results = np.asarray(tally.results, dtype=np.float64).copy()
    finally:
        try:
            if simulation_started:
                openmc.lib.simulation_finalize()
        finally:
            if initialized:
                openmc.lib.finalize()
            os.chdir(previous_cwd)
    if batches < 2 or tally_results is None:
        raise RuntimeError("OpenMC response needs at least two complete batches")
    if sha256_file(model_xml) != model_hash or _library_identity(openmc) != library:
        raise RuntimeError("OpenMC model XML or loaded C API library changed during the run")
    return {"results": tally_results, "batch_values": np.asarray(batch_values),
            "batches": batches, "histories": histories,
            "lost_particles": lost, "lost_particle_fraction_limit": LOST_PARTICLE_FRACTION_LIMIT,
            "solver_status": status, "stop_reason": stop_reason, "model_xml": {"path": str(model_xml), "sha256": model_hash},
            "c_api_library": library, "elapsed_s": time.monotonic() - started}


def _save_npz(path: Path, **arrays) -> dict:
    if path.exists():
        raise ValueError(f"refusing to overwrite response artifact {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as stream:
        np.savez_compressed(stream, **arrays)
    size = path.stat().st_size
    if size > MAX_OUTPUT_BYTES:
        path.unlink(missing_ok=True)
        raise ValueError(f"response artifact exceeds {MAX_OUTPUT_BYTES} bytes")
    return {"path": str(path.resolve()), "sha256": sha256_file(path)}


def _write_json_new(path: Path, value: dict) -> dict:
    raw = (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()
    if len(raw) > 16 * 1024 * 1024:
        raise ValueError("stage receipt exceeds 16 MiB")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as stream:
        stream.write(raw)
    return {"path": str(path.resolve()), "sha256": hashlib.sha256(raw).hexdigest()}


def run_neutron_stage(request: dict, out: Path, openmc=None, request_sha256: str | None = None) -> dict:
    """Score checked 709-group flux and write native ACTINV activation-batch requests."""
    _validate_stage_request(request, NEUTRON_REQUEST_SCHEMA)
    if not isinstance(request_sha256, str) or len(request_sha256) != 64:
        raise ValueError("exact Stage 1 request-byte SHA-256 is required")
    try:
        bytes.fromhex(request_sha256)
    except ValueError as exc:
        raise ValueError("exact Stage 1 request-byte SHA-256 is invalid") from exc
    model_path, geometry_path, xs_path, component_map = _checked_model_paths(request)
    vertex_bounds = h5m_vertex_bounds(geometry_path)
    validate_mesh_covers_geometry(request["activation_mesh"], vertex_bounds)
    try:
        from maintenance_actinv_backend import activation_batches_from_artifacts, validate_safe_model_sources
    except ImportError as exc:
        raise RuntimeError("checked maintenance ACTINV backend helpers are required before neutron transport") from exc
    validate_safe_model_sources(ET.parse(model_path).getroot())
    source_identity = source_kernel_identity(model_path)
    component_materials = component_map.get("materials", {})
    active_components = {group.get("component") for group in request["source_groups"]}
    if not active_components or None in active_components or not active_components.issubset(component_materials):
        raise ValueError("source schedules must name each active component in the checked CAD material map")
    all_ids = [int(mid) for ids in component_materials.values() for mid in ids]
    if len(set(all_ids)) != len(all_ids) or any(mid <= 0 for mid in all_ids):
        raise ValueError("CAD material IDs must be positive and unique across components")
    if openmc is None:
        import openmc
    library_path = checked_ref(request["activation_library_npz"], "ACTINV neutron library", 256 * 1024 * 1024)
    index_path = checked_ref(request["activation_library_index"], "ACTINV neutron index", 16 * 1024 * 1024)
    neutron_edges, boundary_hash = _activation_group_edges(
        library_path, index_path, request["activation_library_npz"])
    openmc.config["cross_sections"] = str(xs_path)
    model = openmc.Model.from_model_xml(str(model_path))
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.photon_transport = False
    settings.particles = request["sampling"]["neutron_particles"]
    settings.batches = request["sampling"]["neutron_batches"]
    settings.seed = request["sampling"]["seed"]
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.5
    settings.output = {"summary": False, "tallies": False}
    settings.statepoint = {"batches": []}
    settings.sourcepoint = {"write": False}
    mesh_spec = request["activation_mesh"]
    mesh = openmc.RegularMesh(name="faris-activation-709-response")
    mesh.lower_left = mesh_spec["lower_left_cm"]
    mesh.upper_right = mesh_spec["upper_right_cm"]
    mesh.dimension = mesh_spec["dimension"]
    volume_sampling_started = time.monotonic()
    material_volumes = mesh.material_volumes(
        model, n_samples=request["sampling"]["mat_vol_samples"], max_materials=4, output=False)
    volume_sampling_elapsed_s = time.monotonic() - volume_sampling_started
    if sha256_file(model_path) != request["model"]["sha256"] or sha256_file(geometry_path) != request["geometry"]["sha256"]:
        raise ValueError("model or geometry changed during OpenMC mesh material-volume sampling")
    active_ids = {int(mid) for component in active_components for mid in component_materials[component]}
    elements, material_ids, material_volumes_cm3 = material_response_rows(material_volumes, active_ids)
    total_volume_by_element = {}
    for element, volume in zip(elements, material_volumes_cm3):
        total_volume_by_element[int(element)] = total_volume_by_element.get(int(element), 0.0) + float(volume)
    _, _, _dims, voxel_cm3 = validate_mesh(mesh_spec)
    if any(volume > voxel_cm3 * 1.00001 for volume in total_volume_by_element.values()):
        raise ValueError("OpenMC sampled material volume exceeds its mesh voxel volume")

    tally = make_neutron_response_tally(openmc, mesh, elements, material_ids, neutron_edges)
    model.tallies = openmc.Tallies([tally])
    out = Path(out).resolve()
    model_dir = out / "neutron_response_model"
    model_dir.mkdir(parents=True, exist_ok=True)
    model.export_to_model_xml(str(model_dir))
    solver_model = model_dir / "model.xml"
    if not solver_model.is_file():
        raise RuntimeError("OpenMC did not export the neutron response model")
    if source_kernel_identity(solver_model)["kernel_sha256"] != source_identity["kernel_sha256"]:
        raise ValueError("exported solver model changed the checked unit-strength source kernel")
    solver_root = ET.parse(solver_model).getroot()
    solver_geometry = solver_root.findall(".//dagmc/filename")
    if len(solver_geometry) != 1 or (solver_model.parent / (solver_geometry[0].text or "")).resolve() != geometry_path:
        raise ValueError("exported solver model no longer references the checked H5M")
    run = run_live_tally(openmc, solver_model, tally.id,
                         request["sampling"]["neutron_particles"], request["sampling"]["neutron_batches"],
                         request["sampling"]["threads"], request["sampling"]["run_budget_s"])
    if run["stop_reason"] != "batch_limit" or run["solver_status"] != 1 or \
            run["batches"] != request["sampling"]["neutron_batches"]:
        raise RuntimeError("709 neutron response did not complete the configured OpenMC batch limit")
    matrix = integrated_track_length_mean(run["results"], run["batches"], len(elements))
    response_path = out / "neutron_response_709.npz"
    response_ref = _save_npz(response_path,
        element_indices=elements.astype(np.int32, copy=False),
        material_ids=material_ids.astype(np.int32, copy=False),
        material_volumes_cm3=material_volumes_cm3.astype(np.float64, copy=False),
        integrated_track_length_cm_per_source=matrix.astype(np.float64, copy=False))
    for ref, path in ((request["model"], model_path), (request["geometry"], geometry_path),
                      (request["cross_sections"], xs_path),
                      (request["component_material_map"], checked_ref(request["component_material_map"],
                       "component material map")),
                      (request["activation_library_npz"], library_path),
                      (request["activation_library_index"], index_path),
                      (request["history_artifact"], checked_ref(request["history_artifact"],
                       "Rust-authored history", MAX_OUTPUT_BYTES)),
                      (request["source_schedules"], checked_ref(request["source_schedules"],
                       "component irradiation schedules", MAX_OUTPUT_BYTES)),
                      (request["response_location_artifacts"][request["event"]["location_id"]],
                       checked_ref(request["response_location_artifacts"][request["event"]["location_id"]],
                                   "task response location", 64 * 1024))):
        if sha256_file(path).lower() != ref["sha256"].lower():
            raise ValueError("a pinned input changed during the OpenMC neutron response")
    internal_record = {
        "schema": "faris-maintenance-709-neutron-response/v0.1", "status": "PASS",
        "request_sha256": request_sha256,
        "response_npz_sha256": response_ref["sha256"],
        "model_sha256": request["model"]["sha256"],
        "geometry_sha256": request["geometry"]["sha256"],
        "cross_sections_sha256": request["cross_sections"]["sha256"],
        "group_boundary_sha256": boundary_hash, "activation_mesh": request["activation_mesh"],
        "source": {**source_identity, "base_source_rate_n_s": request["base_source_rate_n_s"]},
        "solver": {"particles": request["sampling"]["neutron_particles"],
                   "batches": run["batches"], "histories": run["histories"],
                   "lost_particles": run["lost_particles"],
                   "lost_particle_fraction_limit": run["lost_particle_fraction_limit"],
                   "solver_status": run["solver_status"], "stop_reason": run["stop_reason"]},
        "runtime": {"openmc_python_version": openmc.__version__,
                    "openmc_c_library": run["c_api_library"]["path"],
                    "openmc_c_library_sha256": run["c_api_library"]["sha256"]},
    }
    internal_record_ref = _write_json_new(out / "709_neutron_response_record.json", internal_record)
    helper_request = {
        "schema": "faris-maintenance-709-response-request/v0.1",
        "model": request["model"], "geometry": request["geometry"],
        "component_material_map": request["component_material_map"], "response_npz": response_ref,
        "neutron_response_receipt": internal_record_ref,
        "cross_sections": request["cross_sections"],
        "activation_library_npz": request["activation_library_npz"],
        "activation_library_index": request["activation_library_index"],
        "activation_mesh": request["activation_mesh"], "source_groups": request["source_groups"],
        "event": {"shutdown_s": request["event"]["shutdown_s"], "cooling_s": request["event"]["cooling_s"]},
        "base_source_rate_n_s": request["base_source_rate_n_s"],
        "photon_energy_edges_ev": request["photon_energy_edges_ev"],
    }
    activation_batches = activation_batches_from_artifacts(helper_request)
    if activation_batches.get("schema") != ACTIVATION_BATCHES_SCHEMA:
        raise ValueError("ACTINV helper returned an unsupported event-batches schema")
    provenance = activation_batches.setdefault("provenance", {})
    provenance["history_sha256"] = request["history_sha256"]
    provenance["source_schedules_sha256"] = request["source_schedules"]["sha256"]
    out.mkdir(parents=True, exist_ok=True)
    batches_ref = _write_json_new(out / "actinv_event_batches.json", activation_batches)
    solver_model_ref = {"path": str(solver_model.resolve()), "sha256": sha256_file(solver_model)}
    receipt = {
        "schema": NEUTRON_RECEIPT_SCHEMA, "request_id": request["request_id"], "status": "PASS",
        "request_sha256": request_sha256,
        "response_npz": response_ref, "activation_batches": batches_ref,
        "neutron_response_record": internal_record_ref,
        "model": request["model"], "solver_model": solver_model_ref,
        "geometry": request["geometry"], "cross_sections": request["cross_sections"],
        "component_material_map": request["component_material_map"],
        "history_sha256": request["history_sha256"], "history_artifact": request["history_artifact"],
        "source_schedules": request["source_schedules"],
        "response_location_artifacts": request["response_location_artifacts"],
        "activation_library_npz": request["activation_library_npz"],
        "activation_library_index": request["activation_library_index"],
        "activation_mesh": request["activation_mesh"], "h5m_vertex_bounds": vertex_bounds,
        "event": request["event"], "source_groups": request["source_groups"],
        "group_boundary_sha256": boundary_hash,
        "source": {**source_identity, "base_source_rate_n_s": request["base_source_rate_n_s"]},
        "runtime": {"openmc_python_version": openmc.__version__,
                    "openmc_python_module": str(Path(openmc.__file__).resolve()),
                    "openmc_c_library": run["c_api_library"]["path"],
                    "openmc_c_library_sha256": run["c_api_library"]["sha256"]},
        "solver": {"particles": request["sampling"]["neutron_particles"],
                   "batches": run["batches"], "histories": run["histories"],
                   "lost_particles": run["lost_particles"],
                   "lost_particle_fraction_limit": run["lost_particle_fraction_limit"],
                   "solver_status": run["solver_status"], "stop_reason": run["stop_reason"],
                   "elapsed_s": run["elapsed_s"], "mesh_material_bins": len(elements),
                   "energy_groups": MAX_NEUTRON_GROUPS},
        "mesh_material_volume_sampling": {
            "samples": request["sampling"]["mat_vol_samples"], "max_materials_per_voxel": 4,
            "elapsed_s": volume_sampling_elapsed_s,
            "uncertainty": "mesh/material volume sampling uncertainty is not propagated"},
        "track_length_units": "integrated particle-cm per source neutron; divide by the matching material-in-voxel cm3 and multiply physical source rate once",
        "activation_group_setting": request["sampling"]["activation_groups"],
        "activation_group_basis": "the exact 710 tally edges are read from the hash-bound ACTINV NPZ bounds array and verified against its index; activation_groups is not used to define these edges",
        "uncertainty_scope": "Neutron transport batch uncertainty is not propagated through ACTINV; mesh/material volume sampling, nuclear-data, geometry and composition uncertainty are excluded.",
        "d1s": {"status": "NOT_EVALUATED",
                "reason": "ACTINV 709 response stage does not run OpenMC parent-nuclide D1S; no compatible common-history CE-chain run was supplied.",
                "next_step": "supply a separately checked common-history D1S result with chain, cross-section and source identities"},
        "limitations": [
            "FISPACT-709 boundaries and ACTINV index are hash-verified; reaction/channel completeness remains a nuclear-data limitation.",
            "One declared activation mesh and material-volume sample are used; spatial-source resolution and material-volume uncertainty are not propagated.",
            "Status PASS means this bounded OpenMC response stage completed; it is not physical validation.",
        ],
    }
    receipt_path = out / "709_neutron_receipt.json"
    _write_json_new(receipt_path, receipt)
    return receipt


def voxel_bounds(mesh: dict, element_index: int) -> tuple[list[float], list[float]]:
    """Map OpenMC's x-fastest mesh element index to physical axis-aligned voxel bounds."""
    lower, upper, dims, _ = validate_mesh(mesh)
    if isinstance(element_index, bool) or not isinstance(element_index, int) or not 0 <= element_index < math.prod(dims):
        raise ValueError("mesh element index is outside activation-mesh dimensions")
    nx, ny, _nz = dims
    ix = element_index % nx
    iy = (element_index // nx) % ny
    iz = element_index // (nx * ny)
    indices = (ix, iy, iz)
    cell_lo = [lower[a] + (upper[a] - lower[a]) * indices[a] / dims[a] for a in range(3)]
    cell_hi = [lower[a] + (upper[a] - lower[a]) * (indices[a] + 1) / dims[a] for a in range(3)]
    return cell_lo, cell_hi


def _emitters_from_results(request: dict) -> dict[float, list[dict]]:
    """Re-derive Rust emitter rows from hash-bound ActivationResults and reject substitutions."""
    result_refs = request.get("activation_results")
    if not isinstance(result_refs, list) or not result_refs:
        raise ValueError("stage 2 requires hashed ActivationResult artifacts for every active component")
    response_receipt_path = checked_ref(request["neutron_response_receipt"], "neutron response receipt", 16 * 1024 * 1024)
    response_receipt = json.loads(response_receipt_path.read_bytes())
    if response_receipt.get("schema") != NEUTRON_RECEIPT_SCHEMA or response_receipt.get("status") != "PASS":
        raise ValueError("stage-2 input is not a completed 709 neutron response receipt")
    if response_receipt.get("request_id") != request.get("request_id"):
        raise ValueError("Stage 1 and Stage 2 request IDs do not identify the same shutdown event")
    for key in ("model", "geometry", "cross_sections", "component_material_map"):
        if response_receipt.get(key) != request.get(key):
            raise ValueError(f"stage-2 {key} identity differs from the neutron response receipt")
    if response_receipt.get("activation_mesh") != request.get("activation_mesh"):
        raise ValueError("stage-2 activation mesh differs from the neutron response")
    source_event = response_receipt.get("event", {})
    for key in ("design", "class", "component", "replacement_k", "shutdown_s", "cooling_s",
                "location_id", "location_sha256"):
        if source_event.get(key) != request["event"].get(key):
            raise ValueError(f"stage-2 event {key} differs from the neutron response request")
    if response_receipt.get("source_groups") is None:
        raise ValueError("neutron response receipt omits the installed-component source schedules")
    if response_receipt.get("source", {}).get("unit_strength") != 1.0:
        raise ValueError("neutron response receipt does not record the unit-strength source kernel")
    if _finite(response_receipt.get("source", {}).get("base_source_rate_n_s"),
               "neutron source rate", 0.0) <= 0.0:
        raise ValueError("neutron response receipt has invalid base source normalization")
    response_path = checked_ref(response_receipt["response_npz"], "709-group response NPZ", MAX_OUTPUT_BYTES)
    response_arrays = _load_response_npz(response_path)
    count = len(response_arrays["element_indices"])
    if count < 1 or count > MAX_MATERIAL_BINS or any(
            response_arrays[key].shape != (count,) for key in
            ("material_ids", "material_volumes_cm3")) or \
            response_arrays["integrated_track_length_cm_per_source"].shape != (count, MAX_NEUTRON_GROUPS):
        raise ValueError("709 response arrays do not match the bounded bin×709-group contract")
    if response_arrays["element_indices"].dtype != np.dtype("int32") or \
            response_arrays["material_ids"].dtype != np.dtype("int32") or \
            response_arrays["material_volumes_cm3"].dtype != np.dtype("float64") or \
            response_arrays["integrated_track_length_cm_per_source"].dtype != np.dtype("float64"):
        raise ValueError("709 response arrays do not use the exact typed artifact contract")
    if not np.isfinite(response_arrays["material_volumes_cm3"]).all() or \
            np.any(response_arrays["material_volumes_cm3"] <= 0.0) or \
            not np.isfinite(response_arrays["integrated_track_length_cm_per_source"]).all() or \
            np.any(response_arrays["integrated_track_length_cm_per_source"] < 0.0):
        raise ValueError("709 response arrays contain invalid volume or track-length values")
    pairs = np.column_stack((response_arrays["element_indices"], response_arrays["material_ids"]))
    if np.any(response_arrays["element_indices"] < 0) or np.any(response_arrays["material_ids"] <= 0) or \
            len(np.unique(pairs, axis=0)) != count:
        raise ValueError("709 response contains invalid or duplicate mesh/material bins")
    map_path = checked_ref(request["component_material_map"], "component/material map")
    component_map = json.loads(map_path.read_bytes())
    materials_by_component = component_map.get("materials", {})
    event = request["event"]
    coolings = [_finite(x, "cooling time", 0.0) for x in event["cooling_s"]]
    if not coolings or any(b <= a for a, b in zip(coolings, coolings[1:])):
        raise ValueError("photon request cooling grid must be strictly ascending")
    if not isinstance(result_refs, list) or not result_refs:
        raise ValueError("activation results must bind one result per active component")
    expected_components = {ref.get("component") for ref in result_refs if isinstance(ref, dict)}
    receipt_components = {group.get("component") for group in response_receipt["source_groups"]
                          if isinstance(group, dict)}
    if not expected_components or expected_components != receipt_components:
        raise ValueError("ActivationResults must exactly cover the neutron-stage installed components")
    if set(materials_by_component).isdisjoint(expected_components):
        raise ValueError("photon request has no checked active component mapping")
    expected_zones = {}
    activation_mesh = request["activation_mesh"]
    _, _, mesh_dims, voxel_cm3 = validate_mesh(activation_mesh)
    voxel_volume_by_element = {}
    for element, volume in zip(response_arrays["element_indices"], response_arrays["material_volumes_cm3"]):
        if int(element) >= math.prod(mesh_dims):
            raise ValueError("709 response element index is outside activation mesh dimensions")
        voxel_volume_by_element[int(element)] = voxel_volume_by_element.get(int(element), 0.0) + float(volume)
    if any(value > voxel_cm3 * 1.00001 for value in voxel_volume_by_element.values()):
        raise ValueError("709 response material volumes exceed a voxel volume")
    if len(response_arrays["material_ids"]) != len(response_arrays["element_indices"]) or \
            len(response_arrays["material_volumes_cm3"]) != len(response_arrays["element_indices"]):
        raise ValueError("neutron response bin arrays do not have aligned lengths")
    for component in expected_components:
        ids = set(materials_by_component.get(component, []))
        if not ids:
            raise ValueError(f"active component {component} has no CAD material IDs")
        expected_zones[component] = {}
        for row, (element, material_id, volume) in enumerate(zip(response_arrays["element_indices"],
                                                                   response_arrays["material_ids"],
                                                                   response_arrays["material_volumes_cm3"])):
            element, material_id = int(element), int(material_id)
            if material_id in ids:
                zone_id = f"e{element}_m{material_id}"
                expected_zones[component][zone_id] = (element, material_id, float(volume), row)
    expected_by_cooling = {cooling: {} for cooling in coolings}
    seen_components = set()
    for ref in result_refs:
        if not isinstance(ref, dict) or set(ref) != {"component", "path", "sha256"}:
            raise ValueError("activation result ref must bind component, path and SHA-256")
        component = ref["component"]
        if component not in expected_components or component in seen_components:
            raise ValueError("activation results must include each active component exactly once")
        seen_components.add(component)
        path = checked_ref({"path": ref["path"], "sha256": ref["sha256"]}, "activation result")
        result = json.loads(path.read_bytes())
        if result.get("schema") != ACTIVATION_RESULT_SCHEMA:
            raise ValueError("activation result has an unsupported schema")
        expected_library = response_receipt["activation_library_npz"]["sha256"]
        if result.get("data", {}).get("library", {}).get("sha256", "").lower() != expected_library.lower():
            raise ValueError("ActivationResult library hash differs from the checked ACTINV response library")
        if result.get("cooling_s") != coolings:
            raise ValueError("ActivationResult cooling grid differs from the Rust event cooling grid")
        result_zones = result.get("zones", [])
        if not isinstance(result_zones, list):
            raise ValueError("ActivationResult zones must be a list")
        zones = {zone.get("id"): zone for zone in result_zones if isinstance(zone, dict)}
        if len(zones) != len(result_zones):
            raise ValueError("ActivationResult contains malformed or duplicate zones")
        if set(zones) != set(expected_zones[component]):
            raise ValueError(f"activation result zone IDs differ from the actual OpenMC material bins for {component}")
        for cooling in coolings:
            for zone_id, (element, material_id, material_volume, _row) in expected_zones[component].items():
                zone = zones[zone_id]
                if zone.get("completeness") != "COMPLETE" or zone.get("gaps"):
                    raise ValueError(f"activation zone {zone_id} is incomplete or has data gaps")
                input_volume_m3 = _finite(zone.get("input", {}).get("volume_m3"), "activation zone volume", 0.0)
                if not math.isclose(input_volume_m3, material_volume * 1.0e-6, rel_tol=1e-10, abs_tol=1e-18):
                    raise ValueError(f"activation zone {zone_id} volume differs from its OpenMC material bin")
                points = zone.get("points", [])
                if not isinstance(points, list) or len(points) != len(coolings) or \
                        [p.get("cooling_s") for p in points if isinstance(p, dict)] != coolings:
                    raise ValueError(f"activation zone {zone_id} does not provide the exact ordered cooling grid")
                point = points[coolings.index(cooling)]
                if point is None:
                    raise ValueError(f"activation zone {zone_id} misses cooling point {cooling}")
                source = point.get("photon_source") or {}
                total = _finite(source.get("total_photons_per_s"), "ACTINV total photons/s", 0.0)
                outside = _finite(source.get("photons_outside_groups_per_s"), "ACTINV outside-group photons/s", 0.0)
                groups = source.get("groups")
                if not isinstance(groups, list) or not groups:
                    if total == 0 and not groups:
                        continue
                    raise ValueError(f"activation zone {zone_id} has no photon group data")
                rates = [_finite(g.get("photons_per_s"), "ACTINV photon group photons/s", 0.0) for g in groups]
                grouped = math.fsum(rates)
                if outside > max(1e-12, total * 1e-12) or not math.isclose(grouped, total, rel_tol=1e-8, abs_tol=1e-12):
                    raise ValueError(f"ACTINV photon group rates do not conserve total for {zone_id}")
                if total <= 0:
                    continue
                low, high = voxel_bounds(activation_mesh, element)
                emit = {"component": component, "zone_id": zone_id, "mesh_element": element,
                        "material_id": material_id, "lower_left_cm": low, "upper_right_cm": high,
                        "photons_per_s": total,
                        "energy_groups": [{"energy_ev": _finite(g.get("centroid_ev"), "ACTINV centroid eV", 0.0),
                                           "photons_per_s": rate}
                                          for g, rate in zip(groups, rates) if rate > 0.0]}
                expected_by_cooling[cooling][zone_id] = emit
    if seen_components != expected_components:
        raise ValueError("activation result references do not cover every currently installed component")
    emitter_ref = request.get("emitters_by_cooling")
    emitter_path = checked_ref(emitter_ref, "ACTINV photon-emitter artifact", MAX_OUTPUT_BYTES)
    if emitter_path.stat().st_size > MAX_TOTAL_STAGE_BYTES:
        raise ValueError("ACTINV emitter JSON exceeds the bounded memory limit")
    emitter_payload = json.loads(emitter_path.read_bytes())
    if emitter_payload.get("schema") != "faris-maintenance-actinv-photon-emitters/v0.1":
        raise ValueError("ACTINV emitter artifact has an unsupported schema")
    supplied = emitter_payload.get("emitters_by_cooling")
    if not isinstance(supplied, list) or len(supplied) != len(coolings):
        raise ValueError("stage 2 requires emitter sets for every cooling point")
    validated = {}
    for item, cooling in zip(supplied, coolings):
        if not isinstance(item, dict) or item.get("cooling_s") != cooling or not isinstance(item.get("emitters"), list):
            raise ValueError("emitter-set cooling identity or list is invalid")
        source_strength = _finite(item.get("source_strength_photons_s"), "source strength photons/s", 0.0)
        given = {e.get("zone_id"): e for e in item["emitters"]}
        expected = expected_by_cooling[cooling]
        if not math.isclose(source_strength,
                            math.fsum(e["photons_per_s"] for e in expected.values()),
                            rel_tol=1e-10, abs_tol=1e-12):
            raise ValueError("emitter-set source strength does not reconcile to checked ActivationResults")
        if set(given) != set(expected):
            raise ValueError(f"emitter zone IDs differ from checked ACTINV results at cooling {cooling}")
        for zone_id, emission in expected.items():
            actual = given[zone_id]
            for key in ("component", "mesh_element", "material_id", "lower_left_cm", "upper_right_cm"):
                if actual.get(key) != emission[key]:
                    raise ValueError(f"emitter {zone_id} {key} differs from exact activation bin identity")
            if not math.isclose(_finite(actual.get("photons_per_s"), "emitter photons/s", 0.0),
                                emission["photons_per_s"], rel_tol=1e-10, abs_tol=1e-12):
                raise ValueError(f"emitter {zone_id} total photons/s differs from ActivationResult")
            if len(actual.get("energy_groups", [])) != len(emission["energy_groups"]):
                raise ValueError(f"emitter {zone_id} energy groups differ from ActivationResult")
            for left, right in zip(actual["energy_groups"], emission["energy_groups"]):
                if not math.isclose(_finite(left.get("energy_ev"), "emitter centroid eV", 0.0),
                                    right["energy_ev"], rel_tol=1e-12, abs_tol=1e-9) or \
                        not math.isclose(_finite(left.get("photons_per_s"), "emitter group photons/s", 0.0),
                                         right["photons_per_s"], rel_tol=1e-10, abs_tol=1e-12):
                    raise ValueError(f"emitter {zone_id} photon spectrum differs from ActivationResult")
        validated[cooling] = [expected[k] for k in sorted(expected)]
    return validated


def _dose_tally(openmc, response: dict):
    lower = response["lower_left_cm"]
    upper = response["upper_right_cm"]
    mesh = openmc.RegularMesh(name="faris-task-location-dose")
    mesh.lower_left = lower
    mesh.upper_right = upper
    mesh.dimension = response["dimension"]
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    tally = openmc.Tally(name="faris_task_location_effective_dose")
    tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                     openmc.EnergyFunctionFilter(energies, coefficients, interpolation="log-log")]
    tally.scores = ["flux"]
    return mesh, tally


def _photon_sources(openmc, emitters: list[dict], materials_by_id: dict):
    sources = []
    total_strength = 0.0
    for emitter in emitters:
        material_id = int(emitter["material_id"])
        material = materials_by_id.get(material_id)
        if material is None:
            raise ValueError(f"OpenMC model has no material object for CAD material {material_id}")
        groups = emitter["energy_groups"]
        energies = [_finite(item["energy_ev"], "ACTINV photon centroid energy", 0.0) for item in groups]
        rates = [_finite(item["photons_per_s"], "ACTINV photon rate", 0.0) for item in groups]
        strength = _finite(emitter["photons_per_s"], "ACTINV voxel photon rate", 0.0)
        if not energies or any(energy <= 0.0 for energy in energies) or any(rate <= 0.0 for rate in rates):
            raise ValueError("positive ACTINV source requires positive-energy positive-rate photon groups")
        grouped = math.fsum(rates)
        if not math.isclose(grouped, strength, rel_tol=1e-10, abs_tol=1e-12):
            raise ValueError("photon-source group rates do not reconcile to emitter strength")
        low, high = emitter["lower_left_cm"], emitter["upper_right_cm"]
        sources.append(openmc.IndependentSource(
            space=openmc.stats.Box(low, high),
            energy=openmc.stats.Discrete(energies, [rate / grouped for rate in rates]),
            particle="photon", strength=strength,
            constraints={"domains": [material], "rejection_strategy": "resample"}))
        total_strength += strength
        if not math.isfinite(total_strength):
            raise ValueError("combined ACTINV photon-source strength overflowed")
    if not sources or total_strength <= 0.0:
        raise ValueError("ACTINV photon stage has no positive whole-geometry source")
    return sources, total_strength


def run_photon_stage(request: dict, out: Path, openmc=None, request_sha256: str | None = None) -> dict:
    """Transport one absolute ACTINV photon source per checked cooling point."""
    _validate_stage_request(request, PHOTON_REQUEST_SCHEMA)
    if not isinstance(request_sha256, str) or len(request_sha256) != 64:
        raise ValueError("exact Stage 2 request-byte SHA-256 is required")
    try:
        bytes.fromhex(request_sha256)
    except ValueError as exc:
        raise ValueError("exact Stage 2 request-byte SHA-256 is invalid") from exc
    _emitters = _emitters_from_results(request)
    model_path, geometry_path, xs_path, component_map = _checked_model_paths(request)
    location = checked_ref(request["response_mesh"]["artifact"], "task response location", 64 * 1024)
    response_sha = sha256_file(location)
    if response_sha.lower() != request["event"]["location_sha256"].lower():
        raise ValueError("photon response location artifact changed after request validation")
    neutron_receipt = json.loads(checked_ref(request["neutron_response_receipt"], "neutron receipt",
                                             16 * 1024 * 1024).read_bytes())
    try:
        from maintenance_actinv_backend import validate_safe_model_sources
    except ImportError as exc:
        raise RuntimeError("checked maintenance model-source validator is required before photon transport") from exc
    validate_safe_model_sources(ET.parse(model_path).getroot())
    source_identity = source_kernel_identity(model_path)
    if source_identity["kernel_sha256"] != neutron_receipt.get("source", {}).get("kernel_sha256"):
        raise ValueError("hash-pinned primary neutron source kernel differs from the Stage 1 response")
    if openmc is None:
        import openmc
    lib_before = _library_identity(openmc)
    neutron_runtime = neutron_receipt.get("runtime", {})
    if openmc.__version__ != neutron_runtime.get("openmc_python_version") or \
            lib_before["path"] != neutron_runtime.get("openmc_c_library") or \
            lib_before["sha256"] != neutron_runtime.get("openmc_c_library_sha256"):
        raise ValueError("photon transport OpenMC runtime differs from the neutron response runtime")
    openmc.config["cross_sections"] = str(xs_path)
    model = openmc.Model.from_model_xml(str(model_path))
    material_objects = {int(material.id): material for material in model.materials}
    map_ids = {int(mid) for values in component_map["materials"].values() for mid in values}
    if not map_ids.issubset(material_objects):
        raise ValueError("checked CAD material map names an ID absent from OpenMC material definitions")
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.photon_transport = True
    settings.particles = request["sampling"]["photon_particles"]
    settings.batches = request["sampling"]["photon_batches"]
    settings.seed = request["sampling"]["seed"]
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.5
    settings.output = {"summary": False, "tallies": False}
    settings.statepoint = {"batches": []}
    settings.sourcepoint = {"write": False}
    response_mesh, tally = _dose_tally(openmc, request["response_mesh"])
    model.tallies = openmc.Tallies([tally])
    receipt_points = []
    out = Path(out).resolve()
    for point_index, cooling in enumerate(request["event"]["cooling_s"]):
        sources, expected_strength = _photon_sources(openmc, _emitters[cooling], material_objects)
        settings.source = sources
        settings.seed = min(2**31 - 1, request["sampling"]["seed"] + point_index)
        model.settings = settings
        model.tallies = openmc.Tallies([tally])
        model_dir = out / f"photon_cooling_{point_index:03d}"
        model_dir.mkdir(parents=True, exist_ok=False)
        model.export_to_model_xml(str(model_dir))
        solver_model = model_dir / "model.xml"
        if not solver_model.is_file():
            raise RuntimeError("OpenMC did not export the photon model")
        root = ET.parse(solver_model).getroot()
        if sum(float(node.attrib.get("strength", "0")) for node in root.findall("./settings/source")) <= 0:
            raise ValueError("serialized photon model has no positive absolute source strengths")
        run = run_live_tally(openmc, solver_model, tally.id,
                             request["sampling"]["photon_particles"], request["sampling"]["photon_batches"],
                             request["sampling"]["threads"], request["sampling"]["run_budget_s"])
        if run["stop_reason"] != "batch_limit" or run["solver_status"] != 1 or \
                run["batches"] != request["sampling"]["photon_batches"]:
            raise RuntimeError("photon response did not complete the configured OpenMC batch limit")
        batch_values = np.asarray(run["batch_values"], dtype=np.float64)
        if batch_values.shape != (run["batches"], 1) or not np.isfinite(batch_values).all():
            raise RuntimeError("photon response must retain one finite aggregate tally value per batch")
        samples = batch_values[:, 0]
        mean = math.fsum(float(x) for x in samples) / len(samples)
        variance = math.fsum((float(x) - mean) ** 2 for x in samples) / (len(samples) - 1)
        se = math.sqrt(variance / len(samples))
        region_cm3 = math.prod(request["response_mesh"]["upper_right_cm"][i] -
                               request["response_mesh"]["lower_left_cm"][i] for i in range(3))
        factor = 3600.0e-12 / region_cm3
        mean_sv_h, se_sv_h = mean * factor, se * factor
        if not math.isfinite(mean_sv_h) or not math.isfinite(se_sv_h):
            raise RuntimeError("photon dose result or sampling uncertainty is non-finite")
        receipt_points.append({
            "cooling_s": cooling, "mean_sv_h": mean_sv_h, "standard_error_sv_h": se_sv_h,
            "source_strength_photons_s": expected_strength, "batches": run["batches"],
            "particles_per_batch": request["sampling"]["photon_particles"],
            "histories": run["histories"], "lost_particles": run["lost_particles"],
            "lost_particle_fraction_limit": run["lost_particle_fraction_limit"],
            "solver_status": run["solver_status"], "stop_reason": run["stop_reason"],
            "seed": settings.seed, "model_xml": run["model_xml"], "c_api_library": run["c_api_library"],
            "elapsed_s": run["elapsed_s"],
        })
    if sha256_file(model_path) != request["model"]["sha256"] or \
            sha256_file(geometry_path) != request["geometry"]["sha256"] or \
            sha256_file(xs_path) != request["cross_sections"]["sha256"] or \
            _library_identity(openmc) != lib_before:
        raise ValueError("a pinned photon input or loaded OpenMC C API library changed during transport")
    unchanged_refs = [request["component_material_map"], request["neutron_response_receipt"],
                      request["emitters_by_cooling"], request["response_mesh"]["artifact"],
                      *({"path": ref["path"], "sha256": ref["sha256"]}
                        for ref in request["activation_results"])]
    for ref in unchanged_refs:
        checked_ref(ref, "photon stage source/provenance artifact", MAX_OUTPUT_BYTES)
    receipt = {
        "schema": PHOTON_RECEIPT_SCHEMA, "request_id": request["request_id"], "status": "PASS",
        "request_sha256": request_sha256,
        "model": request["model"], "geometry": request["geometry"],
        "cross_sections": request["cross_sections"], "component_material_map": request["component_material_map"],
        "neutron_response_receipt": request["neutron_response_receipt"],
        "activation_results": request["activation_results"], "emitters_by_cooling": request["emitters_by_cooling"],
        "event": request["event"], "response_mesh": request["response_mesh"],
        "source_normalization": "ACTINV group photons/s are absolute source strengths; no neutron or fusion rate is multiplied again.",
        "dose_quantity": "OpenMC AP effective dose coefficient times integrated photon flux, divided by scored volume; Sv/h",
        "source_spatial_model": "uniform source within each activation voxel, constrained to its exact cloned CAD material ID; group-centroid photon energies",
        "uncertainty_scope": "photon transport batch sampling conditional on the exact ACTINV source; excludes neutron response, activation, nuclear-data, material-volume, geometry and source-model uncertainty",
        "points": receipt_points,
        "limitations": ["One checked task-location voxel is scored.",
                        "ACTINV photon groups are sampled at group centroid energies.",
                        "Status PASS is execution status, not physical validation."],
    }
    out.mkdir(parents=True, exist_ok=True)
    _write_json_new(out / "709_photon_receipt.json", receipt)
    return receipt

