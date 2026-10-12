"""S0f fact 12: lower the weight windows where a voxel sees an aperture, to its line-of-sight importance.

The deterministic adjoint under-rates chamber-facing walls for responses behind a port: a wall neutron reaches the
duct across void, which the coarse adjoint barely represents. Roulette then inflates wall neutrons to weights of
hundreds, and an aimed or streaming survivor lands on the coil as a spike (facts 8, 11).

For each voxel v on the visible side of an aperture rectangle and each energy group g, the uncollided route alone gives
an importance of at least

    I_los(v, g) = (omega(v) / 4 pi) * exp(-sigma * L(v)) * I_ap(g),

where omega is the rectangle's solid angle from the voxel centre, exp(-sigma * L) the uncollided transmission averaged
over rays to a 3 x 3 grid of points on the rectangle (L the material path along each, sigma a nominal total
cross-section), and I_ap(g) = 1 / w_ap(g) the importance of a particle
crossing the rectangle (the lowest lower bound just beyond it). The lower bound becomes min(w, 1 / I_los), and the
upper bound keeps its ratio to the lower. Windows only steer effort, so this cannot bias a tally.

    python s0f_los_windows.py BASE_MODEL_XML OUT_MODEL_XML [--sigma 0.15]
"""
import argparse
import math
import re
import tempfile
from pathlib import Path

import numpy as np

import s0f_common as F


def parse_rects(spec: str) -> list[tuple[np.ndarray, np.ndarray, np.ndarray]]:
    """Rectangles (c, u, v) from an OPENMC_APERTURE_BIAS value without its beta."""
    out = []
    for part in spec.split(";"):
        x = [float(t) for t in part.split()]
        if len(x) != 9:
            raise ValueError("each rectangle needs 9 numbers")
        out.append((np.array(x[0:3]), np.array(x[3:6]), np.array(x[6:9])))
    return out


def solid_angle(points: np.ndarray, c: np.ndarray, u: np.ndarray, v: np.ndarray) -> np.ndarray:
    """Exact solid angle of the rectangle c + s u + t v (|s|, |t| <= 1, u perpendicular to v) from each point on its
    back side (opposite the normal u x v); 0 on the front side."""
    n = np.cross(u, v)
    n = n / np.linalg.norm(n)
    eu, ev = u / np.linalg.norm(u), v / np.linalg.norm(v)
    a, b = np.linalg.norm(u), np.linalg.norm(v)
    rel = c - points
    h = rel @ n                                     # distance to the plane, positive on the back side
    x0, y0 = -(rel @ eu), -(rel @ ev)               # point's in-plane offset from the centre

    def corner(x, y, z):
        # solid angle of the rectangle [0, x] x [0, y] seen from height z above its corner (signed in x and y)
        return np.arctan2(x * y, z * np.sqrt(x * x + y * y + z * z))

    out = np.zeros(len(points))
    m = h > 1e-9
    xs, ys, hm = x0[m], y0[m], h[m]
    om = (corner(a - xs, b - ys, hm) - corner(-a - xs, b - ys, hm) - corner(a - xs, -b - ys, hm)
          + corner(-a - xs, -b - ys, hm))
    out[m] = np.abs(om)
    return out


def flat_index(ix, iy, iz, ie, dims):
    """Index into OpenMC's flattened bounds (Fortran order over (x, y, z, energy))."""
    nx, ny, nz = dims
    return ix + nx * (iy + ny * (iz + nz * ie))


def voxel_centres(lower_left, width, dims) -> np.ndarray:
    nx, ny, nz = dims
    ix, iy, iz = np.meshgrid(np.arange(nx), np.arange(ny), np.arange(nz), indexing="ij")
    c = np.stack([ix.ravel("F"), iy.ravel("F"), iz.ravel("F")], axis=1).astype(float)
    return np.asarray(lower_left) + (c + 0.5) * np.asarray(width)


def aperture_bound(lower: np.ndarray, rect, lower_left, width, dims, n_e: int) -> np.ndarray:
    """Per group, the lowest positive lower bound over voxels just beyond the rectangle (within one voxel of its plane,
    over its footprint)."""
    c, u, v = rect
    n = np.cross(u, v)
    n = n / np.linalg.norm(n)
    pts = []
    for s in np.linspace(-1, 1, 9):
        for t in np.linspace(-1, 1, 9):
            for d in (1.0, 0.5 * max(width)):
                pts.append(c + s * u + t * v + d * n)
    idx = set()
    for p in pts:
        i = np.floor((p - np.asarray(lower_left)) / np.asarray(width)).astype(int)
        if np.all(i >= 0) and np.all(i < np.asarray(dims)):
            idx.add(tuple(i))
    lo = lower.reshape(n_e, -1)
    out = np.full(n_e, np.inf)
    for (ix, iy, iz) in idx:
        k = flat_index(ix, iy, iz, 0, dims)
        col = lo[:, k]
        good = col > 0
        out[good] = np.minimum(out[good], col[good])
    return out


