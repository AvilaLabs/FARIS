"""GEO-032: the faceting deviation of a converted model from the CAD solids.

The sample points are the centroids of a solid's triangles and the midpoints of their edges
(each edge once). The deviation of a point is its distance to the solid's boundary, the
minimum over the solid's faces of the distance to the face. Vertices lie on the CAD surface
by construction, so these points are where the faceting departs from it most.

Two ways to measure a point, both exact:

* Faces on a plane, cylinder, cone, sphere or torus: the distance to the surface in closed
  form (numpy, many points at once). It is the distance to the face when the foot point (the
  nearest point of the whole surface) lies inside the face, which OpenCASCADE's 2D face
  classifier decides. When it does not, the point falls back to OpenCASCADE's projection.
* Every other face (B-spline, Bezier, offset, revolved or extruded curves) and the fallback
  points: `BRepExtrema_DistShapeShape`, which costs 1 to 25 ms a point. These run on an even
  stride, at most `occt_points` per solid (0: all of them).

A point's closed-form result is certified, i.e. the exact distance to the boundary, when the
validated face distance is no larger than a lower bound of every other face: the closed-form
distance to its surface, the distance to its bounding box, and the distance to the strip of
its surface between its (u, v) bounds (which is what rules out a neighbour whose surface is
nearer but whose face is not), and no face OpenCASCADE has to measure can be nearer by its
bounding box. A point that cannot be certified still has a validated face distance, an upper
bound of its distance; if that is no larger than the largest certified distance of the
solid, it cannot raise the maximum and is settled. The rest go to OpenCASCADE.

Coverage is reported per solid. A solid whose every point was settled or measured has a
"maximum over all points": exact over the sample points, not a bound over the points between
them. Otherwise the result is a "sampled maximum".
"""
from __future__ import annotations

import math
import os

# One thread per process: the measurement runs in worker processes, and a threaded BLAS in each of
# them would only fight for the cores. Must be set before numpy is imported.
for _variable in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_variable, "1")

import numpy as np

KINDS = ("plane", "cylinder", "cone", "sphere", "torus")
TWO_PI = 2.0 * math.pi
# Length tolerance of the face classifier, in shape units (OpenCASCADE's own default scale).
CLASSIFY_TOLERANCE = 1.0e-7
# A point this close to an axis or centre has no unique foot point; it is measured by OpenCASCADE.
DEGENERATE = 1.0e-9


def _vec(d) -> np.ndarray:
    return np.array([d.X(), d.Y(), d.Z()], dtype=np.float64)


def _pnt(p) -> np.ndarray:
    return np.array([p.X(), p.Y(), p.Z()], dtype=np.float64)


def surface_parameters(face):
    """The analytic surface of a face as (kind, parameters), or (None, None).

    The parameters are in shape units: `o` the reference point, `x`, `y`, `z` the unit
    axes of OpenCASCADE's local frame, and per kind the radii, the half-angle and `apex`.
    """
    from OCP.BRepAdaptor import BRepAdaptor_Surface
    from OCP.GeomAbs import GeomAbs_Cone, GeomAbs_Cylinder, GeomAbs_Plane, GeomAbs_Sphere, GeomAbs_Torus

    surface = BRepAdaptor_Surface(face)
    kind = surface.GetType()
    if kind == GeomAbs_Plane:
        position = surface.Plane().Position()
        values = {}
        name = "plane"
    elif kind == GeomAbs_Cylinder:
        shape = surface.Cylinder()
        position = shape.Position()
        values = {"r": shape.Radius()}
        name = "cylinder"
    elif kind == GeomAbs_Cone:
        shape = surface.Cone()
        position = shape.Position()
        half = abs(shape.SemiAngle())
        radius = shape.RefRadius()
        # The reference circle has radius R at z = 0; the apex is where the radius is zero.
        values = {"half": half, "R": radius, "signed": shape.SemiAngle()}
        name = "cone"
    elif kind == GeomAbs_Sphere:
        shape = surface.Sphere()
        position = shape.Position()
        values = {"r": shape.Radius()}
        name = "sphere"
    elif kind == GeomAbs_Torus:
        shape = surface.Torus()
        position = shape.Position()
        values = {"major": shape.MajorRadius(), "minor": shape.MinorRadius()}
        if values["major"] <= values["minor"]:
            return None, None  # a self-intersecting torus has no simple closed form
        name = "torus"
    else:
        return None, None
    out = {
        "o": _pnt(position.Location()),
        "x": _vec(position.XDirection()),
        "y": _vec(position.YDirection()),
        "z": _vec(position.Direction()),
        **values,
    }
    if name == "cone":
        if out["half"] < 1.0e-9 or abs(out["half"] - math.pi / 2.0) < 1.0e-9:
            return None, None
        out["apex"] = out["o"] - out["z"] * (out["R"] / math.tan(out["signed"]))
    return name, out


