"""S0f fact 14: find the streaming targets from the model, with no hand-placed rectangles.

Step 1 (this file's `uncollided_map`): an analog run of the model's own source with a high energy cut-off, tallying
uncollided flux on a fine regular mesh, split by material (response material against everything else). Voxels of the
response that are lit by uncollided source neutrons are the response's line-of-sight surface; lit voxels outside the
response trace the void channel that leads there.

    python s0f_find_targets.py BASE_MODEL_XML WORK_DIR [--particles N] [--cell CM]
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

import numpy as np


MAP_CELLS = 2_000_000   # a map of at most this many cells (its statepoint ~50 MB): a large machine gets larger cells


def uncollided_map(base: Path, work: Path, particles: int, cell: float, response_material: int, cutoff: float,
                   threads: int = 4, max_cells: int = MAP_CELLS):
    """Returns (flux[2, nx, ny, nz] for (response material, all), lower_left, width, dims).

    Cells are `cell` cm wide, or wider when the window mesh's box would need more than `max_cells` of them: on a
    machine 1.8 times RM-M's size 10 cm cells were 7.4 million, and their statepoint alone (194 MB) took the run
    directory past the job's disk cap."""
    import openmc
    x = base.read_text()
    j = x.index('<mesh id="9000"')
    mesh = x[j:x.index("</mesh>", j)]
    get = lambda tag: [float(t) for t in re.search(rf"<{tag}>(.*?)</{tag}>", mesh, re.S).group(1).split()]
    ll, wd, dm = np.array(get("lower_left")), np.array(get("width")), np.array(get("dimension"))
    ur = ll + wd * dm
    cell = max(cell, float(np.prod(ur - ll) / max_cells) ** (1.0 / 3.0))
    dims = np.ceil((ur - ll) / cell).astype(int)
    width = (ur - ll) / dims
    work.mkdir(parents=True, exist_ok=True)
    s = x[x.index("<settings>"):x.index("</settings>") + len("</settings>")]
    s = re.sub(r"<particles>\d+</particles>", f"<particles>{particles // 10}</particles>", s, count=1)
    s = re.sub(r"<batches>\d+</batches>", "<batches>10</batches>", s, count=1)
    s = re.sub(r"<weight_windows id.*?</weight_windows>", "", s, flags=re.S)
    s = re.sub(r"<weight_windows_on>.*?</weight_windows_on>", "", s, flags=re.S)
    s = re.sub(r"<weight_windows_file>.*?</weight_windows_file>", "", s, flags=re.S)
    s = re.sub(r"<weight_window_checkpoints>.*?</weight_window_checkpoints>", "", s, flags=re.S)
    s = re.sub(r"<state_point>.*?</state_point>", "", s, flags=re.S)
    s = re.sub(r"<energy_neutron>.*?</energy_neutron>", f"<energy_neutron>{cutoff}</energy_neutron>", s, flags=re.S)
    tallies = f"""<tallies>
    <mesh id="9500"><dimension>{dims[0]} {dims[1]} {dims[2]}</dimension><lower_left>{ll[0]} {ll[1]} {ll[2]}</lower_left><upper_right>{ur[0]} {ur[1]} {ur[2]}</upper_right></mesh>
    <filter id="9501" type="mesh"><bins>9500</bins></filter>
    <filter id="9502" type="collision"><bins>0</bins></filter>
    <filter id="9503" type="material"><bins>{response_material}</bins></filter>
    <tally id="9501" name="uncollided_all"><filters>9502 9501</filters><scores>flux</scores></tally>
    <tally id="9502" name="uncollided_response"><filters>9502 9503 9501</filters><scores>flux</scores></tally>
  </tallies>"""
    x = x[:x.index("<settings>")] + s + x[x.index("</settings>") + len("</settings>"):]
    x = x[:x.rindex("<tallies>")] + tallies + x[x.rindex("</tallies>") + len("</tallies>"):]
    (work / "model.xml").write_text(x)
    exe = str(Path(sys.executable).parent / "openmc")
    subprocess.run([exe, "-s", str(threads), "model.xml"], cwd=work, check=True, capture_output=True)
    with openmc.StatePoint(work / "statepoint.10.h5") as sp:
        shape = tuple(int(d) for d in dims)
        allf = sp.get_tally(name="uncollided_all").get_values(value="mean").reshape(shape, order="F")
        resp = sp.get_tally(name="uncollided_response").get_values(value="mean").reshape(shape, order="F")
    return np.stack([resp, allf]), ll, width, dims