def material_path(points: np.ndarray, target: np.ndarray, find_material, step: float = 5.0,
                  stop_after: float = 40.0) -> np.ndarray:
    """Material path length (cm) from each point toward target, marching in steps; stops once past stop_after."""
    out = np.zeros(len(points))
    for k, p in enumerate(points):
        d = target - p
        L = float(np.linalg.norm(d))
        if L <= 0:
            continue
        e = d / L
        acc, t = 0.0, 0.5 * step
        while t < L and acc <= stop_after:
            q = p + e * t
            try:
                mat = find_material(tuple(q))
            except Exception:
                mat = None
            if mat is not None:
                acc += step
            t += step
        out[k] = acc
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("base", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--sigma", type=float, default=0.15, help="nominal total cross-section, 1/cm")
    args = ap.parse_args()
    x = args.base.read_text()
    j = x.index('<mesh id="9000"')
    mesh = x[j:x.index("</mesh>", j)]
    get = lambda tag, s: [float(t) for t in re.search(rf"<{tag}>(.*?)</{tag}>", s, re.S).group(1).split()]
    dims = [int(v) for v in get("dimension", mesh)]
    lower_left, width = get("lower_left", mesh), get("width", mesh)
    i = x.index('<weight_windows id="1">')
    k = x.index("</weight_windows>", i)
    ww = x[i:k]
    n_e = len(get("energy_bounds", ww)) - 1
    lower = np.array(get("lower_ww_bounds", ww))
    upper = np.array(get("upper_ww_bounds", ww))
    n_vox = int(np.prod(dims))
    assert lower.size == n_vox * n_e, (lower.size, n_vox, n_e)

    rects = parse_rects(F.APERTURE.split(None, 1)[1] + "; " + F.DUCT_INLET)
    centres = voxel_centres(lower_left, width, dims)
    lo2 = lower.reshape(n_e, n_vox)
    has = (lo2 > 0).any(axis=0)

    # Geometry for the material path: load the model through openmc.lib without transport.
    import openmc.lib as L
    with tempfile.TemporaryDirectory(dir=Path.home() / ".cache/avila-night") as tmp:
        (Path(tmp) / "model.xml").write_text(x)
        import os
        cwd = os.getcwd()
        os.chdir(tmp)
        try:
            L.init(["-s", "1"])
            best = np.full((n_e, n_vox), np.inf)   # capped lower bound per group and voxel
            for rect in rects:
                w_ap = aperture_bound(lower, rect, lower_left, width, dims, n_e)
                om = solid_angle(centres, *rect)
                # Only voxels that full transmission could lower: omega / 4 pi > w_ap / (largest lower bound).
                w_min = np.min(w_ap[np.isfinite(w_ap)])
                lo_max = np.where(lo2 > 0, lo2, 0.0).max(axis=0)
                cand = np.where(has & (om > 0) & (om / (4 * math.pi) * lo_max > w_min))[0]
                c, u, v = rect
                trans = np.zeros(len(cand))
                grid = [(s_, t_) for s_ in (-0.8, 0.0, 0.8) for t_ in (-0.8, 0.0, 0.8)]
                for s_, t_ in grid:
                    path = material_path(centres[cand], c + s_ * u + t_ * v, L.find_material)
                    trans += np.exp(-args.sigma * path) / len(grid)
                p_los = om[cand] / (4 * math.pi) * trans
                cap = w_ap[:, None] / np.maximum(p_los[None, :], 1e-300)
                best[:, cand] = np.minimum(best[:, cand], cap)
                print(f"rectangle at {rect[0]}: {len(cand)} voxels, w_ap range "
                      f"{np.nanmin(w_ap[np.isfinite(w_ap)]):.3g}-{np.nanmax(w_ap[np.isfinite(w_ap)]):.3g}")
            L.finalize()
        finally:
            os.chdir(cwd)

    new_lo = lo2.copy()
    m = (lo2 > 0) & (best < lo2)
    ratio = np.where(lo2 > 0, upper.reshape(n_e, n_vox) / np.where(lo2 > 0, lo2, 1.0), 0.0)
    new_lo[m] = best[m]
    new_up = upper.reshape(n_e, n_vox).copy()
    new_up[m] = new_lo[m] * ratio[m]
    print(f"lowered {int(m.sum())} of {int((lo2 > 0).sum())} positive bounds; median factor "
          f"{float(np.median(lo2[m] / new_lo[m])) if m.any() else 1.0:.3g}")
    fmt = lambda a: " ".join(repr(float(v)) for v in a.ravel())
    ww2 = re.sub(r"<lower_ww_bounds>.*?</lower_ww_bounds>", f"<lower_ww_bounds>{fmt(new_lo)}</lower_ww_bounds>", ww,
                 flags=re.S)
    ww2 = re.sub(r"<upper_ww_bounds>.*?</upper_ww_bounds>", f"<upper_ww_bounds>{fmt(new_up)}</upper_ww_bounds>", ww2,
                 flags=re.S)
    args.out.write_text(x[:i] + ww2 + x[k:])
    print(args.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