# ---------------------------------------------------------------------------
# Closed-form distances. P is (n, 3); `a` holds the parameters of m faces of one kind as arrays
# (`stack_parameters`); the result is (n, m).

def stack_parameters(params) -> dict:
    """Parameters of several faces of one kind, as one array per key."""
    return {key: np.array([p[key] for p in params], dtype=np.float64) for key in params[0]}


def _radial(rel: np.ndarray, axis: np.ndarray):
    """Axial coordinate and radial distance of `rel` (n, m, 3) about `axis` (m, 3)."""
    t = np.einsum("nmk,mk->nm", rel, axis)
    perp = rel - t[..., None] * axis[None]
    return t, np.sqrt(np.einsum("nmk,nmk->nm", perp, perp))


def distance_plane(P, a):
    return np.abs(np.einsum("nmk,mk->nm", P[:, None, :] - a["o"][None], a["z"]))


def distance_cylinder(P, a):
    _t, rho = _radial(P[:, None, :] - a["o"][None], a["z"])
    return np.abs(rho - a["r"][None])


def distance_sphere(P, a):
    rel = P[:, None, :] - a["o"][None]
    return np.abs(np.sqrt(np.einsum("nmk,nmk->nm", rel, rel)) - a["r"][None])


def distance_torus(P, a):
    t, rho = _radial(P[:, None, :] - a["o"][None], a["z"])
    return np.abs(np.hypot(rho - a["major"][None], t) - a["minor"][None])


def _cone_rays(t, rho, half):
    """Distance to the two rays from the apex that make the double cone, in the (axial, radial) plane."""
    c, s = np.cos(half)[None], np.sin(half)[None]
    along1 = t * c + rho * s
    along2 = -t * c + rho * s
    d1 = np.where(along1 >= 0.0, np.abs(t * s - rho * c), np.hypot(t, rho))
    d2 = np.where(along2 >= 0.0, np.abs(t * s + rho * c), np.hypot(t, rho))
    return d1, d2, along1, along2


def distance_cone(P, a):
    t, rho = _radial(P[:, None, :] - a["apex"][None], a["z"])
    d1, d2, _a1, _a2 = _cone_rays(t, rho, a["half"])
    return np.minimum(d1, d2)


DISTANCES = {
    "plane": distance_plane,
    "cylinder": distance_cylinder,
    "cone": distance_cone,
    "sphere": distance_sphere,
    "torus": distance_torus,
}


# The same distances for many points and many faces by matrix products: about ten times faster than
# the direct forms and accurate to a few nanometres at the sizes of a reactor model, so they rank
# and bound; any distance that is reported is recomputed with the direct forms above.

def _frame(P, centre, origin, axis, want_sq=False):
    """Axial coordinate t and radial distance rho (and the squared distance) of P (n, 3) from
    `origin` (m, 3) about `axis` (m, 3), by matrix products, in place to keep the arrays small."""
    Pc = P - centre
    o = origin - centre
    t = Pc @ axis.T
    t -= np.einsum("mk,mk->m", o, axis)[None]
    sq = Pc @ o.T
    sq *= -2.0
    sq += np.einsum("nk,nk->n", Pc, Pc)[:, None]
    sq += np.einsum("mk,mk->m", o, o)[None]
    rho = t * t
    np.subtract(sq, rho, out=rho)
    np.maximum(rho, 0.0, out=rho)
    np.sqrt(rho, out=rho)
    return (t, sq, rho) if want_sq else (t, None, rho)


def fast_plane(P, a, centre):
    t, _sq, _rho = _frame(P, centre, a["o"], a["z"])
    return np.abs(t)


def fast_cylinder(P, a, centre):
    _t, _sq, rho = _frame(P, centre, a["o"], a["z"])
    return np.abs(rho - a["r"][None])


def fast_sphere(P, a, centre):
    _t, sq, _rho = _frame(P, centre, a["o"], a["z"], want_sq=True)
    return np.abs(np.sqrt(np.maximum(sq, 0.0)) - a["r"][None])


def fast_torus(P, a, centre):
    t, _sq, rho = _frame(P, centre, a["o"], a["z"])
    return np.abs(np.hypot(rho - a["major"][None], t) - a["minor"][None])


def fast_cone(P, a, centre):
    """Distance to the two generatrix lines of the double cone (a lower bound of the distance to
    the cone, equal to it when the foot point is on the generatrix ray, not past the apex)."""
    t, _sq, rho = _frame(P, centre, a["apex"], a["z"])
    return np.abs(rho * np.cos(a["half"])[None] - np.abs(t) * np.sin(a["half"])[None])


FAST = {
    "plane": fast_plane,
    "cylinder": fast_cylinder,
    "cone": fast_cone,
    "sphere": fast_sphere,
    "torus": fast_torus,
}
# Margin for the fast forms' rounding, shape units (nanometres for millimetres).
FAST_MARGIN = 1.0e-6
# Two faces of the same surface are equally far from a point to this many shape units (1e-8 mm);
# a face that is not nearer than the validated one by more than this does not undo its certificate.
TIE_TOLERANCE = 1.0e-8


