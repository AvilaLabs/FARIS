# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Copy a DAGMC H5M and retag only its explicit-void and implicit-complement NAME groups.

This does not remesh geometry. It is deliberately strict: the input must contain
one or more disjoint ``mat:void`` groups whose union is exactly the
caller-declared void volume IDs, and one ``mat:void_comp`` marker. All HDF5
datasets other than NAME/values must be byte-for-byte semantically identical
in the output.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil

RANGE_BIT = 0x10
RENAME = {"mat:void": "mat:filler", "mat:void_comp": "mat:filler_comp"}
IMMUTABLE_DATASETS = (
    "tstt/nodes/coordinates", "tstt/elements/Tri3/connectivity",
    "tstt/sets/list", "tstt/sets/contents", "tstt/sets/children", "tstt/sets/parents",
    "tstt/tags/GEOM_DIMENSION/id_list", "tstt/tags/GEOM_DIMENSION/values",
    "tstt/tags/GEOM_SENSE_2/id_list", "tstt/tags/GEOM_SENSE_2/values",
    "tstt/tags/GLOBAL_ID/id_list", "tstt/tags/GLOBAL_ID/values",
)


def _dtype_identity(dtype) -> str:
    return repr(dtype.descr) if dtype.fields is not None else dtype.str


def _canonical_value(value):
    import numpy as np

    if isinstance(value, np.ndarray):
        dtype = value.dtype
        if dtype.hasobject:
            body = [_canonical_value(item) for item in value.reshape(-1)]
            return {"kind": "array-object", "shape": list(value.shape), "dtype": _dtype_identity(dtype), "values": body}
        return {"kind": "array-bytes", "shape": list(value.shape), "dtype": _dtype_identity(dtype),
                "bytes_hex": np.ascontiguousarray(value).tobytes().hex()}
    if isinstance(value, np.void):
        dtype = value.dtype
        if dtype.hasobject:
            return {"kind": "record-object", "dtype": _dtype_identity(dtype),
                    "fields": {name: _canonical_value(value[name]) for name in dtype.names or ()}}
        return {"kind": "record-bytes", "dtype": _dtype_identity(dtype), "bytes_hex": value.tobytes().hex()}
    if isinstance(value, (bytes, np.bytes_)):
        return {"kind": "bytes", "bytes_hex": bytes(value).hex()}
    if isinstance(value, (str, np.str_)):
        return {"kind": "text", "value": str(value)}
    if isinstance(value, (bool, np.bool_)):
        return {"kind": "bool", "value": bool(value)}
    if isinstance(value, (int, np.integer)):
        return {"kind": "integer", "value": str(int(value))}
    if isinstance(value, (float, np.floating)):
        return {"kind": "float", "hex": float(value).hex()}
    if isinstance(value, dict):
        return {"kind": "mapping", "items": [[_canonical_value(key), _canonical_value(item)]
                                                for key, item in sorted(value.items(), key=lambda pair: repr(pair[0]))]}
    if isinstance(value, (tuple, list)):
        return {"kind": "sequence", "items": [_canonical_value(item) for item in value]}
    if value is None:
        return {"kind": "none"}
    raise TypeError(f"unsupported HDF5 value type for identity hashing: {type(value).__name__}")


def _canonical_dataset_payload(dataset) -> bytes:
    import numpy as np

    array = np.asarray(dataset[()])
    if array.dtype.hasobject:
        payload = json.dumps(_canonical_value(array), sort_keys=True, separators=(",", ":")).encode()
    else:
        payload = np.ascontiguousarray(array).tobytes()
    header = json.dumps({"shape": list(array.shape), "dtype": _dtype_identity(array.dtype)}, sort_keys=True,
                        separators=(",", ":")).encode()
    return header + b"\0" + payload


def _dataset_hash(dataset) -> str:
    digest = hashlib.sha256(_canonical_dataset_payload(dataset))
    for name in sorted(dataset.attrs):
        value = dataset.attrs[name]
        digest.update(name.encode() + b"\0" + json.dumps(_canonical_value(value), sort_keys=True,
                                                             separators=(",", ":")).encode())
    return digest.hexdigest()


def _set_contents(rows, contents, row_index: int) -> list[int]:
    start = 0 if row_index == 0 else int(rows[row_index - 1][0]) + 1
    end = int(rows[row_index][0]) + 1
    values = contents[start:end]
    if int(rows[row_index][3]) & RANGE_BIT:
        if len(values) % 2:
            raise ValueError("malformed MOAB range-encoded set contents")
        expanded = []
        for index in range(0, len(values), 2):
            first, count = int(values[index]), int(values[index + 1])
            if count < 0:
                raise ValueError("negative MOAB set range length")
            expanded.extend(range(first, first + count))
        return expanded
    return [int(value) for value in values]


