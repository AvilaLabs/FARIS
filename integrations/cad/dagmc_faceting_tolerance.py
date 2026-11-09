# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Repair the DagMC faceting-tolerance metadata without changing the mesh.

The installed DagMC reader asks MOAB for sparse double tag ``FACETING_TOL``
on root entity 0.  Some cad_to_dagmc versions instead write a dense
``FACETING_TOLERANCE`` tag containing a bounding-box heuristic.  This helper
copies the source and writes the declared mesher tolerance into the exact
MOAB sparse-root-tag representation.  It does not claim that the mesher
tolerance is a certified maximum geometric deviation.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil

TAG_NAME = "FACETING_TOL"
LEGACY_TAG_NAME = "FACETING_TOLERANCE"
SPARSE_CLASS = 1  # moab::MB_TAG_SPARSE, from MOAB Types.hpp
ROOT_HANDLE = 0


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _dataset_digest(dataset) -> str:
    import numpy as np

    data = np.asarray(dataset[()])
    header = json.dumps({"dtype": repr(data.dtype.descr) if data.dtype.fields else data.dtype.str,
                         "shape": list(data.shape)}, sort_keys=True, separators=(",", ":")).encode()
    if data.dtype.hasobject:
        payload = json.dumps(_canonical(data), sort_keys=True, separators=(",", ":")).encode()
    else:
        payload = np.ascontiguousarray(data).tobytes()
    digest = hashlib.sha256(header + b"\0" + payload)
    for name in sorted(dataset.attrs):
        digest.update(name.encode() + b"\0" + json.dumps(_canonical(dataset.attrs[name]), sort_keys=True,
                                                          separators=(",", ":")).encode())
    return digest.hexdigest()


def _canonical(value):
    import numpy as np

    if isinstance(value, np.ndarray):
        if value.dtype.hasobject:
            return {"kind": "object-array", "shape": list(value.shape),
                    "values": [_canonical(item) for item in value.reshape(-1)]}
        return {"kind": "array", "dtype": value.dtype.str, "shape": list(value.shape),
                "hex": np.ascontiguousarray(value).tobytes().hex()}
    if isinstance(value, np.void):
        return {"kind": "record", "dtype": repr(value.dtype.descr),
                "fields": {name: _canonical(value[name]) for name in value.dtype.names or ()}}
    if isinstance(value, (bytes, np.bytes_)):
        return {"kind": "bytes", "hex": bytes(value).hex()}
    if isinstance(value, (str, np.str_)):
        return {"kind": "text", "value": str(value)}
    if isinstance(value, (bool, np.bool_)):
        return {"kind": "bool", "value": bool(value)}
    if isinstance(value, (int, np.integer)):
        return {"kind": "int", "value": str(int(value))}
    if isinstance(value, (float, np.floating)):
        return {"kind": "float", "value": float(value).hex()}
    if isinstance(value, dict):
        return {"kind": "mapping", "items": [[_canonical(k), _canonical(v)]
                                                for k, v in sorted(value.items(), key=lambda pair: repr(pair[0]))]}
    if isinstance(value, (tuple, list)):
        return {"kind": "sequence", "items": [_canonical(item) for item in value]}
    if value is None:
        return {"kind": "none"}
    raise TypeError(f"unsupported HDF5 value for identity hash: {type(value).__name__}")


def _snapshot(handle, *, include_target: bool = False) -> dict[str, str]:
    import h5py

    result = {}

    def visit(name, obj):
        if isinstance(obj, h5py.Dataset):
            if not include_target and (name.startswith(f"tstt/tags/{TAG_NAME}/") or
                                       name.startswith(f"tstt/tags/{LEGACY_TAG_NAME}/")):
                return
            result[name] = _dataset_digest(obj)

    handle.visititems(visit)
    return result


