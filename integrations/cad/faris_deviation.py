# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
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
them. Otherwise the result is a "sampled maximum". Triangle coverage uses vertex-distance
bounds for verified convex planar faces, topology-verified planar disks and single-hole convex
regions, and topology-proven complete finite cylinders and tori. Remaining triangles use the
1-Lipschitz CAD-distance bound. Uncertain trims stay on the conservative path.
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


def is_convex_polygon_2d(polygon, tolerance):
    """Conservative convexity check using an absolute signed-distance tolerance."""
    p = np.asarray(polygon, dtype=np.float64)
    if p.ndim != 2 or p.shape[1] != 2 or len(p) < 3 or not np.isfinite(p).all():
        return False
    cross2 = lambda a, b: float(a[0] * b[1] - a[1] * b[0])
    area2 = sum(cross2(p[i], p[(i+1) % len(p)]) for i in range(len(p)))
    perimeter = sum(float(np.linalg.norm(p[(i+1) % len(p)] - p[i])) for i in range(len(p)))
    if abs(area2) <= tolerance * perimeter:
        return False
    orientation = 1.0 if area2 > 0 else -1.0
    for i in range(len(p)):
        edge = p[(i+1) % len(p)] - p[i]
        if np.linalg.norm(edge) <= tolerance:
            return False
        edge_length = float(np.linalg.norm(edge))
        if any(orientation * cross2(edge, point-p[i]) / edge_length < -tolerance for point in p):
            return False
    return True


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
    # A full-period surface has identical start and end angles. Modulo therefore
    # turns its sweep into zero; treat that as the full circle so points are not
    # incorrectly bounded by the seam endpoint.
    full_period = np.abs(a1 - a0) >= TWO_PI - 1.0e-10
    inside = full_period | ((np.mod(angle - a0, TWO_PI) <= span) & (span > 0.0))
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
        self._convex_planar_faces = None
        self._planar_region_faces = None
        self._full_torus_faces = None
        self._full_cylinder_faces = None
        self.measured_witness_distance = None

    def full_torus_faces(self):
        """Return torus parameters only for faces proven to be the whole periodic surface.

        A full UV range alone is not enough: trims can include holes or notches. The accepted
        topology is one wire made from exactly two distinct periodic seam edges, both closed
        on the face, plus full canonical U and V periods.
        """
        if self._full_torus_faces is not None:
            return self._full_torus_faces
        from OCP.BRep import BRep_Tool
        from OCP.BRepCheck import BRepCheck_Analyzer
        from OCP.BRepGProp import BRepGProp
        from OCP.GProp import GProp_GProps
        from OCP.TopAbs import TopAbs_EDGE, TopAbs_WIRE
        from OCP.TopExp import TopExp_Explorer
        from OCP.TopoDS import TopoDS

        result = []
        period_slack = 1.0e-8
        for i, face in enumerate(self.faces):
            if self.kind[i] != "torus":
                continue
            if not BRepCheck_Analyzer(face).IsValid():
                continue
            surface_props = GProp_GProps()
            BRepGProp.SurfaceProperties_s(face, surface_props)
            area = surface_props.Mass()
            if not math.isfinite(area) or area <= 0.0:
                continue
            major, minor = self.params[i]["major"], self.params[i]["minor"]
            if not (math.isfinite(major) and math.isfinite(minor) and major > minor > 0.0):
                continue
            u0, u1, v0, v1 = (self.params[i][key] for key in ("u0", "u1", "v0", "v1"))
            if (not all(math.isfinite(x) for x in (u0, u1, v0, v1))
                    or abs((u1-u0) - TWO_PI) > period_slack
                    or abs((v1-v0) - TWO_PI) > period_slack):
                continue
            wires = TopExp_Explorer(face, TopAbs_WIRE)
            if not wires.More():
                continue
            wire = wires.Current()
            wires.Next()
            if wires.More():
                continue
            edges = TopExp_Explorer(wire, TopAbs_EDGE)
            unique_edges = []
            edge_uses = []
            valid = True
            while edges.More():
                edge = TopoDS.Edge_s(edges.Current())
                if not BRep_Tool.IsClosed_s(edge, face):
                    valid = False
                    break
                edge_uses.append(edge)
                if not any(edge.IsSame(prior) for prior in unique_edges):
                    unique_edges.append(edge)
                edges.Next()
            # A torus fundamental rectangle has two seam curves, each used twice
            # in the wire. Anything else can encode an unrecognized trim.
            if (valid and len(edge_uses) == 4 and len(unique_edges) == 2
                    and all(sum(edge.IsSame(candidate) for candidate in edge_uses) == 2
                            for edge in unique_edges)):
                result.append(self.params[i])
        self._full_torus_faces = result
        return result

    def full_cylinder_faces(self):
        """Return finite cylinder faces proven to be complete axial strips.

        Accepted topology is one valid wire with exactly four line pcurves: two full-U
        end rims at v0/v1 and two closed-on-face seam sides at u0/u1 spanning v0/v1.
        This rejects partial angular trims, extra wires/holes, and non-axial trims. For a
        triangle whose axial projection lies in [v0, v1], projection onto the full periodic
        cylinder stays on the face and its exact face distance is abs(rho-r).
        """
        if self._full_cylinder_faces is not None:
            return self._full_cylinder_faces
        from OCP.BRep import BRep_Tool
        from OCP.BRepCheck import BRepCheck_Analyzer
        from OCP.Geom2dAdaptor import Geom2dAdaptor_Curve
        from OCP.GeomAbs import GeomAbs_Line
        from OCP.TopAbs import TopAbs_EDGE, TopAbs_WIRE
        from OCP.TopExp import TopExp_Explorer
        from OCP.TopoDS import TopoDS

        result = []
        uv_slack = 1.0e-8
        for i, face in enumerate(self.faces):
            if self.kind[i] != "cylinder" or not BRepCheck_Analyzer(face).IsValid():
                continue
            params = self.params[i]
            u0, u1, v0, v1 = (params[key] for key in ("u0", "u1", "v0", "v1"))
            radius = params["r"]
            if (not all(math.isfinite(x) for x in (u0, u1, v0, v1, radius))
                    or radius <= 0.0 or v1 <= v0
                    or abs((u1-u0)-TWO_PI) > uv_slack):
                continue
            wires = TopExp_Explorer(face, TopAbs_WIRE)
            if not wires.More():
                continue
            wire = wires.Current()
            wires.Next()
            if wires.More():
                continue
            edges = TopExp_Explorer(wire, TopAbs_EDGE)
            seam_count = rim_count = edge_count = 0
            valid = True
            while edges.More():
                edge = TopoDS.Edge_s(edges.Current())
                edge_count += 1
                is_seam = bool(BRep_Tool.IsClosed_s(edge, face))
                first, last = BRep_Tool.Range_s(edge, face)
                curve = BRep_Tool.CurveOnSurface_s(edge, face, 0.0, 0.0)
                if curve is None or not (math.isfinite(first) and math.isfinite(last) and last > first):
                    valid = False
                    break
                adaptor = Geom2dAdaptor_Curve(curve, first, last)
                if adaptor.GetType() != GeomAbs_Line:
                    valid = False
                    break
                a, b = curve.Value(first), curve.Value(last)
                u_a, v_a, u_b, v_b = float(a.X()), float(a.Y()), float(b.X()), float(b.Y())
                if not all(math.isfinite(x) for x in (u_a, v_a, u_b, v_b)):
                    valid = False
                    break
                if is_seam:
                    on_period_edge = (abs(u_a-u0) <= uv_slack and abs(u_b-u0) <= uv_slack
                                      or abs(u_a-u1) <= uv_slack and abs(u_b-u1) <= uv_slack)
                    if not (on_period_edge and abs(min(v_a, v_b)-v0) <= uv_slack
                            and abs(max(v_a, v_b)-v1) <= uv_slack):
                        valid = False
                        break
                    seam_count += 1
                else:
                    on_end_circle = (abs(v_a-v0) <= uv_slack and abs(v_b-v0) <= uv_slack
                                     or abs(v_a-v1) <= uv_slack and abs(v_b-v1) <= uv_slack)
                    if not (on_end_circle and abs(min(u_a, u_b)-u0) <= uv_slack
                            and abs(max(u_a, u_b)-u1) <= uv_slack):
                        valid = False
                        break
                    rim_count += 1
                edges.Next()
            if valid and edge_count == 4 and seam_count == 2 and rim_count == 2:
                result.append(params)
        self._full_cylinder_faces = result
        return result

    def convex_planar_faces(self):
        """Return verified single-wire convex planar faces as (origin, normal, polygon).

        Unsupported topology is omitted. In particular, holes, curved edges, nonplanarity,
        degenerate turns, and ambiguous vertex order never enter the fast bound.
        """
        if self._convex_planar_faces is not None:
            return self._convex_planar_faces
        from OCP.BRepAdaptor import BRepAdaptor_Curve
        from OCP.GeomAbs import GeomAbs_Line
        from OCP.TopAbs import TopAbs_EDGE, TopAbs_VERTEX, TopAbs_WIRE
        from OCP.TopExp import TopExp_Explorer
        from OCP.BRep import BRep_Tool
        from OCP.TopoDS import TopoDS

        result = []
        # Treat line and vertex geometry as accurate within this shape-unit kernel slack.
        # Uncertain trims are omitted from the fast path.
        eps = max(CLASSIFY_TOLERANCE, FAST_MARGIN)
        for i, face in enumerate(self.faces):
            if self.kind[i] != "plane":
                continue
            wires = TopExp_Explorer(face, TopAbs_WIRE)
            if not wires.More():
                continue
            wire = wires.Current()
            wires.Next()
            if wires.More():  # reject inner wires/holes
                continue
            edges = TopExp_Explorer(wire, TopAbs_EDGE)
            segments = []
            valid = True
            while edges.More():
                edge = TopoDS.Edge_s(edges.Current())
                curve = BRepAdaptor_Curve(edge)
                if curve.GetType() != GeomAbs_Line:
                    valid = False
                    break
                vertices = TopExp_Explorer(edge, TopAbs_VERTEX)
                pts = []
                while vertices.More():
                    p = BRep_Tool.Pnt_s(TopoDS.Vertex_s(vertices.Current()))
                    pts.append(_pnt(p))
                    vertices.Next()
                if len(pts) != 2 or np.linalg.norm(pts[0] - pts[1]) <= eps:
                    valid = False
                    break
                segments.append((pts[0], pts[1]))
                edges.Next()
            if not valid or len(segments) < 3:
                continue
            # Assemble the wire by endpoint connectivity; orientation from the explorer is
            # deliberately not trusted because reversed edges are common in B-rep wires.
            polygon = [segments[0][0], segments[0][1]]
            remaining = list(segments[1:])
            current = polygon[-1]
            while remaining:
                matches = [(j, b if np.linalg.norm(a-current) <= eps else a)
                           for j, (a, b) in enumerate(remaining)
                           if min(np.linalg.norm(a-current), np.linalg.norm(b-current)) <= eps]
                if len(matches) != 1:
                    valid = False
                    break
                j, nxt = matches[0]
                remaining.pop(j)
                if np.linalg.norm(nxt - polygon[0]) <= eps:
                    if remaining:
                        valid = False
                    break
                polygon.append(nxt)
                current = nxt
            if not valid or len(polygon) < 3:
                continue
            p = np.asarray(polygon)
            params = self.params[i]
            origin, normal = params["o"], params["z"]
            if np.max(np.abs((p - origin) @ normal)) > eps:
                continue
            # Establish a local orthonormal frame and check every vertex against every edge.
            u = p[1] - p[0]
            u /= np.linalg.norm(u)
            v = np.cross(normal, u)
            xy = np.stack(((p-origin) @ u, (p-origin) @ v), axis=1)
            # Every polygon vertex must lie in the same closed half-plane of every edge.
            roundoff = 64.0 * np.finfo(np.float64).eps * max(1.0, float(np.abs(p).max()), float(np.abs(origin).max()))
            if not is_convex_polygon_2d(xy, roundoff):
                continue
            result.append((origin, normal, p))
        self._convex_planar_faces = result
        return result

    def planar_region_faces(self):
        """Return strictly validated planar disks and single-hole convex planar regions.

        A region is accepted only when OpenCASCADE validates the face and its complete wire
        topology is either one full circle, one convex line polygon, or an outer convex line
        polygon with exactly one strictly-contained circular or convex polygonal inner wire.
        Multiple holes, curved/nonconvex polygon loops, and ambiguous containment are omitted.
        """
        if self._planar_region_faces is not None:
            return self._planar_region_faces
        from OCP.BRep import BRep_Tool
        from OCP.BRepAdaptor import BRepAdaptor_Curve
        from OCP.BRepCheck import BRepCheck_Analyzer
        from OCP.GeomAbs import GeomAbs_Circle, GeomAbs_Line
        from OCP.TopAbs import TopAbs_EDGE, TopAbs_VERTEX, TopAbs_WIRE
        from OCP.TopExp import TopExp_Explorer
        from OCP.TopoDS import TopoDS

        result = []
        eps = max(CLASSIFY_TOLERANCE, FAST_MARGIN)
        period_slack = 1.0e-8

        def polygon_from_wire(wire, origin, x_axis, y_axis):
            edges = TopExp_Explorer(wire, TopAbs_EDGE)
            segments = []
            while edges.More():
                edge = TopoDS.Edge_s(edges.Current())
                curve = BRepAdaptor_Curve(edge)
                if curve.GetType() != GeomAbs_Line:
                    return None
                vertices = TopExp_Explorer(edge, TopAbs_VERTEX)
                points = []
                while vertices.More():
                    point = BRep_Tool.Pnt_s(TopoDS.Vertex_s(vertices.Current()))
                    points.append(_pnt(point))
                    vertices.Next()
                if len(points) != 2 or np.linalg.norm(points[0]-points[1]) <= eps:
                    return None
                if max(abs(float(np.dot(points[j]-origin, np.cross(x_axis,y_axis)))) for j in range(2)) > eps:
                    return None
                a = np.array([np.dot(points[0]-origin,x_axis), np.dot(points[0]-origin,y_axis)])
                b = np.array([np.dot(points[1]-origin,x_axis), np.dot(points[1]-origin,y_axis)])
                segments.append((a,b))
                edges.Next()
            if len(segments) < 3:
                return None
            polygon = [segments[0][0], segments[0][1]]
            remaining = list(segments[1:])
            while remaining:
                current = polygon[-1]
                matches = [(j, b if np.linalg.norm(a-current) <= eps else a)
                           for j,(a,b) in enumerate(remaining)
                           if min(np.linalg.norm(a-current),np.linalg.norm(b-current)) <= eps]
                if len(matches) != 1:
                    return None
                j, nxt = matches[0]
                remaining.pop(j)
                if np.linalg.norm(nxt-polygon[0]) <= eps:
                    if remaining:
                        return None
                    break
                polygon.append(nxt)
            p=np.asarray(polygon,dtype=np.float64)
            if len(p)<3 or not _strict_convex_polygon_2d(p):
                return None
            return p

        def circle_from_wire(wire, origin, normal, x_axis, y_axis):
            edges=TopExp_Explorer(wire,TopAbs_EDGE)
            if not edges.More():
                return None
            edge=TopoDS.Edge_s(edges.Current()); edges.Next()
            if edges.More():
                return None
            curve=BRepAdaptor_Curve(edge)
            if curve.GetType()!=GeomAbs_Circle:
                return None
            first,last=curve.FirstParameter(),curve.LastParameter()
            if not all(math.isfinite(x) for x in (first,last)) or abs((last-first)-TWO_PI)>period_slack:
                return None
            circle=curve.Circle(); center=circle.Location(); axis=circle.Axis().Direction()
            c3=_pnt(center); n3=_vec(axis)
            if abs(abs(float(np.dot(n3,normal)))-1.0)>1e-10:
                return None
            if abs(float(np.dot(c3-origin,normal)))>eps:
                return None
            c2=np.array([np.dot(c3-origin,x_axis),np.dot(c3-origin,y_axis)],dtype=np.float64)
            radius=float(circle.Radius())
            return {"center":c2,"radius":radius} if math.isfinite(radius) and radius>eps else None

        def area2(poly):
            return sum(float(poly[i,0]*poly[(i+1)%len(poly),1]-poly[i,1]*poly[(i+1)%len(poly),0])
                       for i in range(len(poly)))

        for i,face in enumerate(self.faces):
            if self.kind[i]!="plane" or not BRepCheck_Analyzer(face).IsValid():
                continue
            params=self.params[i]
            origin,normal,x_axis,y_axis=(params[k] for k in ("o","z","x","y"))
            if not all(np.isfinite(params[k]).all() for k in ("o","z","x","y")):
                continue
            if any(abs(float(np.linalg.norm(params[k]))-1.0)>1e-10 for k in ("z","x","y")):
                continue
            if max(abs(float(np.dot(x_axis,y_axis))),abs(float(np.dot(x_axis,normal))),abs(float(np.dot(y_axis,normal))))>1e-10:
                continue
            wires=TopExp_Explorer(face,TopAbs_WIRE)
            wire_list=[]
            while wires.More():
                wire_list.append(wires.Current()); wires.Next()
            if len(wire_list) not in (1,2):
                continue
            loops=[]
            for wire in wire_list:
                circle=circle_from_wire(wire,origin,normal,x_axis,y_axis)
                if circle is not None:
                    loops.append(("circle",circle))
                    continue
                polygon=polygon_from_wire(wire,origin,x_axis,y_axis)
                if polygon is None:
                    loops=[]; break
                loops.append(("polygon",polygon))
            if len(loops)!=len(wire_list):
                continue
            if len(loops)==1:
                kind,loop=loops[0]
                if kind=="circle":
                    result.append({"kind":"disk","o":origin,"z":normal,"x":x_axis,"y":y_axis,
                                   "center":loop["center"],"radius":loop["radius"]})
                else:
                    result.append({"kind":"polygon","o":origin,"z":normal,"x":x_axis,"y":y_axis,
                                   "outer":loop})
                continue
            # Two wires: select the unique larger outer loop by area, then prove strict
            # containment. Equal/ambiguous cases and circle-circle annuli are conservatively omitted.
            areas=[(math.pi*loop["radius"]**2 if kind=="circle" else abs(area2(loop)))
                   for kind,loop in loops]
            if abs(areas[0]-areas[1])<=eps*max(1.0,*areas):
                continue
            oi=0 if areas[0]>areas[1] else 1; hi=1-oi
            outer_kind,outer=loops[oi]; hole_kind,hole=loops[hi]
            if outer_kind!="polygon":
                continue
            outer_orientation=1.0 if area2(outer)>0 else -1.0
            contained=True
            if hole_kind=="circle":
                center=hole["center"]; radius=hole["radius"]
                signed, errors = _polygon_signed_edge_distances_with_error(center, outer)
                if np.any(signed-errors<=radius+eps):
                    contained=False
                if contained:
                    result.append({"kind":"polygon_circle_hole","o":origin,"z":normal,"x":x_axis,"y":y_axis,
                                   "outer":outer,"hole":{"center":center,"radius":radius}})
            else:
                for q in hole:
                    signed, errors = _polygon_signed_edge_distances_with_error(q,outer)
                    if not np.isfinite(signed).all() or not np.isfinite(errors).all() or np.min(signed-errors)<=eps:
                        contained=False; break
                if contained:
                    result.append({"kind":"polygon_polygon_hole","o":origin,"z":normal,"x":x_axis,"y":y_axis,
                                   "outer":outer,"hole":{"polygon":hole}})
        self._planar_region_faces=result
        return result

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
    # Preserve only an attained exact distance as a witness. Closed-form values
    # were face-certified; OCCT values were measured to the complete solid boundary.
    for solid, row in zip(solids, rows):
        at = row["at"]
        value = row["worst"]
        solid.measured_witness_distance = (
            float(value) if at is not None and math.isfinite(value)
            and len(at) == 3 and all(math.isfinite(float(x)) for x in at) else None
        )
    return rows, timing