def response_faces(resp: np.ndarray, allf: np.ndarray, ll, width, join: int = 1):
    """Clusters of lit response voxels -> one axis-aligned face each: (centre, half-extents u, v, unit normal into the
    response). The face is the cluster's thinnest-axis side that the uncollided flux arrives through."""
    from scipy import ndimage
    # Few source neutrons reach the response uncollided, so the lit voxels are sparse: join those within `join`
    # voxels of each other before taking bounding boxes.
    near = ndimage.binary_dilation(resp > 0, structure=np.ones((3, 3, 3)), iterations=join)
    lab, n = ndimage.label(near, structure=np.ones((3, 3, 3)))
    lab = np.where(resp > 0, lab, 0)
    faces = []
    for k in range(1, n + 1):
        idx = np.argwhere(lab == k)
        lo, hi = idx.min(axis=0), idx.max(axis=0) + 1
        ext = hi - lo
        ax = int(np.argmin(ext))
        # flux in the layer just outside each side of the box along the thin axis
        def layer(i):
            sl = [slice(lo[d], hi[d]) for d in range(3)]
            if i < 0 or i >= allf.shape[ax]:
                return 0.0
            sl[ax] = slice(i, i + 1)
            return float(allf[tuple(sl)].sum())
        below, above = layer(lo[ax] - 1), layer(hi[ax])
        side_lo = below >= above                     # neutrons arrive through the low side
        plane = ll[ax] + width[ax] * (lo[ax] if side_lo else hi[ax])
        normal = np.zeros(3)
        normal[ax] = 1.0 if side_lo else -1.0
        centre = ll + width * (lo + hi) / 2.0
        centre[ax] = plane
        half = width * ext / 2.0
        others = [d for d in range(3) if d != ax]
        u, v = np.zeros(3), np.zeros(3)
        u[others[0]], v[others[1]] = half[others[0]], half[others[1]]
        faces.append((centre, u, v, normal, float(resp[lab == k].sum())))
    return faces


def channel_rectangles(allf: np.ndarray, p_rev: np.ndarray, ll, width, face, keep: float = 0.02,
                       narrow: float = 1.4, signal: float = 0.01):
    """Exit and inlet rectangles of the void channel that feeds a face.

    The channel is where the forward uncollided flux times the reverse reach probability is within `keep` of its
    slice maximum. Along the dominant axis from the face toward the source, slices below `signal` of the largest
    slice sum are dropped as noise; the duct is the longest contiguous run of the rest whose channel area stays
    within `narrow` times the smallest area. Its first and last slices give the exit and the inlet (on RM-M the
    area is 12-16 voxels over x 435-585, 20 and more outside). Returns [(name, centre, u, v, normal)], nearest
    the response first."""
    centre, u, v, normal, _ = face
    prod = allf * p_rev
    tot = prod.sum()
    cen = np.array([np.tensordot(prod, (ll[d] + width[d] * (np.arange(prod.shape[d]) + 0.5)), axes=([d], [0])).sum()
                    for d in range(3)]) / tot if tot > 0 else centre
    d = cen - centre
    ax = int(np.argmax(np.abs(d)))
    step = 1 if d[ax] > 0 else -1
    i0 = int(np.clip(np.floor((centre[ax] - ll[ax]) / width[ax]), 0, prod.shape[ax] - 1))
    others = [k for k in range(3) if k != ax]
    slices = []
    i = i0
    while 0 <= i < prod.shape[ax]:
        sl = np.take(prod, i, axis=ax)
        if sl.max() > 0:
            m = sl >= keep * sl.max()
            idx = np.argwhere(m)
            slices.append((i, int(m.sum()), idx.min(axis=0), idx.max(axis=0) + 1, float(sl.sum())))
        i += step
    if not slices:
        return []
    # Keep the slices that carry signal (the far side of the source region is noise), then take the longest
    # contiguous run whose area stays within `narrow` of the smallest: the duct.
    top = max(s[4] for s in slices)
    slices = [s for s in slices if s[4] >= signal * top]
    amin = min(s[1] for s in slices)
    runs, cur = [], []
    for s in slices:
        if s[1] <= narrow * amin and (not cur or abs(s[0] - cur[-1][0]) == 1):
            cur.append(s)
        else:
            if cur:
                runs.append(cur)
            cur = [s] if s[1] <= narrow * amin else []
    if cur:
        runs.append(cur)
    run = max(runs, key=len)
    out = []
    for name, s in (("exit", run[0]), ("inlet", run[-1])):
        i, _, lo, hi, _ = s
        c = np.zeros(3)
        c[ax] = ll[ax] + width[ax] * (i + 0.5)
        uu, vv = np.zeros(3), np.zeros(3)
        for k, (o, vec) in enumerate(zip(others, (uu, vv))):
            c[o] = ll[o] + width[o] * (lo[k] + hi[k]) / 2.0
            vec[o] = width[o] * (hi[k] - lo[k]) / 2.0
        nrm = np.zeros(3)
        nrm[ax] = -float(step)                        # toward the response
        if np.dot(np.cross(uu, vv), nrm) < 0:         # the one-sided rule reads the normal as u x v
            uu, vv = vv, uu
        out.append((name, c, uu, vv, nrm))
    return out


