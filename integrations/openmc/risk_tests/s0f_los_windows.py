"""S0f facts 12-13: cap the weight windows at their line-of-sight importance to the response and the apertures.

The deterministic adjoint under-rates regions that reach the response across void. Roulette then inflates neutrons
there to weights of hundreds, and an aimed or streaming survivor lands on the coil as a spike (facts 8, 11, 12).
Nested DXTRAN needs the windows to deliver particles near the outermost target "with appropriate weight"
(CAD_TRANSPORT_STREAMING_LITERATURE.md); this makes them do so.

A particle of group g in voxel v, emitted isotropically, crosses a target rectangle uncollided with probability
P(v, g). Its importance is then at least P(v, g) / w_t(g), where w_t is the target's own lower bound, so the voxel's
lower bound is capped at w_t(g) / P(v, g); it is never raised, and upper bounds keep their ratio. Windows only steer
effort, so this cannot bias a tally.

P comes from reciprocity: an analog run with a cosine-law surface source on the target, emitting backward, gives an
uncollided flux phi(v, g) per source neutron, and P = phi * A / 4 (A the target's area). That uses the real geometry
and cross-sections. Targets are processed from the response outward, each taking w_t from the bounds as already
capped, so the chain response -> exit -> inlet -> walls is consistent.

    python s0f_los_windows.py BASE_MODEL_XML OUT_MODEL_XML --work DIR [--particles 2000000]

(Fact 12's first version ray-marched with openmc.lib.find_material; solid_angle() remains as a check of P in void.)
"""
import argparse
import math
import re
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


def aperture_bound(lower: np.ndarray, rect, lower_left, width, dims, n_e: int, normal=None) -> np.ndarray:
    """Per group, the lowest positive lower bound over voxels just beyond the rectangle (within one voxel of its plane,
    over its footprint)."""
    c, u, v = rect
    n = np.cross(u, v) if normal is None else np.asarray(normal, dtype=float)
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


# Targets, from the response outward: (name, centre, half-extent u, half-extent v, unit normal toward the response).
# All are axis-aligned. The coil faces lie at phi = +-2 degrees (y = +-20.3 to +-22.5 cm over R 581-645); the planes
# y = +-20 sit just inside the gap between the port coils.
TARGETS = [
    ("coil_face_00", (613.0, 20.0, 0.0), (32.0, 0.0, 0.0), (0.0, 0.0, 30.0), (0.0, 1.0, 0.0)),
    ("coil_face_17", (613.0, -20.0, 0.0), (32.0, 0.0, 0.0), (0.0, 0.0, 30.0), (0.0, -1.0, 0.0)),
    ("duct_exit", (577.5, 0.0, 0.0), (0.0, 22.0, 0.0), (0.0, 0.0, 22.0), (1.0, 0.0, 0.0)),
    ("duct_inlet", (445.5, 0.0, 0.0), (0.0, 15.0, 0.0), (0.0, 0.0, 15.0), (1.0, 0.0, 0.0)),
]
E_MIN = 1.0e5  # the model's neutron cut-off; groups below it are never tracked


def target_area(u, v) -> float:
    return 4.0 * float(np.linalg.norm(np.cross(u, v)))


def reach_probability(flux: np.ndarray, area: float, group_fraction: float, volume: float) -> np.ndarray:
    """P = phi A / 4 from a tally of uncollided track length per source neutron; each group got group_fraction of
    the source."""
    return flux / volume / group_fraction * area / 4.0


def cap_bounds(lower: np.ndarray, w_t: np.ndarray, prob: np.ndarray, rel_err: np.ndarray,
               max_rel_err: float = 0.5) -> np.ndarray:
    """Lower bounds (groups x voxels) capped at w_t[g] / prob where prob is resolved; never raised, -1 kept."""
    ok = (prob > 0) & (rel_err <= max_rel_err) & (lower > 0) & np.isfinite(w_t)[:, None]
    cap = np.where(ok, w_t[:, None] / np.where(prob > 0, prob, 1.0), np.inf)
    return np.where(ok & (cap < lower), cap, lower)


