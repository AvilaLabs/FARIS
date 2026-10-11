"""Shared helpers and rules for stage S0f (docs/notes/CAD_TRANSPORT_S0F.md).

`fill_invalid` is pure Python so test_s0f.py can check it without numpy or OpenMC; `fill_file` applies it to a
weight_windows.h5 with h5py, imported only there.
"""
from __future__ import annotations

import math
from pathlib import Path


def neighbours(index: int, dims: tuple[int, int, int], periodic_axis: int | None = None) -> list[int]:
    """Face neighbours of a flat mesh index, in OpenMC's order (axis 0 fastest). `periodic_axis` wraps (a cylinder's phi)."""
    nx, ny, nz = dims
    i, rest = index % nx, index // nx
    j, k = rest % ny, rest // ny
    out = []
    for axis, (c, n) in enumerate(((i, nx), (j, ny), (k, nz))):
        for step in (-1, 1):
            m = c + step
            if axis == periodic_axis:
                if n < 3 and m % n == c:
                    continue
                m %= n
            elif not 0 <= m < n:
                continue
            p = [i, j, k]
            p[axis] = m
            out.append(p[0] + nx * (p[1] + ny * p[2]))
    return sorted(set(out))


def fill_invalid(values: list[float], dims: tuple[int, int, int], periodic_axis: int | None = None) -> tuple[list[float], int]:
    """Give every bin without a window (bound <= 0) the geometric mean of its valid face neighbours, in sweeps.

    Each sweep uses only the bins valid at its start, so the result does not depend on the order of the bins. Sweeps
    repeat until no invalid bin is left. A group with no valid bin, or none invalid, is returned unchanged.
    Returns (values, number of bins filled).
    """
    if len(values) != dims[0] * dims[1] * dims[2]:
        raise ValueError("values do not match the mesh dimensions")
    out = list(values)
    if all(v > 0 for v in out) or not any(v > 0 for v in out):
        return out, 0
    filled = 0
    todo = [n for n, v in enumerate(out) if v <= 0]
    while todo:
        updates = {}
        for n in todo:
            logs = [math.log(out[m]) for m in neighbours(n, dims, periodic_axis) if out[m] > 0]
            if logs:
                updates[n] = math.exp(sum(logs) / len(logs))
        if not updates:  # an invalid region with no valid bin anywhere around it (cannot happen when one bin is valid)
            raise ValueError("invalid bins with no valid neighbour")
        for n, v in updates.items():
            out[n] = v
        filled += len(updates)
        todo = [n for n in todo if n not in updates]
    return out, filled


def fill_file(src: Path, dst: Path) -> dict:
    """Copy a weight_windows.h5 and fill its invalid bins in every energy group that has any valid bin.

    Upper bounds are set to the file's own upper/lower ratio times the filled lower bound. Returns a summary.
    """
    import shutil

    import h5py

    shutil.copyfile(src, dst)
    with h5py.File(dst, "r+") as f:
        meshes = f["meshes"]
        mesh = meshes[list(meshes)[0]]
        if "r_grid" in mesh:
            dims = (len(mesh["r_grid"]) - 1, len(mesh["phi_grid"]) - 1, len(mesh["z_grid"]) - 1)
            periodic = 1
        else:
            dims = tuple(int(x) for x in mesh["dimension"][()])
            periodic = None
        group = f["weight_windows"][list(f["weight_windows"])[0]]
        lower = group["lower_ww_bounds"][()]
        upper = group["upper_ww_bounds"][()]
        ratios = sorted(float(u / l) for u, l in zip(upper.ravel(), lower.ravel()) if l > 0)
        ratio = ratios[len(ratios) // 2]
        per_group = []
        for e in range(lower.shape[0]):
            values, n = fill_invalid([float(v) for v in lower[e]], dims, periodic)
            if n:
                lower[e] = values
                upper[e] = [v * ratio for v in values]
            per_group.append(n)
        group["lower_ww_bounds"][...] = lower
        group["upper_ww_bounds"][...] = upper
    return {"source": str(src), "filled": str(dst), "dims": list(dims), "periodic_axis": periodic, "ratio": ratio,
            "bins_filled_per_group": per_group}