def _inspect(path: Path, expected_void_volume_ids: set[int], *, retagged: bool = False) -> tuple[dict, list[dict]]:
    import h5py

    with h5py.File(path, "r") as handle:
        tstt = handle["tstt"]
        sets = tstt["sets"]
        rows = sets["list"][()]
        contents = sets["contents"][()]
        set_start = int(sets["list"].attrs["start_id"])
        name_ids = tstt["tags/NAME/id_list"][()]
        name_values = tstt["tags/NAME/values"][()]
        dimension = {int(entity): int(dim) for entity, dim in zip(
            tstt["tags/GEOM_DIMENSION/id_list"][()], tstt["tags/GEOM_DIMENSION/values"][()])}
        global_ids = {int(entity): int(gid) for entity, gid in zip(
            tstt["tags/GLOBAL_ID/id_list"][()], tstt["tags/GLOBAL_ID/values"][()])}
        if len(name_ids) != len(set(int(value) for value in name_ids)):
            raise ValueError("H5M NAME set IDs are not unique")
        if len(rows) + set_start <= max(name_ids, default=set_start - 1):
            raise ValueError("H5M NAME set IDs do not align with the MOAB set list")
        names = [bytes(value).rstrip(b"\0").decode("utf-8") for value in name_values]
        expected_names = {old: (new if retagged else old) for old, new in RENAME.items()}
        matches = {old: [i for i, value in enumerate(names) if value == expected_names[old]] for old in RENAME}
        if not matches["mat:void"] or len(matches["mat:void_comp"]) != 1:
            raise ValueError("H5M must contain mat:void groups and exactly one mat:void_comp NAME group")
        tags = []
        for old, indexes in matches.items():
            for index in indexes:
                set_handle = int(name_ids[index])
                row_index = set_handle - set_start
                if not (0 <= row_index < len(rows)):
                    raise ValueError(f"{old} NAME group is not an H5M set")
                member_handles = _set_contents(rows, contents, row_index)
                members = sorted({global_ids[entity] for entity in member_handles
                                  if dimension.get(entity) == 3 and entity in global_ids})
                if not members or any(dimension.get(entity) != 3 for entity in member_handles):
                    raise ValueError(f"{old} must contain only mapped volume entities")
                tags.append({"set_handle": set_handle, "from": old, "to": RENAME[old],
                             "volume_global_ids": members})
        explicit = [item for item in tags if item["from"] == "mat:void"]
        explicit_ids = [gid for item in explicit for gid in item["volume_global_ids"]]
        if len(explicit_ids) != len(set(explicit_ids)):
            raise ValueError("mat:void groups must have disjoint volume membership")
        if set(explicit_ids) != expected_void_volume_ids:
            raise ValueError("mat:void volume IDs do not match the explicitly declared void roles")
        if "tstt/tags/NAME/values" not in handle:
            raise ValueError("H5M NAME value table is missing")
        immutable = {name: _dataset_hash(handle[name]) for name in IMMUTABLE_DATASETS}
        all_immutable = {}

        def collect(name, obj):
            if isinstance(obj, h5py.Dataset) and name != "tstt/tags/NAME/values":
                all_immutable[name] = _dataset_hash(obj)

        handle.visititems(collect)
        return {"dataset_hashes": immutable, "immutable_dataset_hashes": all_immutable,
                "name_ids": name_ids.copy(), "names": names}, tags


def retag_void_h5m(source: Path, destination: Path, expected_void_volume_ids) -> dict:
    """Copy ``source`` and rename only exact explicit/complement void material group tags.

    ``expected_void_volume_ids`` are DAGMC GLOBAL_IDs from the verified role map.
    The source is opened read-only and is never modified.
    """
    import h5py
    import numpy as np

    source, destination = Path(source), Path(destination)
    if source.resolve() == destination.resolve():
        raise ValueError("DAGMC retagging requires a distinct destination file")
    if not source.is_file():
        raise FileNotFoundError(source)
    if os.path.lexists(destination):
        raise FileExistsError(f"destination already exists: {destination}")
    expected = {int(value) for value in expected_void_volume_ids}
    if not expected:
        raise ValueError("at least one expected explicit-void volume ID is required")
    before, changed = _inspect(source, expected)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as src, destination.open("xb") as dst:
        shutil.copyfileobj(src, dst)

    with h5py.File(destination, "r+") as handle:
        values = handle["tstt/tags/NAME/values"]
        width = values.dtype.itemsize
        for index, old in enumerate(before["names"]):
            new = RENAME.get(old)
            if new is not None:
                encoded = new.encode("utf-8")
                if len(encoded) > width:
                    raise ValueError("replacement material tag exceeds H5M NAME storage width")
                values[index] = np.bytes_(encoded)

    after, actual_changes = _inspect(destination, expected, retagged=True)
    if before["name_ids"].tolist() != after["name_ids"].tolist():
        raise RuntimeError("retagging unexpectedly changed H5M NAME set IDs")
    expected_names = [RENAME.get(name, name) for name in before["names"]]
    if after["names"] != expected_names:
        raise RuntimeError("retagging changed NAME entries outside the declared void groups")
    if before["immutable_dataset_hashes"] != after["immutable_dataset_hashes"]:
        raise RuntimeError("retagging changed an H5M dataset other than NAME/values")
    if changed != actual_changes:
        raise RuntimeError("retagging changed void-group membership or volume IDs")
    return {"source_sha256": _file_sha256(source), "destination_sha256": _file_sha256(destination),
            "changed_material_groups": changed, "immutable_dataset_hashes": before["immutable_dataset_hashes"],
            "geometry_dataset_hashes": before["dataset_hashes"],
            "name_values_before_sha256": _bytes_sha256("\n".join(before["names"]).encode()),
            "name_values_after_sha256": _bytes_sha256("\n".join(after["names"]).encode())}


def _bytes_sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()