def _point_segment_distance_2d(p, a, b):
    ab = b - a
    den = float(np.dot(ab, ab))
    t = 0.0 if den == 0.0 else min(1.0, max(0.0, float(np.dot(p-a, ab) / den)))
    return float(np.linalg.norm(p - (a + t * ab)))


def convex_planar_triangle_bound(triangle, face):
    """Upper bound distance from a triangle to one verified convex planar polygon.

    `face` is (origin, unit normal, ordered 3D polygon vertices). The projection of each
    triangle vertex is measured to the polygon (including its edges). Convexity makes the
    maximum of these three attained vertex distances an upper bound throughout the triangle:
    convex combinations of the three closest polygon points remain on the face.
    """
    origin, normal, polygon = face
    triangle = np.asarray(triangle, dtype=np.float64)
    polygon = np.asarray(polygon, dtype=np.float64)
    # Build a 2D frame from the polygon itself; all calculations retain the normal offset.
    u = polygon[1] - polygon[0]
    u /= np.linalg.norm(u)
    v = np.cross(normal, u)
    poly2 = np.stack(((polygon-origin) @ u, (polygon-origin) @ v), axis=1)
    cross2 = lambda a, b: float(a[0] * b[1] - a[1] * b[0])
    area2 = sum(cross2(poly2[i], poly2[(i+1) % len(poly2)]) for i in range(len(poly2)))
    orientation = 1.0 if area2 > 0 else -1.0
    roundoff = 64.0 * np.finfo(np.float64).eps * max(
        1.0, float(np.abs(origin).max()), float(np.abs(polygon).max()), float(np.abs(triangle).max()))
    bounds = []
    for point in triangle:
        rel = point - origin
        h = float(np.dot(rel, normal))
        q = rel - h * normal
        q2 = np.array([np.dot(q, u), np.dot(q, v)])
        inside = all(
            orientation * cross2(poly2[(i+1) % len(poly2)]-poly2[i], q2-poly2[i])
            / np.linalg.norm(poly2[(i+1) % len(poly2)]-poly2[i]) >= -roundoff
            for i in range(len(poly2))
        )
        d2 = 0.0 if inside else min(_point_segment_distance_2d(q2, poly2[i], poly2[(i+1) % len(poly2)])
                                    for i in range(len(poly2)))
        bounds.append(math.hypot(h, d2))
    scale = max(1.0, float(np.abs(triangle).max()), float(np.abs(polygon).max()))
    kernel_slack = max(CLASSIFY_TOLERANCE, FAST_MARGIN)
    return max(bounds) + kernel_slack + 64.0 * np.finfo(np.float64).eps * scale