def reverse_map(base: Path, work: Path, face, particles: int, ll, dims, energy: float = 1.4e7, threads: int = 4):
    """Uncollided reach probability onto `face` for every voxel of the fine mesh (reciprocity; see
    s0f_los_windows.py). Returns P[nx, ny, nz]."""
    import math
    import openmc
    centre, u, v, normal, _ = face
    work.mkdir(parents=True, exist_ok=True)
    start = centre - 0.5 * normal                     # just outside the response
    lo, hi = start - np.abs(u) - np.abs(v), start + np.abs(u) + np.abs(v)
    ax = lambda k: (openmc.stats.Discrete([start[k]], [1.0]) if lo[k] == hi[k] else openmc.stats.Uniform(lo[k], hi[k]))
    ref_v = u / np.linalg.norm(u)
    src = openmc.IndependentSource(
        space=openmc.stats.CartesianIndependent(ax(0), ax(1), ax(2)),
        angle=openmc.stats.PolarAzimuthal(mu=openmc.stats.PowerLaw(0.0, 1.0, 1.0),
                                          phi=openmc.stats.Uniform(0.0, 2 * math.pi), reference_uvw=tuple(-normal),
                                          reference_vwu=tuple(ref_v)),
        energy=openmc.stats.Discrete([energy], [1.0]))
    st = openmc.Settings()
    st.run_mode = "fixed source"
    st.batches, st.particles, st.source = 10, particles // 10, src
    st.cutoff = {"energy_neutron": 0.7 * energy}
    st.max_lost_particles, st.rel_max_lost_particles = 10000, 0.5
    st.output = {"tallies": False}
    st.export_to_xml(work / "settings.xml")
    settings = (work / "settings.xml").read_text()
    settings = settings[settings.index("<settings>"):settings.index("</settings>") + len("</settings>")]
    (work / "settings.xml").unlink()
    x = base.read_text()
    j = x.index('<mesh id="9000"')
    mesh = x[j:x.index("</mesh>", j)]
    get = lambda tag: [float(t) for t in re.search(rf"<{tag}>(.*?)</{tag}>", mesh, re.S).group(1).split()]
    ur = np.array(get("lower_left")) + np.array(get("width")) * np.array(get("dimension"))
    tallies = f"""<tallies>
    <mesh id="9500"><dimension>{dims[0]} {dims[1]} {dims[2]}</dimension><lower_left>{ll[0]} {ll[1]} {ll[2]}</lower_left><upper_right>{ur[0]} {ur[1]} {ur[2]}</upper_right></mesh>
    <filter id="9501" type="mesh"><bins>9500</bins></filter>
    <filter id="9502" type="collision"><bins>0</bins></filter>
    <tally id="9501" name="uncollided"><filters>9502 9501</filters><scores>flux</scores></tally>
  </tallies>"""
    x = x[:x.index("<settings>")] + settings + x[x.index("</settings>") + len("</settings>"):]
    x = x[:x.rindex("<tallies>")] + tallies + x[x.rindex("</tallies>") + len("</tallies>"):]
    (work / "model.xml").write_text(x)
    exe = str(Path(sys.executable).parent / "openmc")
    subprocess.run([exe, "-s", str(threads), "model.xml"], cwd=work, check=True, capture_output=True)
    with openmc.StatePoint(work / "statepoint.10.h5") as sp:
        f = sp.get_tally(name="uncollided").get_values(value="mean").reshape(tuple(int(d) for d in dims), order="F")
    width = (ur - ll) / np.asarray(dims)
    area = 4.0 * float(np.linalg.norm(np.cross(u, v)))
    return f / float(np.prod(width)) * area / 4.0