# ---------------------------------------------------------------------------
# Many (point, face) pairs at once. In these functions `a` holds one row of parameters per pair
# (`pair_parameters`), P is (n, 3), and every result is per pair.

def pair_parameters(f, n) -> dict:
    """One face's scalar parameters repeated for n pairs."""
    return {key: np.broadcast_to(np.asarray(value, dtype=np.float64), (n, *np.shape(value))) for key, value in f.items()}


def _dot(a, b):
    return np.einsum("nk,nk->n", a, b)


def paired_distance(kind, a, P):
    """Distance of each point to the infinite surface of its face (direct form)."""
    if kind == "cone":
        rel = P - a["apex"]
        t = _dot(rel, a["z"])
        perp = rel - t[:, None] * a["z"]
        rho = np.sqrt(_dot(perp, perp))
        c, s = np.cos(a["half"]), np.sin(a["half"])
        along1, along2 = t * c + rho * s, -t * c + rho * s
        apex = np.hypot(t, rho)
        d1 = np.where(along1 >= 0.0, np.abs(t * s - rho * c), apex)
        d2 = np.where(along2 >= 0.0, np.abs(t * s + rho * c), apex)
        return np.minimum(d1, d2)
    rel = P - a["o"]
    t = _dot(rel, a["z"])
    if kind == "plane":
        return np.abs(t)
    if kind == "sphere":
        return np.abs(np.sqrt(_dot(rel, rel)) - a["r"])
    perp = rel - t[:, None] * a["z"]
    rho = np.sqrt(_dot(perp, perp))
    if kind == "cylinder":
        return np.abs(rho - a["r"])
    return np.abs(np.hypot(rho - a["major"], t) - a["minor"])  # torus


def paired_foot(kind, a, P):
    """(foot point, ok): the nearest point of the whole surface; not ok where it is not unique."""
    n = len(P)
    if kind == "plane":
        d = _dot(P - a["o"], a["z"])
        return P - d[:, None] * a["z"], np.ones(n, dtype=bool)
    if kind == "sphere":
        rel = P - a["o"]
        norm = np.sqrt(_dot(rel, rel))
        ok = norm > DEGENERATE
        return a["o"] + a["r"][:, None] * rel / np.where(ok, norm, 1.0)[:, None], ok
    origin = a["apex"] if kind == "cone" else a["o"]
    rel = P - origin
    t = _dot(rel, a["z"])
    perp = rel - t[:, None] * a["z"]
    rho = np.sqrt(_dot(perp, perp))
    radial = rho > DEGENERATE
    unit = perp / np.where(radial, rho, 1.0)[:, None]
    if kind == "cylinder":
        return a["o"] + t[:, None] * a["z"] + a["r"][:, None] * unit, radial
    if kind == "torus":
        q = (rho - a["major"])[:, None] * unit + t[:, None] * a["z"]
        qn = np.sqrt(_dot(q, q))
        ok = radial & (qn > DEGENERATE)
        centre = a["o"] + a["major"][:, None] * unit
        return centre + a["minor"][:, None] * q / np.where(qn > DEGENERATE, qn, 1.0)[:, None], ok
    # cone: the nearer of the two generatrix rays
    c, s = np.cos(a["half"]), np.sin(a["half"])
    along1, along2 = t * c + rho * s, -t * c + rho * s
    apex = np.hypot(t, rho)
    d1 = np.where(along1 >= 0.0, np.abs(t * s - rho * c), apex)
    d2 = np.where(along2 >= 0.0, np.abs(t * s + rho * c), apex)
    first = d1 <= d2
    along = np.where(first, along1, along2)
    foot_t = np.where(first, 1.0, -1.0) * along * c
    foot_rho = along * s
    ok = radial & (along > DEGENERATE)  # at the apex the foot point has no (u, v)
    return a["apex"] + foot_t[:, None] * a["z"] + foot_rho[:, None] * unit, ok


def paired_uv(kind, a, X):
    """(u, v) of points `X` on the surface, as OpenCASCADE's ElSLib parameterises it."""
    rel = X - a["o"]
    x, y, z = _dot(rel, a["x"]), _dot(rel, a["y"]), _dot(rel, a["z"])
    if kind == "plane":
        return x, y
    u = np.mod(np.arctan2(y, x), TWO_PI)
    rho = np.hypot(x, y)
    if kind == "cylinder":
        return u, z
    if kind == "sphere":
        return u, np.arctan2(z, rho)
    if kind == "torus":
        return u, np.mod(np.arctan2(z, rho - a["major"]), TWO_PI)
    # Cone: P = O + (R + V sin a)(cos U X + sin U Y) + V cos a Z, so V = z / cos a for a point on
    # the surface; on the far side of the apex the radius is negative and U is turned by pi.
    v = z / np.cos(a["signed"])
    radius = a["R"] + v * np.sin(a["signed"])
    return np.where(radius < 0.0, np.mod(u + math.pi, TWO_PI), u), v