def _polygon_orientation(polygon):
    p = np.asarray(polygon, dtype=np.float64)
    area2 = math.fsum(float(p[i, 0] * p[(i+1) % len(p), 1] - p[i, 1] * p[(i+1) % len(p), 0])
                      for i in range(len(p)))
    return 1.0 if area2 >= 0.0 else -1.0


def _polygon_signed_edge_distances_with_error(point, polygon):
    p = np.asarray(polygon, dtype=np.float64)
    point = np.asarray(point, dtype=np.float64)
    orientation = _polygon_orientation(p)
    values, errors = [], []
    eps = np.finfo(np.float64).eps
    for i in range(len(p)):
        a, b = p[i], p[(i+1) % len(p)]
        edge = b-a
        length = float(np.linalg.norm(edge))
        scale = max(1.0, float(np.abs(a).max()), float(np.abs(b).max()), float(np.abs(point).max()))
        length_lo = length - 8.0 * eps * scale
        if not math.isfinite(length_lo) or length_lo <= 0.0:
            return np.full(len(p), -math.inf), np.full(len(p), math.inf)
        cross = float(edge[0] * (point[1]-a[1]) - edge[1] * (point[0]-a[0]))
        values.append(orientation * cross / length)
        # Covers subtraction, products, determinant accumulation, and normalization error;
        # division by a short edge is explicit instead of hidden in a global coordinate slack.
        errors.append(128.0 * eps * scale * scale / length_lo)
    return np.asarray(values,dtype=np.float64), np.asarray(errors,dtype=np.float64)