def _validate_source(path: Path) -> tuple[dict[str, str], dict]:
    import h5py

    with h5py.File(path, "r") as handle:
        if "tstt/tags" not in handle:
            raise ValueError("not a DAGMC H5M: missing tstt/tags")
        tags = handle["tstt/tags"]
        if TAG_NAME in tags:
            raise ValueError(f"source already has {TAG_NAME}; refusing to overwrite existing metadata")
        if LEGACY_TAG_NAME not in tags:
            raise ValueError(f"source lacks expected producer tag {LEGACY_TAG_NAME}")
        legacy = tags[LEGACY_TAG_NAME]
        if int(legacy.attrs.get("class", -1)) != 2:
            raise ValueError("legacy FACETING_TOLERANCE tag class differs from expected producer output")
        if "id_list" not in legacy or "values" not in legacy:
            raise ValueError("legacy FACETING_TOLERANCE root metadata is incomplete")
        if list(map(int, legacy["id_list"][()])) != [ROOT_HANDLE]:
            raise ValueError("legacy FACETING_TOLERANCE is not attached only to MOAB root entity 0")
        return _snapshot(handle), {
            "tag_name": LEGACY_TAG_NAME,
            "tag_class": int(legacy.attrs["class"]),
            "entity_handles": list(map(int, legacy["id_list"][()])),
            "values": list(map(float, legacy["values"][()])),
            "value_dtype": str(legacy["values"].dtype),
        }


def correct_faceting_tolerance(source: Path, destination: Path, declared_tolerance_cm: float) -> dict:
    """Create an audited copy with DagMC's exact sparse root tag.

    The source remains untouched.  Destination creation is exclusive.  The
    returned geometry hash map covers every HDF5 dataset other than the two
    corrected root-tag payload datasets; all other datasets must match.
    """
    import h5py
    import numpy as np

    source, destination = Path(source), Path(destination)
    tolerance = float(declared_tolerance_cm)
    if not np.isfinite(tolerance) or tolerance <= 0:
        raise ValueError("declared mesher tolerance must be finite and positive")
    if not source.is_file():
        raise FileNotFoundError(source)
    if source.resolve() == destination.resolve():
        raise ValueError("metadata correction requires a distinct destination")
    if os.path.lexists(destination):
        raise FileExistsError(f"destination already exists: {destination}")
    before_datasets, legacy_metadata = _validate_source(source)
    source_hash = file_sha256(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as src, destination.open("xb") as dst:
        shutil.copyfileobj(src, dst)

    try:
        with h5py.File(destination, "r+") as handle:
            tags = handle["tstt/tags"]
            del tags[LEGACY_TAG_NAME]
            tag = tags.create_group(TAG_NAME)
            tag["type"] = np.dtype("f8")
            tag.attrs.create("class", SPARSE_CLASS, dtype=np.int32)
            tag.create_dataset("id_list", data=np.asarray([ROOT_HANDLE], dtype=np.uint64))
            tag.create_dataset("values", data=np.asarray([tolerance], dtype=np.float64))
        with h5py.File(destination, "r") as handle:
            after_datasets = _snapshot(handle)
            tag = handle[f"tstt/tags/{TAG_NAME}"]
            if int(tag.attrs["class"]) != SPARSE_CLASS or tag["type"].dtype != np.dtype("f8"):
                raise RuntimeError("corrected tag does not have MOAB sparse-double schema")
            if list(map(int, tag["id_list"][()])) != [ROOT_HANDLE]:
                raise RuntimeError("corrected tag is not attached to MOAB root entity 0")
            if tag["values"].dtype != np.dtype("f8") or tag["values"].shape != (1,):
                raise RuntimeError("corrected root value is not one float64")
            if float(tag["values"][0]) != tolerance:
                raise RuntimeError("corrected tag value differs from declared mesher tolerance")
            if LEGACY_TAG_NAME in handle["tstt/tags"]:
                raise RuntimeError("legacy FACETING_TOLERANCE tag remains in corrected copy")
        if before_datasets != after_datasets:
            raise RuntimeError("metadata correction changed an H5M dataset outside faceting tolerance")
    except Exception:
        destination.unlink(missing_ok=True)
        raise

    return {
        "source_sha256": source_hash,
        "corrected_sha256": file_sha256(destination),
        "tag_name": TAG_NAME,
        "tag_class": "MB_TAG_SPARSE (1)",
        "entity_handle": ROOT_HANDLE,
        "declared_mesher_tolerance_cm": tolerance,
        "legacy_tag_removed": LEGACY_TAG_NAME,
        "legacy_metadata_before": legacy_metadata,
        "immutable_dataset_sha256": before_datasets,
        "geometry_identity_sha256": hashlib.sha256(json.dumps(before_datasets, sort_keys=True,
                                                               separators=(",", ":")).encode()).hexdigest(),
        "source_unchanged": file_sha256(source) == source_hash,
        "interpretation": "declared mesher input tolerance; not a certified maximum geometric deviation",
    }