def paired_strip(kind, a, P):
    """A lower bound of the distance from each point to its face (see `strip_bound`)."""
    rel = P - a["o"]
    t = _dot(rel, a["z"])
    if kind == "plane":
        x, y = _dot(rel, a["x"]), _dot(rel, a["y"])
        gap_x = np.maximum(0.0, np.maximum(a["u0"] - x, x - a["u1"]))
        gap_y = np.maximum(0.0, np.maximum(a["v0"] - y, y - a["v1"]))
        return np.sqrt(t * t + gap_x * gap_x + gap_y * gap_y)
    perp = rel - t[:, None] * a["z"]
    rho = np.sqrt(_dot(perp, perp))
    zero = np.zeros(len(P))
    if kind == "cylinder":
        return _segment_distance(rho, t, a["r"], a["v0"], a["r"], a["v1"])
    if kind == "cone":
        sin, cos = np.sin(a["signed"]), np.cos(a["signed"])
        r0, r1 = a["R"] + a["v0"] * sin, a["R"] + a["v1"] * sin
        bound = _segment_distance(rho, t, r0, a["v0"] * cos, r1, a["v1"] * cos)
        return np.where(r0 * r1 < 0.0, 0.0, bound)  # a face that spans the apex is not bounded
    if kind == "sphere":
        return _arc_distance(rho, t, zero, zero, a["r"], a["v0"], a["v1"])
    return _arc_distance(rho, t, a["major"], zero, a["minor"], a["v0"], a["v1"])  # torus


def uv(kind, f, X):
    return paired_uv(kind, pair_parameters(f, len(X)), X)


FEETS = {kind: (lambda P, f, kind=kind: paired_foot(kind, pair_parameters(f, len(P)), P)) for kind in KINDS}
FEET = FEETS


def _spread_bits(x):
    """Spread the low 21 bits of x so that two zero bits separate each (for a 3D Morton key)."""
    x = x & 0x1FFFFF
    x = (x | x << 32) & 0x1F00000000FFFF
    x = (x | x << 16) & 0x1F0000FF0000FF
    x = (x | x << 8) & 0x100F00F00F00F00F
    x = (x | x << 4) & 0x10C30C30C30C30C3
    x = (x | x << 2) & 0x1249249249249249
    return x


def morton_order(points: np.ndarray, bits: int = 14) -> np.ndarray:
    """Indices that sort points along a 3D Z-order curve, so a block of consecutive points is a cluster."""
    lo = points.min(axis=0)
    span = max(float((points.max(axis=0) - lo).max()), 1.0e-300)
    q = np.floor((points - lo) / span * ((1 << bits) - 1)).astype(np.int64)
    key = _spread_bits(q[:, 0]) | (_spread_bits(q[:, 1]) << 1) | (_spread_bits(q[:, 2]) << 2)
    return np.argsort(key, kind="stable")


def box_distance(P, lo, hi):
    """Distance from points (n, 3) to boxes (m, 3): (n, m)."""
    gap = np.maximum(0.0, np.maximum(lo[None] - P[:, None, :], P[:, None, :] - hi[None]))
    return np.sqrt(np.einsum("nmk,nmk->nm", gap, gap))


def _segment_distance(x, y, x0, y0, x1, y1):
    """Distance from points (x, y), shape (n, m), to segments (x0, y0)-(x1, y1), each shape (m,)."""
    dx, dy = x1 - x0, y1 - y0
    length2 = dx * dx + dy * dy
    s = np.clip(((x - x0) * dx + (y - y0) * dy) / np.where(length2 > 0.0, length2, 1.0), 0.0, 1.0)
    return np.hypot(x - (x0 + s * dx), y - (y0 + s * dy))


def _arc_distance(x, y, cx, cy, radius, a0, a1):
    """Distance from points (x, y), shape (n, m), to arcs of the given radius about (cx, cy) between angles a0 and a1."""
    angle = np.arctan2(y - cy, x - cx)
    span = np.mod(a1 - a0, TWO_PI)
    inside = (np.mod(angle - a0, TWO_PI) <= span) & (span > 0.0)
    on_circle = np.abs(np.hypot(x - cx, y - cy) - radius)
    ends = np.minimum(
        np.hypot(x - (cx + radius * np.cos(a0)), y - (cy + radius * np.sin(a0))),
        np.hypot(x - (cx + radius * np.cos(a1)), y - (cy + radius * np.sin(a1))),
    )
    return np.where(inside, on_circle, ends)