def _polygon_signed_edge_distances(point, polygon):
    return _polygon_signed_edge_distances_with_error(point, polygon)[0]


def _strict_convex_polygon_2d(polygon):
    p=np.asarray(polygon,dtype=np.float64)
    if p.ndim!=2 or p.shape[1]!=2 or len(p)<3 or not np.isfinite(p).all():
        return False
    scale=max(1.0,float(np.abs(p).max()))
    eps=np.finfo(np.float64).eps
    area2=math.fsum(float(p[i,0]*p[(i+1)%len(p),1]-p[i,1]*p[(i+1)%len(p),0]) for i in range(len(p)))
    if abs(area2)<=128.0*eps*scale*scale*len(p):
        return False
    for i in range(len(p)):
        a,b=p[i],p[(i+1)%len(p)]
        edge_length=float(np.linalg.norm(b-a))
        if edge_length<=128.0*eps*scale:
            return False
        # Check only nonadjacent vertices; adjacent endpoints define this edge.
        for j,q in enumerate(p):
            if j in (i,(i+1)%len(p)):
                continue
            edge=b-a
            cross=float(edge[0]*(q[1]-a[1])-edge[1]*(q[0]-a[0]))
            inward=(1.0 if area2>0.0 else -1.0)*cross/edge_length
            err=128.0*eps*scale*scale/edge_length
            if not math.isfinite(inward) or inward<=err:
                return False
    return True


def _point_in_convex_polygon_2d(point, polygon, slack=0.0):
    distances, errors = _polygon_signed_edge_distances_with_error(point, polygon)
    return bool(np.isfinite(distances).all() and np.isfinite(errors).all()
                and np.all(distances >= errors + slack))


def _point_convex_polygon_distance_2d(point, polygon, slack=0.0):
    p = np.asarray(polygon, dtype=np.float64)
    if _point_in_convex_polygon_2d(point, p, slack=slack):
        return 0.0
    # In the uncertain/outer case, segment distances are upper candidates. Inflate each
    # candidate by its coordinate/edge-length error before taking the minimum.
    return min(_point_segment_distance_2d(point, p[i], p[(i+1) % len(p)])
               + _polygon_signed_edge_distances_with_error(point, p)[1][i] + slack
               for i in range(len(p)))


def _point_triangle_distance_2d(point, triangle, roundoff):
    """Conservative lower bound from point to a projected triangle.

    Return zero unless one half-plane violation is proven outside after per-edge error
    inflation. In the proven-outside case, subtract a conditioned segment-distance error
    from each edge candidate before taking their minimum.
    """
    tri=np.asarray(triangle,dtype=np.float64)
    point=np.asarray(point,dtype=np.float64)
    scale=max(1.0,float(np.abs(tri).max()),float(np.abs(point).max()))
    eps=np.finfo(np.float64).eps
    area2=math.fsum(float(tri[i,0]*tri[(i+1)%3,1]-tri[i,1]*tri[(i+1)%3,0]) for i in range(3))
    area_error=128.0*eps*scale*scale*3.0+6.0*roundoff*scale
    if abs(area2)<=area_error:
        return 0.0
    orientation=1.0 if area2>0.0 else -1.0
    signed=[]; errors=[]; lengths=[]
    for i in range(3):
        a,b=tri[i],tri[(i+1)%3]
        edge=b-a
        length=float(np.linalg.norm(edge))
        length_lo=length-8.0*eps*scale
        if not math.isfinite(length_lo) or length_lo<=0.0:
            return 0.0
        cross=float(edge[0]*(point[1]-a[1])-edge[1]*(point[0]-a[0]))
        signed.append(orientation*cross/length)
        errors.append(roundoff+128.0*eps*scale*scale/length_lo)
        lengths.append(length_lo)
    # If no edge is definitely violated, the center may be inside; zero is a safe lower
    # bound and avoids a false positive radial minimum near acute or short projected edges.
    if not any(signed[i]+errors[i] < 0.0 for i in range(3)):
        return 0.0
    segment_lowers=[]
    for i in range(3):
        d=_point_segment_distance_2d(point,tri[i],tri[(i+1)%3])
        err=roundoff+128.0*eps*scale*scale/lengths[i]
        segment_lowers.append(max(0.0,float(np.nextafter(d-err,-np.inf))))
    return min(segment_lowers)



