#!/usr/bin/env python3
"""Reader and writer for the FARIS facets format `faris-facets/v1`.

The format is documented in crates/faris-model/src/design/facets.rs and in
docs/notes/DESIGN_FILE_FORMAT.md (appendix). In short, one little-endian binary
file:

    magic "FARISFC1" | u32 version = 1 | u32 flags (bit 0: unimprinted per-solid meshes)
    u64 n_vertices | u64 n_triangles | u64 n_surfaces | u64 n_volumes
    vertices  f64 x3, metres
    triangles u32 x3 vertex indices, grouped by surface
    surfaces  u64 first_triangle | u64 triangle_count | u32 forward_volume | u32 reverse_volume

(volumes are numbered from 1; 0 is the implicit complement; a triangle's normal
points out of the forward volume), and one JSON manifest beside it.

This module also reads a DAGMC `h5m` file in MOAB's native HDF5 layout with
h5py alone (tags GEOM_DIMENSION, GLOBAL_ID, GEOM_SENSE_2, CATEGORY and NAME, set
contents and children). It never imports MOAB or OpenMC, so the geometry FARIS
checks does not pass through the code that will transport on it. Vertices are
kept by MOAB handle, never merged by position.

The writer and reader here use only the standard library, so the seeded-defect
generators run anywhere; reading an h5m needs h5py and numpy.
"""
from __future__ import annotations

import array
import hashlib
import json
import struct
import sys
from pathlib import Path

SCHEMA = "faris-facets/v1"
MAGIC = b"FARISFC1"
VERSION = 1
FLAG_UNIMPRINTED = 1
HEADER = struct.Struct("<8sIIQQQQ")
SURFACE = struct.Struct("<QQII")
HELPER_VERSION = "2"