def visibility_run(base: Path, work: Path, target, groups: np.ndarray, mesh_xml: str, particles: int,
                   threads: int = 4):
    """Uncollided flux on the window mesh from a cosine-law source on the target; returns (mean, rel_err) arrays of
    shape (n_groups_above_cutoff, n_voxels)."""
    import subprocess
    import openmc
    name, c, u, v, n = target
    c, u, v, n = map(np.asarray, (c, u, v, n))
    work.mkdir(parents=True, exist_ok=True)
    lo, hi = c - np.abs(u) - np.abs(v), c + np.abs(u) + np.abs(v)
    ax = lambda k: (openmc.stats.Discrete([c[k]], [1.0]) if lo[k] == hi[k] else openmc.stats.Uniform(lo[k], hi[k]))
    mids = np.sqrt(groups[:-1] * groups[1:])
    src = openmc.IndependentSource(
        space=openmc.stats.CartesianIndependent(ax(0), ax(1), ax(2)),
        angle=openmc.stats.PolarAzimuthal(mu=openmc.stats.PowerLaw(0.0, 1.0, 1.0),
                                          phi=openmc.stats.Uniform(0.0, 2 * math.pi), reference_uvw=tuple(-n),
                                          reference_vwu=tuple(u / np.linalg.norm(u))),
        energy=openmc.stats.Discrete(mids, np.full(len(mids), 1.0 / len(mids))))
    st = openmc.Settings()
    st.run_mode = "fixed source"
    st.batches = 10
    st.particles = particles // 10
    st.source = src
    st.cutoff = {"energy_neutron": E_MIN}
    st.max_lost_particles = 10000
    st.rel_max_lost_particles = 0.5
    st.output = {"tallies": False}
    st.export_to_xml(work / "settings.xml")
    settings = (work / "settings.xml").read_text()
    settings = settings[settings.index("<settings>"):settings.index("</settings>") + len("</settings>")]
    (work / "settings.xml").unlink()
    edges = " ".join(repr(float(e)) for e in groups)
    tallies = f"""<tallies>
    {mesh_xml}
    <filter id="9001" type="mesh"><bins>9000</bins></filter>
    <filter id="9002" type="energy"><bins>{edges}</bins></filter>
    <filter id="9003" type="collision"><bins>0</bins></filter>
    <tally id="9001" name="uncollided"><filters>9003 9002 9001</filters><scores>flux</scores></tally>
  </tallies>"""
    x = base.read_text()
    x = x[:x.index("<settings>")] + settings + x[x.index("</settings>") + len("</settings>"):]
    # the settings' <output> block has its own <tallies> element, so take the last one
    x = x[:x.rindex("<tallies>")] + tallies + x[x.rindex("</tallies>") + len("</tallies>"):]
    (work / "model.xml").write_text(x)
    import sys
    exe = str(Path(sys.executable).parent / "openmc")
    subprocess.run([exe, "-s", str(threads), "model.xml"], cwd=work, check=True, capture_output=True)
    with openmc.StatePoint(work / "statepoint.10.h5") as sp:
        t = sp.get_tally(name="uncollided")
        mean = t.get_values(value="mean").reshape(len(groups) - 1, -1)
        sd = t.get_values(value="std_dev").reshape(len(groups) - 1, -1)
    return mean, np.where(mean > 0, sd / np.where(mean > 0, mean, 1.0), np.inf)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("base", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("--particles", type=int, default=2_000_000)
    ap.add_argument("--targets", type=Path, help="targets.json from s0f_find_targets.py, in place of the hand-placed ones")
    args = ap.parse_args()
    targets = TARGETS
    if args.targets:
        import json
        targets = [(r["name"], tuple(r["centre"]), tuple(r["u"]), tuple(r["v"]), tuple(r["normal"]))
                   for r in json.loads(args.targets.read_text())]
    x = args.base.read_text()
    j = x.index('<mesh id="9000"')
    mesh_xml = x[j:x.index("</mesh>", j) + len("</mesh>")]
    get = lambda tag, s: [float(t) for t in re.search(rf"<{tag}>(.*?)</{tag}>", s, re.S).group(1).split()]
    dims = [int(v) for v in get("dimension", mesh_xml)]
    lower_left, width = get("lower_left", mesh_xml), get("width", mesh_xml)
    i = x.index('<weight_windows id="1">')
    k = x.index("</weight_windows>", i)
    ww = x[i:k]
    bounds = np.array(get("energy_bounds", ww))
    n_e, n_vox = len(bounds) - 1, int(np.prod(dims))
    lower = np.array(get("lower_ww_bounds", ww)).reshape(n_e, n_vox)
    upper = np.array(get("upper_ww_bounds", ww)).reshape(n_e, n_vox)
    ratio = np.where(lower > 0, upper / np.where(lower > 0, lower, 1.0), 0.0)
    g0 = int(np.searchsorted(bounds, E_MIN))          # first group above the cut-off
    groups = bounds[g0:]
    volume = float(np.prod(width))
    new = lower.copy()
    for target in targets:
        name, c, u, v, n = target
        mean, rel = visibility_run(args.base, args.work / name, target, groups, mesh_xml, args.particles)
        prob = reach_probability(mean, target_area(u, v), 1.0 / (len(groups) - 1), volume)
        w_t = aperture_bound(new.ravel(), (np.asarray(c), np.asarray(u), np.asarray(v)), lower_left, width, dims,
                             n_e, normal=np.asarray(n))[g0:]
        before = new[g0:].copy()
        new[g0:] = cap_bounds(before, w_t, prob, rel)
        m = new[g0:] < before
        print(f"{name}: w_t {np.nanmin(w_t[np.isfinite(w_t)]):.3g}-{np.nanmax(w_t[np.isfinite(w_t)]):.3g}, lowered "
              f"{int(m.sum())} bounds, median factor "
              f"{float(np.median(before[m] / new[g0:][m])) if m.any() else 1.0:.3g}", flush=True)
    m = new < lower
    new_up = np.where(m, new * ratio, upper)
    print(f"total: lowered {int(m.sum())} of {int((lower > 0).sum())} positive bounds; factor median "
          f"{float(np.median(lower[m] / new[m])):.3g}, 99th percentile {float(np.percentile(lower[m] / new[m], 99)):.3g}")
    fmt = lambda a: " ".join(repr(float(v)) for v in a.ravel())
    ww2 = re.sub(r"<lower_ww_bounds>.*?</lower_ww_bounds>", f"<lower_ww_bounds>{fmt(new)}</lower_ww_bounds>", ww,
                 flags=re.S)
    ww2 = re.sub(r"<upper_ww_bounds>.*?</upper_ww_bounds>", f"<upper_ww_bounds>{fmt(new_up)}</upper_ww_bounds>", ww2,
                 flags=re.S)
    args.out.write_text(x[:i] + ww2 + x[k:])
    print(args.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