def planar_region_triangle_bound(triangle, region, extra_slack=0.0):
    """Sound upper bound from a triangle to a verified planar disk or one-hole convex region.

    Regions use a local orthonormal frame and are one of: a filled disk; a convex polygon;
    a convex polygon with one strictly-contained circular disk removed; or a convex polygon
    with one strictly-contained convex polygon removed. For a hole, distance from the region
    is bounded by the max of distance outside the convex outer polygon and distance to the
    hole boundary. The latter uses exact triangle-to-circle-center distance or affine signed
    edge-distance maxima. Unsupported/invalid records return infinity.
    """
    tri = np.asarray(triangle, dtype=np.float64)
    if tri.shape != (3, 3) or not np.isfinite(tri).all():
        return math.inf
    try:
        origin = np.asarray(region["o"], dtype=np.float64)
        normal = np.asarray(region["z"], dtype=np.float64)
        x_axis = np.asarray(region["x"], dtype=np.float64)
        y_axis = np.asarray(region["y"], dtype=np.float64)
        margin = float(extra_slack)
        outer = np.asarray(region.get("outer", []), dtype=np.float64)
        kind = region["kind"]
    except (KeyError, TypeError, ValueError):
        return math.inf
    if (origin.shape != (3,) or normal.shape != (3,) or x_axis.shape != (3,) or y_axis.shape != (3,)
            or not all(np.isfinite(v).all() for v in (origin, normal, x_axis, y_axis, outer))
            or not math.isfinite(margin) or margin < 0.0):
        return math.inf
    if (any(abs(float(np.linalg.norm(v))-1.0)>1e-10 for v in (normal,x_axis,y_axis))
            or any(abs(float(np.dot(a,b)))>1e-10 for a,b in ((normal,x_axis),(normal,y_axis),(x_axis,y_axis)))):
        return math.inf
    if kind not in ("disk", "polygon", "polygon_circle_hole", "polygon_polygon_hole"):
        return math.inf
    if kind != "disk" and (outer.ndim != 2 or outer.shape[1] != 2 or len(outer) < 3
                            or not _strict_convex_polygon_2d(outer)):
        return math.inf
    eps = np.finfo(np.float64).eps
    hole_data = region.get("hole", {})
    if isinstance(hole_data, dict):
        hole_scale = max((float(np.abs(np.asarray(v,dtype=np.float64)).max())
                          for v in hole_data.values() if np.asarray(v).size), default=0.0)
    else:
        hole_scale = 0.0
    scale = max(1.0, float(np.abs(tri).max()), float(np.abs(origin).max()),
                float(np.abs(outer).max()) if outer.size else 0.0, hole_scale)
    roundoff = 128.0 * eps * scale
    rel = tri-origin
    height = rel @ normal
    projected = np.stack((rel @ x_axis, rel @ y_axis), axis=1)
    if kind == "disk":
        center = np.asarray(region.get("center"), dtype=np.float64)
        radius = float(region.get("radius", math.nan))
        if center.shape != (2,) or not np.isfinite(center).all() or not math.isfinite(radius) or radius <= 0.0:
            return math.inf
        distances = []
        for h, q in zip(height, projected):
            rho = float(np.linalg.norm(q-center))
            distances.append(math.hypot(float(h), max(0.0, rho-radius)))
        return float(np.nextafter(max(distances) + max(CLASSIFY_TOLERANCE, FAST_MARGIN)
                                  + roundoff + margin, np.inf))
    outer_d = max(_point_convex_polygon_distance_2d(q, outer, slack=0.0) for q in projected)
    hole = region.get("hole")
    if kind == "polygon_circle_hole":
        center = np.asarray(hole.get("center"), dtype=np.float64)
        radius = float(hole.get("radius", math.nan))
        if center.shape != (2,) or not np.isfinite(center).all() or not math.isfinite(radius) or radius <= 0.0:
            return math.inf
        outer_signed, outer_errors = _polygon_signed_edge_distances_with_error(center, outer)
        if not np.isfinite(outer_signed).all() or not np.isfinite(outer_errors).all() \
                or np.min(outer_signed-outer_errors) <= radius + roundoff:
            return math.inf
        rho_min = _point_triangle_distance_2d(center, projected, roundoff)
        hole_gap = max(0.0, radius-rho_min)
    elif kind == "polygon_polygon_hole":
        inner = np.asarray(hole.get("polygon"), dtype=np.float64)
        if (inner.ndim != 2 or inner.shape[1] != 2 or len(inner) < 3 or not np.isfinite(inner).all()
                or not _strict_convex_polygon_2d(inner)):
            return math.inf
        for q in inner:
            signed, errors = _polygon_signed_edge_distances_with_error(q, outer)
            if not np.isfinite(signed).all() or not np.isfinite(errors).all() \
                    or np.min(signed-errors) <= roundoff:
                return math.inf
        # For each affine inward edge distance, its maximum over the triangle is at a vertex.
        # max_T min_edge d <= min_edge max_T d, a sound upper bound on hole clearance.
        edge_maxima = []
        for i in range(len(inner)):
            vals=[]
            for q in projected:
                signed, errors = _polygon_signed_edge_distances_with_error(q, inner)
                vals.append(signed[i]+errors[i])
            edge_maxima.append(float(max(vals)))
        hole_gap = max(0.0, min(edge_maxima))
    else:
        hole_gap = 0.0
    planar_upper = max(outer_d, hole_gap)
    bound = math.hypot(float(np.max(np.abs(height))), planar_upper)
    return float(np.nextafter(bound + max(CLASSIFY_TOLERANCE, FAST_MARGIN) + roundoff + margin, np.inf))


