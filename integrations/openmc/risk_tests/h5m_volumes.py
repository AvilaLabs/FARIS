"""Faceted volume, centroid and material tag of every volume in a DAGMC h5m file.

Reads the MOAB HDF5 layout directly with h5py and numpy, with no OpenMC or
MOAB, so the result is independent of the transport code. A volume's faceted
volume is the signed sum of tetrahedra over its bounding surface triangles,
each surface counted with the sense (forward or reverse) the file records for
that volume. Lengths are those of the file (OpenMC uses cm).
"""
from __future__ import annotations

RANGE_BIT = 0x10  # MOAB set flag: contents stored as (start, count) pairs


def _tag(tags, name):
    group = tags[name]
    return group["id_list"][()], group["values"][()]


def facet_volumes(path: str) -> dict:
    """Return {volume_global_id: {faceted_volume, centroid, triangles, material_tag, handle}}."""
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
        children = sets["children"][()].astype(np.int64)
        tags = t["tags"]
        dim_ids, dim_values = _tag(tags, "GEOM_DIMENSION")
        gid_ids, gid_values = _tag(tags, "GLOBAL_ID")
        sense_ids, sense_values = _tag(tags, "GEOM_SENSE_2")
        name_ids, name_values = _tag(tags, "NAME")

    dimension = {int(h): int(v) for h, v in zip(dim_ids, dim_values)}
    global_id = {int(h): int(v) for h, v in zip(gid_ids, gid_values)}
    sense = {int(h): (int(v[0]), int(v[1])) for h, v in zip(sense_ids, sense_values)}
    names = {int(h): bytes(v).rstrip(b"\0").decode() for h, v in zip(name_ids, name_values)}

    def set_contents(index):
        start = 0 if index == 0 else int(rows[index - 1][0]) + 1
        end = int(rows[index][0]) + 1
        data = contents[start:end]
        if int(rows[index][3]) & RANGE_BIT:
            out = []
            for k in range(0, len(data), 2):
                out.extend(range(int(data[k]), int(data[k]) + int(data[k + 1])))
            return out
        return data.tolist()

    def set_children(index):
        start = 0 if index == 0 else int(rows[index - 1][1]) + 1
        end = int(rows[index][1]) + 1
        return children[start:end].tolist()

    # Per-surface signed volume contribution and first moment (forward-sense convention).
    surface_vol = {}
    surface_moment = {}
    surface_count = {}
    for index in range(len(rows)):
        handle = set_start + index
        if dimension.get(handle) != 2:
            continue
        handles = np.array(set_contents(index), dtype=np.int64)
        tris = handles[(handles >= tri_start) & (handles < tri_start + len(conn))] - tri_start  # sets also hold vertices
        p = coords[conn[tris] - node_start]
        cross = np.cross(p[:, 1], p[:, 2])
        tet = np.einsum("ij,ij->i", p[:, 0], cross) / 6.0
        surface_vol[handle] = float(tet.sum())
        centroids = p.sum(axis=1) / 4.0  # tetra centroid with the origin vertex: (a+b+c)/4
        surface_moment[handle] = (tet[:, None] * centroids).sum(axis=0)
        surface_count[handle] = len(tris)

    # Material tag of a volume: the group set (NAME 'mat:...') that has it as a child.
    material_of = {}
    for index in range(len(rows)):
        handle = set_start + index
        name = names.get(handle, "")
        if name.startswith("mat:"):
            for child in set_children(index):
                material_of[int(child)] = name[4:]

    result = {}
    for index in range(len(rows)):
        handle = set_start + index
        if dimension.get(handle) != 3:
            continue
        volume = 0.0
        moment = np.zeros(3)
        triangles = 0
        for child in set_children(index):
            child = int(child)
            if child not in sense:
                continue
            fwd, rev = sense[child]
            sign = 1.0 if fwd == handle else (-1.0 if rev == handle else 0.0)
            volume += sign * surface_vol[child]
            moment += sign * surface_moment[child]
            triangles += surface_count[child]
        result[global_id[handle]] = {
            "faceted_volume": volume,
            "centroid": (moment / volume).tolist() if volume else [0.0, 0.0, 0.0],
            "triangles": triangles,
            "material_tag": material_of.get(handle),
            "handle": handle,
        }
    return result