def strip_bound(kind, a, P):
    """`paired_strip` for every (point, face) of P (n, 3) against m faces' parameter arrays `a`: (n, m).

    The face lies inside the strip of its surface between its (u, v) bounds (for a plane, inside
    its rectangle), so its distance is at least that to the strip: in the meridian plane (axial,
    radial) for a surface of revolution, ignoring the angle, and in the plane's own rectangle for
    a plane. A point whose foot point is past the face's edge is bounded by the distance to that
    edge, which belongs to the face; this is what rules out a neighbour whose surface is nearer
    but whose face is not.
    """
    n, m = len(P), len(a["o"])
    pairs = {key: np.repeat(value[None], n, axis=0).reshape(n * m, *value.shape[1:]) for key, value in a.items()}
    return paired_strip(kind, pairs, np.repeat(P, m, axis=0)).reshape(n, m)


def face_lower_bound(kind, f, P):
    """`paired_strip` for one face with scalar parameters: (n,)."""
    return paired_strip(kind, pair_parameters(f, len(P)), P)


def _pair_box_distance(P, lo, hi):
    """Distance from each point (n, 3) to its own box (n, 3) lo/hi."""
    gap = np.maximum(0.0, np.maximum(lo - P, P - hi))
    return np.sqrt(np.einsum("nk,nk->n", gap, gap))