def full_cylinder_triangle_bounds(triangles, params, extra_slack=0.0):
    """Certified upper bounds to topologically complete finite cylinder faces.

    A parameter record is admissible only after topology establishes a complete 2*pi
    angular period with no trims other than the two axial end circles. The distance to
    that finite face is ``hypot(abs(rho-radius), axial_gap)`` where axial_gap is distance
    outside the face's finite V interval. The Hessian norm of rho is at most
    1/rho away from the axis; Taylor interpolation gives the outward-rounded bound below.
    """
    tris = np.asarray(triangles, dtype=np.float64)
    if tris.ndim != 3 or tris.shape[1:] != (3, 3) or not np.isfinite(tris).all():
        return np.full(len(tris) if tris.ndim else 0, np.inf)
    out = np.full(len(tris), np.inf)
    eps = np.finfo(np.float64).eps
    try:
        margins = np.broadcast_to(np.asarray(extra_slack, dtype=np.float64), (len(tris),))
    except ValueError:
        return out
    if not np.isfinite(margins).all() or np.any(margins < 0.0):
        return out
    for face in params:
        origin, axis = np.asarray(face["o"], dtype=np.float64), np.asarray(face["z"], dtype=np.float64)
        radius, v0, v1 = (float(face[k]) for k in ("r", "v0", "v1"))
        if (origin.shape != (3,) or axis.shape != (3,) or not np.isfinite(origin).all()
                or not np.isfinite(axis).all() or not math.isfinite(radius) or radius <= 0.0
                or not all(math.isfinite(x) for x in (v0, v1)) or v1 <= v0):
            continue
        axis_norm = float(np.linalg.norm(axis))
        if not math.isfinite(axis_norm) or abs(axis_norm - 1.0) > 64.0 * eps:
            continue
        rel = tris - origin
        axial = np.einsum("ntk,k->nt", rel, axis)
        perp = rel - axial[..., None] * axis
        rho = np.linalg.norm(perp, axis=2)
        edges = np.stack((tris[:, 0]-tris[:, 1], tris[:, 1]-tris[:, 2], tris[:, 2]-tris[:, 0]), axis=1)
        # rho is constant along the cylinder axis, so use the projected diameter;
        # axial triangle extent is checked separately against the finite face trim.
        projected_edges = edges - np.einsum("nte,e->nt", edges, axis)[..., None] * axis
        diameter = np.linalg.norm(projected_edges, axis=2).max(axis=1)
        scale = np.maximum(1.0, np.maximum(np.abs(tris).max(axis=(1, 2)),
                          np.maximum(float(np.abs(origin).max()), radius)))
        slack = 1024.0 * eps * scale
        diameter_hi = np.nextafter(diameter + slack, np.inf)
        rho_lo = np.nextafter(rho.min(axis=1) - diameter_hi - slack, -np.inf)
        axial_lo = np.nextafter(axial.min(axis=1) - slack, -np.inf)
        axial_hi = np.nextafter(axial.max(axis=1) + slack, np.inf)
        valid = (np.isfinite(rho_lo) & np.isfinite(axial_lo) & np.isfinite(axial_hi)
                 & np.isfinite(diameter_hi) & (rho_lo > 0.0))
        if not valid.any():
            continue
        curvature = np.nextafter(1.0 / rho_lo[valid], np.inf)
        curvature = np.nextafter(curvature + 1024.0 * eps * np.maximum(1.0, curvature), np.inf)
        vertex_deviation = np.nextafter(np.abs(rho - radius).max(axis=1) + slack, np.inf)
        diameter_sq_hi = np.nextafter(diameter_hi[valid] * diameter_hi[valid], np.inf)
        interpolation = np.nextafter(curvature * diameter_sq_hi, np.inf)
        interpolation = np.nextafter(interpolation / 6.0, np.inf)
        radial_bound = np.nextafter(vertex_deviation[valid] + interpolation, np.inf)
        radial_bound = np.nextafter(radial_bound + max(CLASSIFY_TOLERANCE, FAST_MARGIN), np.inf)
        radial_bound = np.nextafter(radial_bound + margins[valid], np.inf)
        axial_gap = np.maximum(np.maximum(v0 - axial_lo[valid], axial_hi[valid] - v1), 0.0)
        axial_gap = np.nextafter(axial_gap + slack[valid], np.inf)
        bound = np.nextafter(np.hypot(radial_bound, axial_gap), np.inf)
        out[valid] = np.minimum(out[valid], bound)
    return out


def full_torus_triangle_bounds(triangles, params, extra_slack=0.0):
    """Certified distance upper bounds for triangles against complete analytic tori.

    On a full torus, boundary distance is ``abs(f-r)`` where
    ``f=hypot(rho-major, axial)``. Away from the axis and from the major circle,
    the Hessian norm of f is bounded by the expression below. Taylor's theorem
    and barycentric variance then bound its deviation from linear interpolation
    on a triangle by K * max_edge**2 / 6. Projection lengths and intermediate
    lower/upper bounds are inflated outwards before computing the curvature bound;
    invalid frames and non-finite intermediates leave the triangle unresolved.
    """
    tris = np.asarray(triangles, dtype=np.float64)
    if tris.ndim != 3 or tris.shape[1:] != (3, 3) or not np.isfinite(tris).all():
        return np.full(len(tris) if tris.ndim else 0, np.inf)
    out = np.full(len(tris), np.inf)
    eps = np.finfo(np.float64).eps
    try:
        margins = np.broadcast_to(np.asarray(extra_slack, dtype=np.float64), (len(tris),))
    except ValueError:
        return out
    if not np.isfinite(margins).all() or np.any(margins < 0.0):
        return out
    for face in params:
        origin, axis = face["o"], face["z"]
        major, minor = float(face["major"]), float(face["minor"])
        if (np.shape(origin) != (3,) or np.shape(axis) != (3,)
                or not np.isfinite(origin).all() or not np.isfinite(axis).all()
                or not math.isfinite(major) or not math.isfinite(minor)
                or not major > minor > 0.0):
            continue
        axis_norm = float(np.linalg.norm(axis))
        if not math.isfinite(axis_norm) or abs(axis_norm - 1.0) > 64.0 * eps:
            continue
        rel = tris - origin
        axial = np.einsum("ntk,k->nt", rel, axis)
        perp = rel - axial[..., None] * axis
        rho = np.linalg.norm(perp, axis=2)
        f = np.hypot(rho - major, axial)
        edges = np.stack((tris[:, 0]-tris[:, 1], tris[:, 1]-tris[:, 2], tris[:, 2]-tris[:, 0]), axis=1)
        diameter = np.linalg.norm(edges, axis=2).max(axis=1)
        scale = np.maximum(1.0, np.maximum(np.abs(tris).max(axis=(1, 2)),
                          np.maximum(float(np.abs(origin).max()), np.maximum(major, minor))))
        # Cover projection/norm roundoff, including cancellation against a distant origin.
        # Every geometric input below is inflated before it is used in a certified bound.
        slack = 1024.0 * eps * scale
        diameter_hi = np.nextafter(diameter + slack, np.inf)
        rho_lo = np.nextafter(rho.min(axis=1) - diameter_hi - slack, -np.inf)
        rho_hi = np.nextafter(rho.max(axis=1) + slack, np.inf)
        f_lo = np.nextafter(f.min(axis=1) - diameter_hi - slack, -np.inf)
        valid = (np.isfinite(rho_lo) & np.isfinite(rho_hi) & np.isfinite(f_lo)
                 & np.isfinite(diameter_hi) & (rho_lo > 0.0) & (f_lo > 0.0))
        if not valid.any():
            continue
        numerator = np.maximum(np.abs(rho_lo[valid]-major), np.abs(rho_hi[valid]-major))
        numerator = np.nextafter(numerator + slack[valid], np.inf)
        denominator = np.nextafter(rho_lo[valid] * f_lo[valid], 0.0)
        if np.any(denominator <= 0.0):
            continue
        curvature = np.maximum(np.nextafter(1.0 / f_lo[valid], np.inf),
                               np.nextafter(numerator / denominator, np.inf))
        curvature = np.nextafter(curvature + 1024.0 * eps * np.maximum(1.0, curvature), np.inf)
        vertex_deviation = np.nextafter(np.abs(f - minor).max(axis=1) + slack, np.inf)
        diameter_sq_hi = np.nextafter(diameter_hi[valid] * diameter_hi[valid], np.inf)
        interpolation = np.nextafter(curvature * diameter_sq_hi, np.inf)
        interpolation = np.nextafter(interpolation / 6.0, np.inf)
        bound = np.nextafter(vertex_deviation[valid] + interpolation, np.inf)
        bound = np.nextafter(bound + max(CLASSIFY_TOLERANCE, FAST_MARGIN), np.inf)
        bound = np.nextafter(bound + margins[valid], np.inf)
        out[valid] = np.minimum(out[valid], bound)
    return out