if sys.byteorder != "little":  # the format is little-endian and arrays are written as they sit
    raise SystemExit("faris_facets: big-endian hosts are not supported")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def pack_facets(vertices, triangles, surfaces, n_volumes: int, flags: int = 0) -> bytes:
    """Serialise. `vertices` is a flat or nested sequence of floats (metres), `triangles` of
    ints, `surfaces` a list of (first_triangle, triangle_count, forward, reverse)."""
    flat_v = array.array("d", _flatten(vertices))
    flat_t = array.array("I", _flatten(triangles))
    if len(flat_v) % 3 or len(flat_t) % 3:
        raise ValueError("vertices and triangles must come in threes")
    out = bytearray(HEADER.pack(MAGIC, VERSION, flags, len(flat_v) // 3, len(flat_t) // 3, len(surfaces), n_volumes))
    out += flat_v.tobytes()
    out += flat_t.tobytes()
    for first, count, forward, reverse in surfaces:
        out += SURFACE.pack(first, count, forward, reverse)
    return bytes(out)


def _flatten(seq):
    # numpy arrays and nested lists alike
    if hasattr(seq, "ravel"):
        return seq.ravel().tolist()
    out = []
    for item in seq:
        if isinstance(item, (list, tuple)):
            out.extend(item)
        else:
            out.append(item)
    return out


def unpack_facets(data: bytes) -> dict:
    """Parse the binary into plain Python lists; raises ValueError on a malformed file."""
    if len(data) < HEADER.size:
        raise ValueError("shorter than the header")
    magic, version, flags, nv, nt, ns, nvol = HEADER.unpack_from(data)
    if magic != MAGIC or version != VERSION:
        raise ValueError("not a faris-facets v1 file")
    need = HEADER.size + 24 * nv + 12 * nt + SURFACE.size * ns
    if need != len(data):
        raise ValueError(f"counts need {need} bytes, file has {len(data)}")
    at = HEADER.size
    vertices = array.array("d")
    vertices.frombytes(data[at : at + 24 * nv])
    at += 24 * nv
    triangles = array.array("I")
    triangles.frombytes(data[at : at + 12 * nt])
    at += 12 * nt
    surfaces = [SURFACE.unpack_from(data, at + SURFACE.size * i) for i in range(ns)]
    return {
        "flags": flags,
        "n_volumes": nvol,
        "vertices": [list(vertices[3 * i : 3 * i + 3]) for i in range(nv)],
        "triangles": [list(triangles[3 * i : 3 * i + 3]) for i in range(nt)],
        "surfaces": [list(s) for s in surfaces],
    }


def make_manifest(binary: bytes, volumes, surfaces_global_ids, source_kind: str, source_sha256: str,
                  n_vertices: int, n_triangles: int, n_surfaces: int, flags: int,
                  implicit_complement_tag: str | None = None) -> dict:
    manifest = {
        "schema": SCHEMA,
        "unit": "m",
        "unimprinted": bool(flags & FLAG_UNIMPRINTED),
        "counts": {"vertices": n_vertices, "triangles": n_triangles, "surfaces": n_surfaces, "volumes": len(volumes)},
        "source": {"kind": source_kind, "sha256": source_sha256},
        "binary_sha256": sha256_bytes(binary),
        "helper_version": HELPER_VERSION,
        "volumes": volumes,
        "surfaces": [{"index": i, "global_id": g} for i, g in enumerate(surfaces_global_ids)],
    }
    if implicit_complement_tag is not None:
        manifest["implicit_complement_tag"] = implicit_complement_tag
    return manifest


def write_pair(base: Path, binary: bytes, manifest: dict) -> tuple[Path, Path]:
    """Write `<base>.facets.bin` and `<base>.facets.json`."""
    base = Path(base)
    bin_path = base.with_name(base.name + ".facets.bin")
    json_path = base.with_name(base.name + ".facets.json")
    bin_path.write_bytes(binary)
    json_path.write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")
    return bin_path, json_path


def volumes_from_surfaces(surfaces, n_volumes: int) -> list[int]:
    counts = [0] * n_volumes
    for _first, count, forward, reverse in surfaces:
        for v in (forward, reverse):
            if v:
                counts[v - 1] += count
    return counts


# ---------------------------------------------------------------------------
# DAGMC h5m, read natively.

RANGE_BIT = 0x10  # MOAB set flag: contents stored as (start, count) pairs


def read_h5m(path: Path, scale_to_m: float):
    """Read the MOAB layout. Returns (vertices Nx3 float64 in metres, triangles Mx3 uint32,
    surfaces [(first, count, fwd, rev)], volumes [manifest dicts], surface_global_ids, complement_tag).

    Triangles are grouped by surface in surface-set order, vertices are those the
    triangles use, in ascending MOAB handle order.
    """
    import h5py
    import numpy as np

    with h5py.File(path, "r") as f:
        t = f["tstt"]
        coords = t["nodes/coordinates"][()]
        node_start = int(t["nodes/coordinates"].attrs["start_id"])
        tri_group = t["elements/Tri3"]
        conn = tri_group["connectivity"][()].astype(np.int64)
        tri_start = int(tri_group["connectivity"].attrs["start_id"])
        sets = t["sets"]
        rows = sets["list"][()]
        set_start = int(sets["list"].attrs["start_id"])
        contents = sets["contents"][()].astype(np.int64)
        tags = t["tags"]

        def tag(name):
            group = tags[name]
            return group["id_list"][()], group["values"][()]

        dim_ids, dim_values = tag("GEOM_DIMENSION")
        gid_ids, gid_values = tag("GLOBAL_ID")
        sense_ids, sense_values = tag("GEOM_SENSE_2")
        name_ids, name_values = tag("NAME")

    dimension = {int(h): int(v) for h, v in zip(dim_ids, dim_values)}
    global_id = {int(h): int(v) for h, v in zip(gid_ids, gid_values)}
    sense = {int(h): (int(v[0]), int(v[1])) for h, v in zip(sense_ids, sense_values)}
    names = {int(h): bytes(v).rstrip(b"\0").decode() for h, v in zip(name_ids, name_values)}

    def set_contents(index):
        start = 0 if index == 0 else int(rows[index - 1][0]) + 1
        end = int(rows[index][0]) + 1
        data = contents[start:end]
        if int(rows[index][3]) & RANGE_BIT:
            return np.concatenate([np.arange(data[k], data[k] + data[k + 1]) for k in range(0, len(data), 2)]) if len(data) else data
        return data

    surface_handles, volume_handles = [], []
    for index in range(len(rows)):
        handle = set_start + index
        d = dimension.get(handle)
        if d == 2:
            surface_handles.append(handle)
        elif d == 3:
            volume_handles.append(handle)
    volume_index = {h: i + 1 for i, h in enumerate(volume_handles)}

    # A volume belongs to the group set (NAME "mat:...") that lists it among its contents.
    # DAGMC's implicit complement is named by a group ending in "_comp"; that group does not
    # give a volume its material.
    material_of = {}
    complement_tag = None
    for index in range(len(rows)):
        handle = set_start + index
        name = names.get(handle, "")
        if not name.startswith("mat:"):
            continue
        if name.endswith("_comp"):
            complement_tag = complement_tag or name
            continue
        for member in set_contents(index).tolist():
            if int(member) in volume_index:
                material_of[int(member)] = name

    tri_blocks, surfaces, surface_ids = [], [], []
    first = 0
    for handle in surface_handles:
        handles = set_contents(handle - set_start)
        tris = handles[(handles >= tri_start) & (handles < tri_start + len(conn))] - tri_start
        if len(tris) == 0:
            raise ValueError(f"surface set {handle} holds no triangles")
        tri_blocks.append(tris)
        fwd, rev = sense.get(handle, (0, 0))
        surfaces.append((first, int(len(tris)), volume_index.get(fwd, 0), volume_index.get(rev, 0)))
        surface_ids.append(global_id.get(handle, 0))
        first += len(tris)
    order = np.concatenate(tri_blocks)
    used_nodes = conn[order]  # absolute node handles, M x 3
    unique_nodes = np.unique(used_nodes)
    remap = np.searchsorted(unique_nodes, used_nodes)
    vertices = coords[unique_nodes - node_start] * scale_to_m
    triangles = remap.astype(np.uint32)

    volumes = []
    per_volume = volumes_from_surfaces(surfaces, len(volume_handles))
    for i, handle in enumerate(volume_handles):
        volumes.append({
            "index": i + 1,
            "global_id": global_id.get(handle, 0),
            "material_tag": material_of.get(handle),
            "triangles": per_volume[i],
        })
    return vertices, triangles, surfaces, volumes, surface_ids, complement_tag


def export_h5m(h5m: Path, out_base: Path, scale_to_m: float = 0.01) -> dict:
    """Write the facets pair for a DAGMC h5m (default: file in centimetres, as OpenMC uses)."""
    vertices, triangles, surfaces, volumes, surface_ids, complement = read_h5m(Path(h5m), scale_to_m)
    binary = pack_facets(vertices, triangles, surfaces, len(volumes), 0)
    manifest = make_manifest(binary, volumes, surface_ids, "h5m", sha256_file(Path(h5m)),
                             len(vertices), len(triangles), len(surfaces), 0, complement)
    bin_path, json_path = write_pair(Path(out_base), binary, manifest)
    return {"bin": str(bin_path), "json": str(json_path), "manifest": manifest}


def main(argv=None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="export a DAGMC h5m to faris-facets/v1")
    parser.add_argument("h5m", type=Path)
    parser.add_argument("--out-base", type=Path, required=True)
    parser.add_argument("--scale-to-m", type=float, default=0.01, help="metres per unit of the h5m (default 0.01: centimetres)")
    args = parser.parse_args(argv)
    result = export_h5m(args.h5m, args.out_base, args.scale_to_m)
    print(json.dumps({k: result[k] for k in ("bin", "json")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