def find_targets(base: Path, work: Path, particles: int, cell: float, response_material: int, cutoff: float,
                 min_share: float = 0.05):
    """All targets for a model, from the response outward: [(name, centre, u, v, normal)]."""
    flux, ll, width, dims = uncollided_map(base, work / "map", particles, cell, response_material, cutoff)
    faces = response_faces(flux[0], flux[1], ll, width)
    total = sum(f[4] for f in faces)
    out = []
    for k, face in enumerate(sorted(faces, key=lambda f: -f[4])):
        if face[4] < min_share * total:
            continue                                  # a stray lit voxel, not a face
        out.append((f"face_{k}", face[0], face[1], face[2], face[3]))
        p_rev = reverse_map(base, work / f"rev_{k}", face, particles // 5, ll, dims)
        for name, c, uu, vv, nrm in channel_rectangles(flux[1], p_rev, ll, width, face):
            out.append((f"{name}_{k}", c, uu, vv, nrm))
    return order_outward(merge_rectangles(out, tol=1.5 * float(max(width))))


def merge_rectangles(targets, tol: float):
    """Faces that share a duct yield the same exit and inlet twice: merge rectangles with the same normal whose
    planes lie within tol and whose extents overlap, into their bounding rectangle."""
    out = []
    for t in targets:
        name, c, u, v, n = t
        for k, (name2, c2, u2, v2, n2) in enumerate(out):
            if name.split("_")[0] != name2.split("_")[0] or name.startswith("face") or not np.allclose(n, n2):
                continue
            ax = int(np.argmax(np.abs(n)))
            h, h2 = np.abs(u) + np.abs(v), np.abs(u2) + np.abs(v2)
            if abs(c[ax] - c2[ax]) <= tol and np.all(np.abs(c - c2)[[d for d in range(3) if d != ax]]
                                                     <= (h + h2)[[d for d in range(3) if d != ax]]):
                lo, hi = np.minimum(c - h, c2 - h2), np.maximum(c + h, c2 + h2)
                cc = (lo + hi) / 2.0
                cc[ax] = (c[ax] + c2[ax]) / 2.0
                half = (hi - lo) / 2.0
                others = [d for d in range(3) if d != ax]
                uu, vv = np.zeros(3), np.zeros(3)
                uu[others[0]], vv[others[1]] = half[others[0]], half[others[1]]
                out[k] = (name2, cc, uu, vv, n2)
                break
        else:
            out.append(t)
    return out


def order_outward(targets):
    """Response faces first, then exits, then inlets: the order the windows are capped in (s0f_los_windows.py)."""
    rank = {"face": 0, "exit": 1, "inlet": 2}
    return sorted(targets, key=lambda t: rank[t[0].split("_")[0]])


def aperture_spec(targets, beta: float = 0.1) -> str:
    """OPENMC_APERTURE_BIAS for the exits and inlets found (the faces are response targets, not aimed at)."""
    num = lambda a: " ".join(f"{float(x):g}" for x in a)
    rects = [f"{num(c)} {num(u)} {num(v)}" for name, c, u, v, _ in order_outward(targets) if not name.startswith("face")]
    return f"{beta:g} " + "; ".join(rects)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("base", type=Path)
    ap.add_argument("work", type=Path)
    ap.add_argument("--particles", type=int, default=10_000_000)
    ap.add_argument("--cell", type=float, default=10.0)
    ap.add_argument("--response-material", type=int, default=4)
    ap.add_argument("--cutoff", type=float, default=1.0e7)
    args = ap.parse_args()
    import json
    targets = find_targets(args.base, args.work, args.particles, args.cell, args.response_material, args.cutoff)
    rows = [{"name": n, "centre": list(map(float, c)), "u": list(map(float, u)), "v": list(map(float, v)),
             "normal": list(map(float, nr))} for n, c, u, v, nr in targets]
    (args.work / "targets.json").write_text(json.dumps(rows, indent=1))
    print("OPENMC_APERTURE_BIAS=" + aperture_spec(targets))
    for r in rows:
        print(r["name"], "centre", r["centre"], "u", r["u"], "v", r["v"], "normal", r["normal"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