def _batched_full_torus_triangle_bounds(triangles, params, batch_size=8192):
    """Evaluate the torus bound in fixed-size chunks to cap peak temporary memory."""
    triangles = np.asarray(triangles, dtype=np.float64)
    out = np.full(len(triangles), np.inf)
    for start in range(0, len(triangles), batch_size):
        stop = min(start + batch_size, len(triangles))
        out[start:stop] = full_torus_triangle_bounds(triangles[start:stop], params)
    return out


def certify_triangles(solid, triangles, tolerance, max_queries=100_000, max_depth=18):
    """Bound distance to the CAD boundary over every point of every facet triangle.

    Verified convex planar faces, filled disks, and single-hole convex planar regions provide
    geometric upper bounds over each triangle. Complete finite cylinders and full tori add
    outward-rounded Hessian bounds. Query-free analytic subdivision refines candidates that do
    not meet tolerance at first. Remaining leaves
    use the generic 1-Lipschitz bound: for centroid ``c``, every point is within
    ``max(|vertex-c|)`` of c, so ``distance(c) + radius`` bounds the whole triangle.
    ``point_distance`` returns the attained nearest-boundary distance; query limits or a depth
    limit leave the result unresolved, never certified by sampling alone. The planar bounds
    require OpenCASCADE-verified wire topology, strict containment, and convexity; uncertain
    trims are omitted. Kernel and arithmetic slack is rounded outward. Analytic subdivision
    work is capped at min(25.6 million face-bound evaluations, max(8192, 256 * max_queries));
    temporary triangle batches are bounded independently of facet count.
    """
    import heapq
    import time

    started = time.monotonic()
    triangles = np.asarray(triangles, dtype=np.float64)
    if triangles.ndim != 3 or triangles.shape[1:] != (3, 3):
        raise ValueError("facet triangles must have shape (n, 3, 3)")
    if not math.isfinite(tolerance) or tolerance < 0 or max_queries < 1 or max_depth < 0:
        raise ValueError("invalid certification limits")
    if len(triangles) == 0:
        return {"status": "unresolved", "upper_bound": None, "queries": 0,
                "max_queries": max_queries, "max_depth": max_depth,
                "seconds": time.monotonic() - started}
    planar_faces = solid.convex_planar_faces() if hasattr(solid, "convex_planar_faces") else ()
    planar_regions = solid.planar_region_faces() if hasattr(solid, "planar_region_faces") else ()
    planar_upper = np.full(len(triangles), np.inf)
    if planar_faces:
        for i, tri in enumerate(triangles):
            planar_upper[i] = min(convex_planar_triangle_bound(tri, face) for face in planar_faces)
    planar_region_upper = np.full(len(triangles), np.inf)
    if planar_regions:
        for i, tri in enumerate(triangles):
            planar_region_upper[i] = min(planar_region_triangle_bound(tri, face) for face in planar_regions)
    torus_faces = solid.full_torus_faces() if hasattr(solid, "full_torus_faces") else ()
    torus_upper = _batched_full_torus_triangle_bounds(triangles, torus_faces) if torus_faces else np.full(len(triangles), np.inf)
    cylinder_faces = solid.full_cylinder_faces() if hasattr(solid, "full_cylinder_faces") else ()
    cylinder_upper = full_cylinder_triangle_bounds(triangles, cylinder_faces) if cylinder_faces else np.full(len(triangles), np.inf)
    analytic_upper = np.minimum(np.minimum(np.minimum(planar_upper, planar_region_upper), torus_upper), cylinder_upper)
    analytic_done = analytic_upper <= tolerance
    certified_upper = float(np.max(analytic_upper[analytic_done])) if analytic_done.any() else 0.0
    witness_distance = getattr(solid, "measured_witness_distance", None)
    if ((torus_faces or cylinder_faces or planar_regions) and witness_distance is not None and math.isfinite(witness_distance)
            and witness_distance > tolerance and np.isfinite(analytic_upper).all()):
        # The measured witness proves the tolerance is exceeded. A finite analytic
        # upper bound over every triangle still certifies the complete global maximum.
        # Do not spend subdivision or CAD-query budget trying to make a failing case pass.
        global_upper = max(float(np.max(analytic_upper)), float(witness_distance))
        return {"status": "certified", "sampled_distance_upper": float(witness_distance),
                "upper_bound": global_upper, "queries": 0,
                "max_queries": max_queries, "max_depth": max_depth,
                "seconds": time.monotonic() - started}
    # Largest unresolved upper bound first, so each query reduces the current global bound.
    pending = []
    serial = 0
    queries = 0
    sampled_distance_upper = 0.0
    unresolved_upper = 0.0
    unresolved = False
    # Initial bounds are mandatory; adaptive analytic work is separately capped so
    # subdivision cannot expand without limit. The cap scales with the CAD query budget.
    analytic_work_cap = min(25_600_000, max(8192, max_queries * 256))
    analytic_faces = len(planar_faces) + len(planar_regions) + len(torus_faces) + len(cylinder_faces)
    analytic_work = len(triangles) * analytic_faces

    def evaluate(tri, depth, coverage_slack=0.0):
        nonlocal queries, sampled_distance_upper, certified_upper
        analytic_bound = math.inf
        if planar_faces:
            analytic_bound = min(convex_planar_triangle_bound(tri, face) + coverage_slack for face in planar_faces)
        if planar_regions:
            analytic_bound = min(analytic_bound, min(planar_region_triangle_bound(
                tri, face, extra_slack=coverage_slack) for face in planar_regions))
        if torus_faces:
            analytic_bound = min(analytic_bound, float(full_torus_triangle_bounds(
                tri[None], torus_faces, extra_slack=coverage_slack)[0]))
        if cylinder_faces:
            analytic_bound = min(analytic_bound, float(full_cylinder_triangle_bounds(
                tri[None], cylinder_faces, extra_slack=coverage_slack)[0]))
        if analytic_bound <= tolerance:
            certified_upper = max(certified_upper, analytic_bound)
            return analytic_bound, None, depth
        center = tri.mean(axis=0)
        value, _calls = solid.point_distance(center, floor=0.0)
        queries += 1
        radius = float(np.linalg.norm(tri - center, axis=1).max())
        # Round outward by a scale-aware floating point allowance. This does not remove the
        # separate kernel tolerance; it prevents arithmetic rounding from shrinking the bound.
        scale = max(1.0, float(np.abs(tri).max()), abs(value), radius)
        # Include OpenCASCADE's face-classifier tolerance as outward slack. The query distance is
        # an attained boundary distance (hence an upper bound on the minimum distance).
        upper = value + radius + coverage_slack + CLASSIFY_TOLERANCE + 64.0 * np.finfo(np.float64).eps * scale
        upper = min(upper, analytic_bound)
        sampled_distance_upper = max(sampled_distance_upper, value)
        return upper, center, depth

    remaining_triangles = triangles[~analytic_done]
    if not torus_faces and not cylinder_faces and not planar_regions and len(remaining_triangles) > max_queries:
        return {
            "status": "unresolved", "upper_bound": None, "queries": 0,
            "max_queries": max_queries, "max_depth": max_depth,
            "seconds": time.monotonic() - started,
        }
    def enqueue_fallback(tri, depth, coverage_slack, known_upper):
        nonlocal serial, unresolved, unresolved_upper, certified_upper
        if queries >= max_queries:
            unresolved = True
            unresolved_upper = max(unresolved_upper, known_upper)
            return
        upper, _center, _depth = evaluate(tri, depth, coverage_slack)
        if upper <= tolerance:
            certified_upper = max(certified_upper, upper)
        else:
            heapq.heappush(pending, (-upper, serial, tri, depth, coverage_slack))
            serial += 1

    if torus_faces or cylinder_faces or planar_regions:
        # DFS over bounded batches. Each batch has at most 2048 triangles and each
        # stack entry at most that many, so even a deep refinement keeps memory fixed.
        # The four child groups are processed separately instead of materializing a
        # whole 4^depth frontier.
        indices = np.flatnonzero(~analytic_done)
        for start in range(0, len(indices), 2048):
            group = indices[start:start + 2048]
            roots = triangles[group]
            root_bounds = analytic_upper[group]
            stack = [(roots, np.zeros(len(roots), dtype=np.int16),
                      np.zeros(len(roots), dtype=np.float64), root_bounds)]
            while stack:
                batch, depths, slack, bounds = stack.pop()
                if len(batch) == 0:
                    continue
                certified = bounds <= tolerance
                if certified.any():
                    certified_upper = max(certified_upper, float(np.max(bounds[certified])))
                bad = np.flatnonzero(~certified)
                if len(bad) == 0:
                    continue
                can_split = (depths[bad] < max_depth
                             ) & (analytic_work + 4 * len(bad) * analytic_faces <= analytic_work_cap)
                fallback = bad[~can_split]
                for index in fallback:
                    enqueue_fallback(batch[index], int(depths[index]), float(slack[index]), float(bounds[index]))
                split = bad[can_split]
                if len(split) == 0:
                    continue
                parent = batch[split]
                a, b, c = parent[:, 0], parent[:, 1], parent[:, 2]
                ab, bc, ca = (a / 2.0 + b / 2.0), (b / 2.0 + c / 2.0), (c / 2.0 + a / 2.0)
                midpoint_error = 32.0 * np.finfo(np.float64).eps * np.maximum(
                    1.0, np.abs(parent).max(axis=(1, 2)))
                child_slack = slack[split] + midpoint_error
                child_depth = depths[split] + 1
                children = (np.stack((a, ab, ca), axis=1), np.stack((ab, b, bc), axis=1),
                            np.stack((ca, bc, c), axis=1), np.stack((ab, bc, ca), axis=1))
                for child in children:
                    child_bounds = np.full(len(child), np.inf)
                    if torus_faces:
                        child_bounds = np.minimum(child_bounds, full_torus_triangle_bounds(
                            child, torus_faces, extra_slack=child_slack))
                    if cylinder_faces:
                        child_bounds = np.minimum(child_bounds, full_cylinder_triangle_bounds(
                            child, cylinder_faces, extra_slack=child_slack))
                    if planar_faces:
                        for j, child_tri in enumerate(child):
                            child_bounds[j] = min(child_bounds[j], min(
                                convex_planar_triangle_bound(child_tri, face) + float(child_slack[j])
                                for face in planar_faces))
                    if planar_regions:
                        for j, child_tri in enumerate(child):
                            child_bounds[j] = min(child_bounds[j], min(
                                planar_region_triangle_bound(child_tri, face, extra_slack=float(child_slack[j]))
                                for face in planar_regions))
                    # The child bounds are computed here and included in the work cap;
                    # push the already bounded arrays so they are not recomputed.
                    analytic_work += len(child) * analytic_faces
                    stack.append((child, child_depth, child_slack, child_bounds))
    else:
        for tri in remaining_triangles:
            enqueue_fallback(tri, 0, 0.0, math.inf)

    while pending:
        neg_upper, _serial, tri, depth, coverage_slack = heapq.heappop(pending)
        upper = -neg_upper
        if upper <= tolerance:
            certified_upper = max(certified_upper, upper)
            continue
        if queries + 4 > max_queries or depth >= max_depth:
            unresolved = True
            unresolved_upper = max(unresolved_upper, upper)
            continue
        a, b, c = tri
        ab, bc, ca = a / 2.0 + b / 2.0, b / 2.0 + c / 2.0, c / 2.0 + a / 2.0
        scale = max(1.0, float(np.abs(tri).max()))
        child_slack = coverage_slack + 32.0 * np.finfo(np.float64).eps * scale
        for child in (np.array([a, ab, ca]), np.array([ab, b, bc]),
                      np.array([ca, bc, c]), np.array([ab, bc, ca])):
            child_upper, _center, child_depth = evaluate(child, depth + 1, child_slack)
            if child_upper <= tolerance:
                certified_upper = max(certified_upper, child_upper)
            else:
                heapq.heappush(pending, (-child_upper, serial, child, child_depth, child_slack))
                serial += 1

    return {
        "status": "unresolved" if unresolved else "certified",
        "sampled_distance_upper": sampled_distance_upper,
        "upper_bound": max(certified_upper, unresolved_upper),
        "queries": queries,
        "max_queries": max_queries,
        "max_depth": max_depth,
        "seconds": time.monotonic() - started,
    }