class SolidFaces:
    """One solid's faces: bounding boxes, analytic parameters, and the lazy OpenCASCADE objects."""

    NEAR = 12  # nearest surfaces kept per point
    RANKS = 4  # of those, tried against the face classifier, nearest by bounding box and distance first

    def __init__(self, solid, bounds="optimal"):
        from OCP.Bnd import Bnd_Box
        from OCP.BRepBndLib import BRepBndLib
        from OCP.TopAbs import TopAbs_FACE
        from OCP.TopExp import TopExp_Explorer
        from OCP.BRepTools import BRepTools
        from OCP.TopoDS import TopoDS

        self.faces = []
        self.kind = []
        self.params = []
        lows, highs = [], []
        explorer = TopExp_Explorer(solid, TopAbs_FACE)
        while explorer.More():
            face = TopoDS.Face_s(explorer.Current())
            box = Bnd_Box()
            if bounds == "optimal":
                BRepBndLib.AddOptimal_s(face, box, False, False)
            else:
                BRepBndLib.Add_s(face, box, False)
            lo, hi = box.CornerMin(), box.CornerMax()
            lows.append([lo.X(), lo.Y(), lo.Z()])
            highs.append([hi.X(), hi.Y(), hi.Z()])
            kind, params = surface_parameters(face)
            if params is not None:
                params["u0"], params["u1"], params["v0"], params["v1"] = BRepTools.UVBounds_s(face)
            self.faces.append(face)
            self.kind.append(kind)
            self.params.append(params)
            explorer.Next()
        self.lo = np.array(lows).reshape(-1, 3)
        self.hi = np.array(highs).reshape(-1, 3)
        self.ids = {k: np.array([i for i, kk in enumerate(self.kind) if kk == k], dtype=np.int64) for k in KINDS}
        self.arrays = {k: stack_parameters([self.params[i] for i in self.ids[k]]) for k in KINDS if len(self.ids[k])}
        self.analytic = np.array([k is not None for k in self.kind], dtype=bool)
        self.kind_code = np.array([KINDS.index(k) if k is not None else -1 for k in self.kind], dtype=np.int64)
        self.slot = np.zeros(len(self.faces), dtype=np.int64)  # a face's row in its kind's parameter arrays
        for kind, ids in self.ids.items():
            self.slot[ids] = np.arange(len(ids))
        self._classifiers = {}

    def __len__(self):
        return len(self.faces)

    # -- closed form over many points ----------------------------------------------------

    def closed_form(self, P, candidates, fast=False):
        """(D, face ids): the distance of every point to the infinite surface of each analytic candidate.

        `fast` uses the matrix-product forms, good for ranking and bounds (within FAST_MARGIN).
        """
        columns, ids = [], []
        centre = P.mean(axis=0)
        wanted = np.zeros(len(self.faces), dtype=bool)
        wanted[candidates] = True
        for kind, arrays in self.arrays.items():
            pick = np.nonzero(wanted[self.ids[kind]])[0]
            if len(pick) == 0:
                continue
            chosen = {key: value[pick] for key, value in arrays.items()}
            columns.append(FAST[kind](P, chosen, centre) if fast else DISTANCES[kind](P, chosen))
            ids.append(self.ids[kind][pick])
        if not columns:
            return np.empty((len(P), 0)), np.empty(0, dtype=np.int64)
        return np.concatenate(columns, axis=1), np.concatenate(ids)

    def classifier(self, face):
        found = self._classifiers.get(face)
        if found is None:
            from OCP.BRepTopAdaptor import BRepTopAdaptor_FClass2d

            found = self._classifiers[face] = BRepTopAdaptor_FClass2d(self.faces[face], CLASSIFY_TOLERANCE)
        return found

    def gather(self, kind, faces):
        """Parameter rows, one per entry of `faces` (face ids of `kind`)."""
        rows = self.slot[faces]
        return {key: value[rows] for key, value in self.arrays[kind].items()}

    def exact_pairs(self, block, rows, faces):
        """Closed-form distance of point `rows[i]` to analytic face `faces[i]`, NaN where the foot point of
        its surface is not in the face (OpenCASCADE's face classifier decides)."""
        from OCP.gp import gp_Pnt2d
        from OCP.TopAbs import TopAbs_IN, TopAbs_ON

        out = np.full(len(rows), np.nan)
        points = block[rows]
        accepted = (TopAbs_IN.value, TopAbs_ON.value)  # compared as numbers: the enum's own comparison is slow
        probe = gp_Pnt2d(0.0, 0.0)  # one point, moved for each foot point: cheaper than a new one
        set_coord = probe.SetCoord
        classifiers = self._classifiers
        for code, kind in enumerate(KINDS):
            pick = np.nonzero(self.kind_code[faces] == code)[0]
            if len(pick) == 0:
                continue
            a = self.gather(kind, faces[pick])
            P = points[pick]
            foot, ok = paired_foot(kind, a, P)
            u, v = paired_uv(kind, a, foot)
            inside = []
            for f, good, x, y in zip(faces[pick].tolist(), ok.tolist(), u.tolist(), v.tolist()):
                if not good:
                    inside.append(False)
                    continue
                set_coord(x, y)
                state = (classifiers.get(f) or self.classifier(f)).Perform(probe)
                inside.append(state.value in accepted)
            distance = paired_distance(kind, a, P)
            out[pick] = np.where(inside, distance, np.nan)
        return out

    def exact_on_face(self, face, P):
        """Closed-form distance of points to analytic `face`, NaN where the foot point is not in the face."""
        return self.exact_pairs(P, np.arange(len(P)), np.full(len(P), face, dtype=np.int64))

    def face_bounds_pairs(self, block, rows, faces):
        """Lower bound of the distance of point `rows[i]` to face `faces[i]`: the larger of its distance to the
        face's surface (direct form) and to the strip of the surface the face lies in."""
        out = np.empty(len(rows))
        points = block[rows]
        for code, kind in enumerate(KINDS):
            pick = np.nonzero(self.kind_code[faces] == code)[0]
            if len(pick):
                a = self.gather(kind, faces[pick])
                out[pick] = np.maximum(paired_distance(kind, a, points[pick]), paired_strip(kind, a, points[pick]))
        return out

    # -- OpenCASCADE ---------------------------------------------------------------------

    def occt_distance(self, face, p):
        from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeVertex
        from OCP.BRepExtrema import BRepExtrema_DistShapeShape
        from OCP.gp import gp_Pnt

        vertex = BRepBuilderAPI_MakeVertex(gp_Pnt(float(p[0]), float(p[1]), float(p[2]))).Vertex()
        probe = BRepExtrema_DistShapeShape(vertex, self.faces[face])
        if not probe.IsDone():
            raise RuntimeError("OpenCASCADE could not measure the distance to a face")
        return probe.Value()

    # -- one point, all faces ------------------------------------------------------------

    def point_distance(self, p, subset=None, floor=0.0):
        """Exact distance of one point to the solid's boundary over `subset` faces, by OpenCASCADE where needed.

        Faces are taken in order of their lower bound (bounding box, and the closed-form distance
        for analytic faces). With a `floor`, the search stops as soon as the value is at or below
        it, since it then cannot raise a maximum above `floor`; the value is then an upper bound.
        Returns (value, OpenCASCADE calls).
        """
        ids = np.arange(len(self.faces)) if subset is None else np.asarray(subset)
        point = np.asarray(p, dtype=np.float64)[None]
        bound = box_distance(point, self.lo[ids], self.hi[ids])[0]
        analytic_ids = ids[self.analytic[ids]]
        if len(analytic_ids):
            d, order = self.closed_form(point, analytic_ids)
            position = {int(f): k for k, f in enumerate(ids)}
            for column, f in enumerate(order):
                k = position[int(f)]
                bound[k] = max(bound[k], d[0, column])
        best = math.inf
        calls = 0
        for k in np.argsort(bound):
            if bound[k] >= best:
                break
            f = int(ids[k])
            value = self.exact_on_face(f, point)[0] if self.analytic[f] else math.nan
            if math.isnan(value):
                value = self.occt_distance(f, point[0])
                calls += 1
            best = min(best, value)
            if best <= floor:
                break
        return best, calls

    # -- a block of points ---------------------------------------------------------------

    def bounds(self, block, candidates=None):
        """Closed-form pass over `block` (n, 3), looking only at the `candidates` faces (default all).

        Returns (upper, certified): `upper[i]` is a distance of point i to a face, validated by
        the face classifier, hence an upper bound of its distance to the boundary (infinity when
        none of the nearest faces holds the point's foot point). `certified[i]` is true when no
        other face can be nearer, so `upper[i]` is the exact distance: every other analytic face
        is at least that far by its closed-form distance, its bounding box and its (u, v) strip,
        and no face OpenCASCADE would have to measure is nearer by its bounding box.
        """
        n = len(block)
        upper = np.full(n, np.inf)
        certified = np.zeros(n, dtype=bool)
        everything = np.arange(len(self.faces)) if candidates is None else np.asarray(candidates)
        analytic = everything[self.analytic[everything]]
        other = everything[~self.analytic[everything]]
        if len(analytic) == 0:
            return upper, certified
        d, ids = self.closed_form(block, analytic, fast=True)
        # The NEAR nearest surfaces of each point, nearest first; a face outside them is at least
        # as far as the last of them.
        near = min(self.NEAR, d.shape[1])
        scratch = d.copy()
        rows_all = np.arange(n)
        columns = np.empty((n, near), dtype=np.int64)
        dist = np.empty((n, near))
        for j in range(near):
            column = np.argmin(scratch, axis=1)
            columns[:, j] = column
            dist[:, j] = scratch[rows_all, column]
            scratch[rows_all, column] = np.inf
        faces = ids[columns]
        gap = np.maximum(0.0, np.maximum(self.lo[faces] - block[:, None, :], block[:, None, :] - self.hi[faces]))
        lower = np.maximum(dist, np.sqrt(np.einsum("nkc,nkc->nk", gap, gap)))
        order = np.argsort(lower, axis=1)
        chosen = np.full(n, -1, dtype=np.int64)
        rows_left = rows_all
        for rank in range(min(self.RANKS, near)):
            if len(rows_left) == 0:
                break
            winner = faces[rows_left, order[rows_left, rank]]
            exact = self.exact_pairs(block, rows_left, winner)
            good = ~np.isnan(exact)
            upper[rows_left[good]] = exact[good]
            chosen[rows_left[good]] = winner[good]
            rows_left = rows_left[~good]
        done = np.nonzero(np.isfinite(upper))[0]
        if len(done):
            blocker = np.full(n, np.inf)
            if d.shape[1] > near:
                blocker[done] = dist[done, -1] - FAST_MARGIN
            # Faces that might be nearer than the validated one: anything not ruled out by its
            # closed-form distance or bounding box, with the exact bounds taken for those.
            maybe_rows, maybe_cols = np.nonzero(lower[done] < (upper[done] + FAST_MARGIN)[:, None])
            rows, cols = done[maybe_rows], maybe_cols
            pair_faces = faces[rows, cols]
            keep = pair_faces != chosen[rows]
            rows, pair_faces = rows[keep], pair_faces[keep]
            if len(rows):
                bound = self.face_bounds_pairs(block, rows, pair_faces)
                np.minimum.at(blocker, rows, bound)
            if len(other):
                blocker[done] = np.minimum(blocker[done], box_distance(block[done], self.lo[other], self.hi[other]).min(axis=1))
            certified[done] = upper[done] <= blocker[done] + TIE_TOLERANCE
        return upper, certified

    def strip_bounds_at(self, p):
        """`paired_strip` of one point against every analytic face: (faces,), 0 for the others."""
        out = np.zeros(len(self.faces))
        for kind, arrays in self.arrays.items():
            out[self.ids[kind]] = paired_strip(kind, arrays, np.repeat(p[None], len(self.ids[kind]), axis=0))
        return out

    def reach(self, block):
        """Faces that can be nearest to some point of `block` (n, 3): exact, by the 1-Lipschitz bounds.

        A point's distance to the boundary, and a face's strip bound, change by no more than the
        distance the point moves. Let p0 be a point of the block whose distance d0 is certified and
        R the largest distance from p0 to a point of the block. Every point of the block is within
        d0 + R of the boundary, so an analytic face can only matter if its strip bound at p0 is at
        most d0 + 2R, and any other face only if its bounding box is within d0 + R of the block's box.
        """
        everything = np.arange(len(self.faces))
        probe = block[:: max(1, len(block) // 4)][:4]
        upper, certified = self.bounds(probe)
        if not certified.any():
            return everything
        j = int(np.nonzero(certified)[0][0])
        p0, d0 = probe[j], float(upper[j])
        radius = float(np.linalg.norm(block - p0, axis=1).max())
        keep = np.zeros(len(self.faces), dtype=bool)
        reach_a = d0 + 2.0 * radius + FAST_MARGIN
        keep[self.analytic] = self.strip_bounds_at(p0)[self.analytic] <= reach_a
        other = everything[~self.analytic]
        if len(other):
            gap = np.maximum(0.0, np.maximum(self.lo[other] - block.max(axis=0), block.min(axis=0) - self.hi[other]))
            keep[other[np.linalg.norm(gap, axis=1) <= d0 + radius + FAST_MARGIN]] = True
        return everything[keep]

    def measure_block(self, block):
        """(certified values or NaN, upper bounds): the exact distance of each point that closed form
        settles, and for the others an upper bound (infinity when there is none)."""
        upper, certified = self.bounds(block, self.reach(block))
        values = np.where(certified, upper, np.nan)
        return values, np.where(certified, np.nan, upper)


# ---------------------------------------------------------------------------
# The measurement over all solids, in worker processes (fork: the faces are shared copy-on-write).

_STATE: dict = {}


def _order_task(k):
    return morton_order(_STATE["points"][k])


def _closed_form_task(job):
    k, b = job
    solid, points = _STATE["solids"][k], _STATE["points"][k]
    indices = _STATE["blocks"][k][b]
    block = points[indices]
    values, upper = solid.measure_block(block)
    done = ~np.isnan(values)
    worst = at = None
    if done.any():
        index = int(np.nanargmax(values))
        worst, at = float(values[index]), block[index].tolist()
    left = np.nonzero(~done)[0]
    return {"solid": k, "certified": int(done.sum()), "worst": worst, "at": at, "left": indices[left], "upper": upper[left]}


def _occt_task(job):
    k, indices, floor = job
    solid, points = _STATE["solids"][k], _STATE["points"][k]
    worst, at, calls = floor, None, 0
    for i in indices:
        value, made = solid.point_distance(points[i], floor=worst)
        calls += made
        if value > worst:
            worst, at = value, points[i].tolist()
    return {"solid": k, "worst": worst, "at": at, "calls": calls, "points": len(indices)}


def measure(solids, point_sets, workers=3, occt_points=2000, block=512):
    """Deviation of each solid's sample points from its boundary.

    `solids` are `SolidFaces`, `point_sets` the (n, 3) sample points in shape units. Each solid's
    points are put in Morton order and cut into blocks of `block`, so a block is a compact cluster
    and few faces can reach it. Returns `(rows, timing)`; `rows` has one dict per solid: the worst distance and where, and how many points were settled in closed
    form, how many needed OpenCASCADE, how many of those were measured, and how many there were.

    A point is settled in closed form when its distance is certified exactly, or when a distance
    to one of the solid's faces (an upper bound) is already no larger than the largest certified
    distance of the solid: it cannot raise the maximum. The rest need OpenCASCADE.
    """
    import multiprocessing
    import time

    timing = {}
    clock = time.monotonic()
    context = multiprocessing.get_context("fork")
    _STATE["points"] = list(point_sets)
    with context.Pool(max(1, workers)) as pool:
        orders = pool.map(_order_task, range(len(point_sets)))
    ordered = [p[o] for p, o in zip(point_sets, orders)]
    blocks = [[np.arange(s, min(s + block, len(p))) for s in range(0, len(p), block)] for p in ordered]
    _STATE["solids"], _STATE["points"], _STATE["blocks"] = solids, ordered, blocks
    timing["order"] = time.monotonic() - clock
    clock = time.monotonic()
    rows = [
        {"points_available": len(p), "points_closed_form": 0, "points_occt": 0, "points_occt_needed": 0,
         "worst": 0.0, "at": None, "occt_calls": 0}
        for p in ordered
    ]
    tasks = [(k, b) for k, bs in enumerate(blocks) for b in range(len(bs))]
    left = [[] for _ in ordered]
    upper = [[] for _ in ordered]
    with context.Pool(max(1, workers)) as pool:
        for answer in pool.imap_unordered(_closed_form_task, tasks, chunksize=4):
            k = answer["solid"]
            rows[k]["points_closed_form"] += answer["certified"]
            if answer["worst"] is not None and answer["worst"] > rows[k]["worst"]:
                rows[k]["worst"], rows[k]["at"] = answer["worst"], answer["at"]
            left[k].append(answer["left"])
            upper[k].append(answer["upper"])
        timing["closed_form"] = time.monotonic() - clock
        clock = time.monotonic()
        jobs = []
        for k in range(len(ordered)):
            index = np.concatenate(left[k]) if left[k] else np.empty(0, dtype=np.int64)
            bound = np.concatenate(upper[k]) if upper[k] else np.empty(0)
            settled = bound <= rows[k]["worst"]
            rows[k]["points_closed_form"] += int(settled.sum())
            need = np.sort(index[~settled])
            rows[k]["points_occt_needed"] = int(len(need))
            if occt_points and len(need) > occt_points:
                need = need[np.linspace(0, len(need) - 1, occt_points).astype(np.int64)]
            rows[k]["points_occt"] = int(len(need))
            for s in range(0, len(need), 8):
                jobs.append((k, need[s : s + 8], rows[k]["worst"]))
        for answer in pool.imap_unordered(_occt_task, jobs, chunksize=1):
            k = answer["solid"]
            rows[k]["occt_calls"] += answer["calls"]
            if answer["at"] is not None and answer["worst"] > rows[k]["worst"]:
                rows[k]["worst"], rows[k]["at"] = answer["worst"], answer["at"]
    timing["occt"] = time.monotonic() - clock
    for row in rows:
        row["points"] = row["points_closed_form"] + row["points_occt"]
    return rows, timing
