# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for GEO-032 (faris_deviation.py) and the per-solid tessellation of faris_cad.py.

The closed forms are compared with OpenCASCADE's own projection on random points for each
surface type, including points whose foot point falls outside the face. The CAD
interpreter (FARIS_CAD_PYTHON, default ~/.cache/avila-night/feasibility-cad/cadvenv) runs the
tests; under any other Python they re-run themselves there and skip when there is none.
Run from the repository root:

    python3 -m unittest discover -s integrations/cad -p 'test_*.py'
"""
import json
import math
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

CAD_PYTHON = Path(
    os.environ.get(
        "FARIS_CAD_PYTHON",
        str(Path.home() / ".cache/avila-night/feasibility-cad/cadvenv/bin/python"),
    )
)

try:
    import numpy as np
    import faris_deviation as fd
    import OCP  # noqa: F401

    HAVE_CAD = True
except ImportError:
    HAVE_CAD = False


if 'np' in globals():
    class ArcBounds(unittest.TestCase):
        def test_full_torus_period_keeps_actual_first_wall_point_on_circle(self):
            # Point at the GEO-032 worst sample for the synthetic FW shell. Its
            # torus meridian radius is 335.0110998575 mm, about 0.0111 mm from
            # the 335 mm torus, so a full-period strip bound must stay tiny.
            point = np.array([211.65011620904295, 131.612499811874, -222.1460905106671])
            rho = np.hypot(point[0], point[1])
            meridian = np.hypot(rho - 500.0, point[2])
            bound = fd._arc_distance(
                np.array([rho]), np.array([point[2]]), np.array([500.0]),
                np.array([0.0]), np.array([335.0]), np.array([0.0]),
                np.array([2.0 * math.pi]),
            )
            self.assertAlmostEqual(float(bound[0]), abs(meridian - 335.0), places=9)
            self.assertLess(float(bound[0]), 0.02)

        @unittest.skipUnless(HAVE_CAD, "OpenCASCADE is unavailable")
        def test_reach_keeps_the_nearest_face_for_the_actual_first_wall_point(self):
            solid = fd.SolidFaces(cq.Solid.makeTorus(500.0, 335.0).wrapped)
            point = np.array([[211.65011620904295, 131.612499811874, -222.1460905106671]])
            candidates = solid.reach(point)
            all_faces, all_exact = solid.bounds(point)
            pruned_faces, pruned_exact = solid.bounds(point, candidates)
            self.assertEqual(candidates.tolist(), [0])
            self.assertTrue(all_exact[0])
            self.assertTrue(pruned_exact[0])
            self.assertAlmostEqual(pruned_faces[0], all_faces[0], places=9)
            self.assertLess(pruned_faces[0], 0.02)

        def test_partial_arc_still_bounds_from_nearest_endpoint(self):
            # The point lies opposite a 0..pi/2 arc; its nearest point is the
            # arc endpoint at pi/2, at distance sqrt(8).
            bound = fd._arc_distance(
                np.array([-2.0]), np.array([0.0]), np.array([0.0]),
                np.array([0.0]), np.array([2.0]), np.array([0.0]),
                np.array([math.pi / 2.0]),
            )
            self.assertAlmostEqual(float(bound[0]), math.sqrt(8.0), places=12)

    class ConvexPlanarBounds(unittest.TestCase):
        def test_coplanar_triangle_uses_zero_subdivision_queries(self):
            face = (np.zeros(3), np.array([0., 0., 1.]),
                    np.array([[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]]))

            class FaceOracle:
                def convex_planar_faces(self):
                    return [face]

                def point_distance(self, _point, floor=0.0):
                    raise AssertionError("the verified planar-face bound should settle this triangle")

            tri = np.array([[-.5, -.5, 0.], [.5, -.5, 0.], [0., .5, 0.]])
            result = fd.certify_triangles(FaceOracle(), np.array([tri]), .05)
            self.assertEqual(result["status"], "certified")
            self.assertLessEqual(result["upper_bound"], .05)
            self.assertEqual(result["queries"], 0)

        def test_triangle_bound_covers_tilted_triangle_and_projection_outside_face(self):
            face = (np.array([0., 0., 0.]), np.array([0., 0., 1.]),
                    np.array([[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]]))
            tri = np.array([[2., 0., 0.], [2.2, 0.2, .1], [2., .4, 0.]])
            bound = fd.convex_planar_triangle_bound(tri, face)
            self.assertGreater(bound, math.sqrt(1.2**2 + .1**2))
            for i in range(21):
                for j in range(21-i):
                    a, b = i/20, j/20
                    p = (1-a-b)*tri[0] + a*tri[1] + b*tri[2]
                    point_bound = fd.convex_planar_triangle_bound(np.repeat(p[None], 3, axis=0), face)
                    self.assertLessEqual(point_bound, bound + 1e-12)

        def test_short_polygon_edge_does_not_hide_outside_point(self):
            face = (np.zeros(3), np.array([0., 0., 1.]),
                    np.array([[0., 0., 0.], [1e-5, 0., 0.], [1e-5, 2e-5, 0.], [0., 2e-5, 0.]]))
            point = np.array([1e-5 + 1e-3, 1e-5, 0.])
            bound = fd.convex_planar_triangle_bound(np.repeat(point[None], 3, axis=0), face)
            self.assertGreaterEqual(bound, 1e-3)
            self.assertLess(bound, 1e-3 + 2e-6)

        def test_convexity_check_rejects_concavity_self_intersection_and_degeneracy(self):
            self.assertTrue(fd.is_convex_polygon_2d([[0, 0], [2, 0], [2, 2], [0, 2]], 1e-9))
            self.assertFalse(fd.is_convex_polygon_2d([[0, 0], [2, 0], [1, 1], [2, 2], [0, 2]], 1e-9))
            self.assertFalse(fd.is_convex_polygon_2d([[0, 0], [2, 2], [0, 2], [2, 0]], 1e-9))
            self.assertFalse(fd.is_convex_polygon_2d([[0, 0], [2, 0], [2, 2], [1, 2-1e-8], [0, 2]], 1e-12))
            self.assertFalse(fd.is_convex_polygon_2d([[0, 0], [1, 0], [2, 0]], 1e-9))

        @unittest.skipUnless(HAVE_CAD, "OpenCASCADE is unavailable")
        def test_only_single_wire_convex_planar_faces_are_extracted(self):
            box = fd.SolidFaces(cq.Workplane().box(4, 3, 2).val().wrapped)
            self.assertEqual(len(box.convex_planar_faces()), 6)
            holed = fd.SolidFaces(cq.Workplane("XY").circle(2).circle(1).extrude(1).val().wrapped)
            self.assertFalse(any(abs(abs(normal[2])-1) < 1e-9 for _origin, normal, _poly in holed.convex_planar_faces()))
            concave_shape = cq.Workplane("XY").polyline([(0, 0), (2, 0), (2, 1), (1, 1), (1, 2), (0, 2)]).close().extrude(1).val()
            concave = fd.SolidFaces(concave_shape.wrapped)
            self.assertFalse(any(abs(origin[2]-1) < 1e-9 and abs(normal[2]) > .9
                                 for origin, normal, _poly in concave.convex_planar_faces()))


if not HAVE_CAD:

    @unittest.skipUnless(CAD_PYTHON.is_file(), "no CAD interpreter (set FARIS_CAD_PYTHON)")
    class RunInCadInterpreter(unittest.TestCase):
        def test_deviation_and_tessellation(self):
            result = subprocess.run(
                [str(CAD_PYTHON), "-m", "unittest", "test_faris_deviation", "-v"],
                cwd=HERE, capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr[-4000:])

else:
    import cadquery as cq
    import faris_cad
    import faris_deviation as fd
    import faris_facets
    from OCP.ElSLib import ElSLib
    from OCP.gp import gp_Pnt

    class PlaneOracle:
        """Boundary-distance fixture for the Lipschitz subdivision proof."""
        def point_distance(self, point, floor=0.0):
            return abs(float(point[2])), 0

    class CertifiedTriangleBound(unittest.TestCase):
        def test_full_torus_hessian_bound_covers_dense_barycentric_samples(self):
            params = {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 30., "minor": 5.}
            tri = np.array([[[35., 0., 0.], [34., 1., .4], [34., -1., -.4]]])
            bound = fd.full_torus_triangle_bounds(tri, [params])[0]
            self.assertTrue(math.isfinite(bound))
            for i in range(41):
                for j in range(41-i):
                    a, b = i/40, j/40
                    p = (1-a-b)*tri[0, 0] + a*tri[0, 1] + b*tri[0, 2]
                    rho = math.hypot(p[0], p[1])
                    exact = abs(math.hypot(rho-30., p[2])-5.)
                    self.assertLessEqual(exact, bound + 1e-12)

        def test_full_torus_bound_rejects_triangles_near_singular_sets(self):
            params = {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 30., "minor": 5.}
            tri = np.array([[[30., 0., 0.], [30., 1., 0.], [30., 0., 1.]]])
            self.assertTrue(math.isinf(fd.full_torus_triangle_bounds(tri, [params])[0]))

        def test_bounded_torus_subdivision_certifies_parent_with_zero_cad_queries(self):
            params = {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 30., "minor": 5.}

            class FullTorusOracle:
                def full_torus_faces(self):
                    return [params]

                def convex_planar_faces(self):
                    return []

                def point_distance(self, _point, floor=0.0):
                    raise AssertionError("analytic subdivision should certify every child")

            angles = np.deg2rad([0., 2., -2.])
            tri = np.array([[[35*np.cos(a), 35*np.sin(a), 0.] for a in angles]])
            initial = fd.full_torus_triangle_bounds(tri, [params])[0]
            self.assertGreater(initial, .1)
            result = fd.certify_triangles(FullTorusOracle(), tri, .1, max_queries=1, max_depth=8)
            self.assertEqual(result["status"], "certified")
            self.assertEqual(result["queries"], 0)

        def test_torus_analytic_work_cap_leaves_unresolved_bound(self):
            params = {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 30., "minor": 5.}

            class FullTorusOracle:
                def full_torus_faces(self):
                    return [params]

                def convex_planar_faces(self):
                    return []

                def point_distance(self, point, floor=0.0):
                    rho = math.hypot(point[0], point[1])
                    return abs(math.hypot(rho-params["major"], point[2])-params["minor"]), 0

            angles = np.deg2rad([0., 2., -2.])
            one = np.array([[35*np.cos(a), 35*np.sin(a), 0.] for a in angles])
            triangles = np.repeat(one[None], 2048, axis=0)
            result = fd.certify_triangles(FullTorusOracle(), triangles, .1, max_queries=1, max_depth=8)
            self.assertEqual(result["status"], "unresolved")
            self.assertLessEqual(result["queries"], 1)

        def test_measured_tori7_witness_returns_loose_certified_global_bound(self):
            params = [
                {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 1800., "minor": 900.},
                {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 1800., "minor": 600.},
            ]

            class MeasuredTorusSolid:
                faces = [object(), object()]
                measured_witness_distance = 1.1795735420579376

                def full_torus_faces(self):
                    return params

                def convex_planar_faces(self):
                    return []

                def point_distance(self, _point, floor=0.0):
                    raise AssertionError("a measured out-of-tolerance witness makes CAD queries unnecessary")

            tri = np.array([[
                [2650.721516306545, 409.42907740697643, 178.33152887945784],
                [2672.1997098391607, 308.49242137904616, 134.13803955855698],
                [2663.937255521974, 411.47037366572624, 89.61106193623502],
            ]])
            result = fd.certify_triangles(MeasuredTorusSolid(), tri, 1.0)
            self.assertEqual(result["status"], "certified")
            self.assertEqual(result["queries"], 0)
            self.assertGreater(result["upper_bound"], 1.0)
            self.assertGreaterEqual(result["upper_bound"], 1.1795735420579376)

        def test_within_tolerance_witness_keeps_normal_torus_certification_path(self):
            params = {"o": np.zeros(3), "z": np.array([0., 0., 1.]), "major": 30., "minor": 5.}

            class MeasuredTorusSolid:
                measured_witness_distance = .05

                def full_torus_faces(self):
                    return [params]

                def convex_planar_faces(self):
                    return []

                def point_distance(self, _point, floor=0.0):
                    raise AssertionError("the analytic bound should certify this in-tolerance triangle")

            angles = np.deg2rad([0., 2., -2.])
            tri = np.array([[[35*np.cos(a), 35*np.sin(a), 0.] for a in angles]])
            result = fd.certify_triangles(MeasuredTorusSolid(), tri, .1)
            self.assertEqual(result["status"], "certified")
            self.assertEqual(result["queries"], 0)
            self.assertLessEqual(result["upper_bound"], .1)

        def test_unproven_torus_witness_does_not_skip_generic_fallback(self):
            class UnsupportedOracle:
                measured_witness_distance = 1.2

                def full_torus_faces(self):
                    return []

                def convex_planar_faces(self):
                    return []

                def point_distance(self, _point, floor=0.0):
                    return .01, 0

            tri = np.array([[[0., 0., 0.], [.2, 0., 0.], [0., .2, 0.]]])
            result = fd.certify_triangles(UnsupportedOracle(), tri, .05, max_queries=1, max_depth=0)
            self.assertEqual(result["status"], "unresolved")
            self.assertEqual(result["queries"], 1)

        def test_subdivision_certifies_every_point_when_global_upper_is_under_tolerance(self):
            triangles = np.array([[[0.0, 0.0, 0.09], [0.2, 0.0, 0.09], [0.0, 0.2, 0.09]]])
            result = fd.certify_triangles(PlaneOracle(), triangles, 0.1, max_queries=1000, max_depth=8)
            self.assertEqual(result["status"], "certified")
            self.assertLessEqual(result["upper_bound"], 0.1)
            self.assertGreater(result["queries"], 1)

        def test_budget_exhaustion_is_unresolved_not_a_sampled_pass(self):
            triangles = np.array([[[0.0, 0.0, 0.09], [0.2, 0.0, 0.09], [0.0, 0.2, 0.09]]])
            result = fd.certify_triangles(PlaneOracle(), triangles, 0.1, max_queries=1, max_depth=8)
            self.assertEqual(result["status"], "unresolved")

        def test_outside_tolerance_remains_unresolved_without_a_proven_lower_bound(self):
            triangles = np.array([[[0.0, 0.0, 0.11], [0.2, 0.0, 0.11], [0.0, 0.2, 0.11]]])
            result = fd.certify_triangles(PlaneOracle(), triangles, 0.1, max_queries=1000, max_depth=8)
            self.assertEqual(result["status"], "unresolved")
            self.assertGreater(result["upper_bound"], 0.1)

    class DeviationReportAssembly(unittest.TestCase):
        def test_each_solid_gets_its_own_global_certification(self):
            with tempfile.TemporaryDirectory() as temp_dir:
                root = Path(temp_dir)
                mapping = [
                    {"volume": 1, "step_index": 0},
                    {"volume": 2, "step_index": 1},
                ]
                map_path = root / "map.json"
                map_path.write_text(json.dumps(mapping))

                vertices = np.array([
                    [0., 0., 0.], [1., 0., 0.], [0., 1., 0.],
                    [2., 0., 0.], [3., 0., 0.], [2., 1., 0.],
                ], dtype="<f8")
                triangles = np.array([[0, 1, 2], [3, 4, 5]], dtype="<u4")
                surface_dtype = np.dtype([
                    ("first", "<u8"), ("count", "<u8"),
                    ("fwd", "<u4"), ("rev", "<u4"),
                ])
                surfaces = np.array([(0, 1, 1, 0), (1, 1, 2, 0)], dtype=surface_dtype)
                header = bytearray(48)
                header[16:40] = np.array([6, 2, 2], dtype="<u8").tobytes()
                facets_path = root / "facets.bin"
                facets_path.write_bytes(header + vertices.tobytes() + triangles.tobytes() + surfaces.tobytes())

                class FaceStub:
                    analytic = np.array([True, False])

                    def __len__(self):
                        return 2

                face_a, face_b = FaceStub(), FaceStub()
                rows = [
                    {"points": 1, "points_available": 1, "points_closed_form": 1,
                     "points_occt": 0, "points_occt_needed": 0, "occt_calls": 0,
                     "worst": 0.1, "at": None},
                    {"points": 1, "points_available": 1, "points_closed_form": 1,
                     "points_occt": 0, "points_occt_needed": 0, "occt_calls": 0,
                     "worst": 0.2, "at": None},
                ]
                certifications = [
                    {"status": "certified", "upper_bound": 1.5, "queries": 11,
                     "max_queries": 100, "max_depth": 8},
                    {"status": "unresolved", "upper_bound": 4.5, "queries": 22,
                     "max_queries": 200, "max_depth": 9},
                ]
                args = type("Args", (), {
                    "map": map_path,
                    "facets": facets_path,
                    "step": root / "unused.step",
                    "shape_cache": None,
                    "shape_unit_m": 1e-3,
                    "workers": 1,
                    "occt_points": 0,
                    "block_points": 32,
                    "tolerance_m": 1e-4,
                    "max_bound_queries": 100,
                    "max_bound_depth": 8,
                    "report": root / "deviation.json",
                })()

                with patch.object(faris_cad, "load_solids", return_value=["solid-a", "solid-b"]), \
                     patch.object(fd, "SolidFaces", side_effect=[face_a, face_b]), \
                     patch.object(fd, "measure", return_value=(rows, {})), \
                     patch.object(fd, "certify_triangles", side_effect=certifications) as certify, \
                     patch.object(faris_cad, "tool_versions", return_value={}), \
                     patch.object(faris_cad, "peak_rss_mb", return_value=1):
                    self.assertEqual(faris_cad.deviation(args), 0)

                report = json.loads(args.report.read_text())
                self.assertEqual(
                    [(solid["volume"], solid["global_bound_status"]) for solid in report["solids"]],
                    [(1, "certified"), (2, "unresolved")],
                )
                self.assertAlmostEqual(report["solids"][0]["global_upper_bound_m"], 0.0015)
                self.assertAlmostEqual(report["solids"][1]["global_upper_bound_m"], 0.0045)
                self.assertEqual(certify.call_args_list[0].args[0], face_a)
                self.assertEqual(certify.call_args_list[1].args[0], face_b)

    def shapes():
        """Solids with one analytic surface type each, cut so that some faces are partial."""
        box = cq.Workplane().box(40, 30, 20).val()
        cylinder = cq.Workplane().circle(10).extrude(20).intersect(cq.Workplane().box(30, 12, 30).translate((5, 0, 10))).val()
        torus = cq.Workplane().add(cq.Solid.makeTorus(30, 5)).intersect(cq.Workplane().box(80, 40, 40).translate((25, 0, 0))).val()
        return {
            "plane": box,
            "cylinder": cylinder,
            "cone": cq.Solid.makeCone(10, 4, 20),
            "cone to an apex": cq.Solid.makeCone(0, 10, 20),
            "sphere": cq.Solid.makeSphere(10, cq.Vector(0, 0, 0), cq.Vector(0, 0, 1), -30, 70, 360),
            "torus": torus,
        }

    def spline_solid():
        """A solid with a B-spline face: a block whose top is an extruded spline."""
        profile = cq.Workplane("XZ").moveTo(0, 0).lineTo(30, 0).lineTo(30, 10).spline([(20, 14), (10, 8), (0, 12)], includeCurrent=True).close()
        return profile.extrude(20).val()

    class ClosedForms(unittest.TestCase):
        def test_measure_retains_an_exact_boundary_witness_for_certification(self):
            solid = fd.SolidFaces(cq.Solid.makeTorus(30, 5).wrapped)
            points = np.array([[33.5, 0., 0.]])
            rows, _timing = fd.measure([solid], [points], workers=1, occt_points=0, block=8)
            self.assertIsNotNone(rows[0]["at"])
            self.assertGreater(rows[0]["worst"], 1.0)
            self.assertEqual(solid.measured_witness_distance, rows[0]["worst"])

        def test_full_torus_topology_guard_rejects_trimmed_torus(self):
            full = fd.SolidFaces(cq.Solid.makeTorus(30, 5).wrapped)
            self.assertGreaterEqual(len(full.full_torus_faces()), 1)
            trimmed_shape = cq.Solid.makeTorus(30, 5).intersect(
                cq.Workplane().box(80, 40, 40).translate((25, 0, 0)).val())
            trimmed = fd.SolidFaces(trimmed_shape.wrapped)
            self.assertEqual(len(trimmed.full_torus_faces()), 0)

        def test_the_closed_form_distance_agrees_with_opencascade_on_every_surface_type(self):
            rng = np.random.default_rng(1)
            for name, shape in shapes().items():
                with self.subTest(name):
                    solid = fd.SolidFaces(shape.wrapped)
                    lo, hi = solid.lo.min(axis=0), solid.hi.max(axis=0)
                    scale = float(np.linalg.norm(hi - lo))
                    points = lo - 0.3 * (hi - lo) + rng.random((300, 3)) * 1.6 * (hi - lo)
                    inside = outside = 0
                    for f in range(len(solid)):
                        self.assertTrue(solid.analytic[f], f"{name}: face {f} is not analytic")
                        exact = solid.exact_on_face(f, points)
                        surface = fd.DISTANCES[solid.kind[f]](points, fd.stack_parameters([solid.params[f]]))[:, 0]
                        for i, p in enumerate(points):
                            projected = solid.occt_distance(f, p)
                            if math.isnan(exact[i]):
                                outside += 1
                                # the foot point is outside the face: the surface is nearer than the face
                                self.assertLessEqual(surface[i], projected + 1e-9 * scale)
                            else:
                                inside += 1
                                self.assertLessEqual(abs(exact[i] - projected), 1e-9 * scale)
                    self.assertGreater(inside, 20)
                    self.assertGreater(outside, 20, "the sample must include feet outside the faces")

        def test_the_whole_solid_distance_is_the_minimum_over_faces(self):
            rng = np.random.default_rng(2)
            for name, shape in shapes().items():
                with self.subTest(name):
                    solid = fd.SolidFaces(shape.wrapped)
                    lo, hi = solid.lo.min(axis=0), solid.hi.max(axis=0)
                    scale = float(np.linalg.norm(hi - lo))
                    for p in lo - 0.2 * (hi - lo) + rng.random((25, 3)) * 1.4 * (hi - lo):
                        brute = min(solid.occt_distance(f, p) for f in range(len(solid)))
                        value, _calls = solid.point_distance(p)
                        self.assertLessEqual(abs(value - brute), 1e-9 * scale)

        def test_uv_follows_opencascades_parameterisation(self):
            rng = np.random.default_rng(3)
            for name, shape in shapes().items():
                solid = fd.SolidFaces(shape.wrapped)
                for f, kind in enumerate(solid.kind):
                    if kind == "plane":
                        continue
                    params = solid.params[f]
                    # points on the surface, from the closed-form foot of random points
                    probe = solid.lo[f] + rng.random((40, 3)) * (solid.hi[f] - solid.lo[f])
                    foot, ok = fd.FEET[kind](probe, params)
                    u, v = fd.uv(kind, params, foot[ok])
                    for k, x in enumerate(foot[ok]):
                        pnt = gp_Pnt(*[float(c) for c in x])
                        position = _ax3(params)
                        if kind == "cylinder":
                            eu, ev = ElSLib.CylinderParameters_s(position, params["r"], pnt)
                        elif kind == "sphere":
                            eu, ev = ElSLib.SphereParameters_s(position, params["r"], pnt)
                        elif kind == "torus":
                            eu, ev = ElSLib.TorusParameters_s(position, params["major"], params["minor"], pnt)
                        else:
                            eu, ev = ElSLib.ConeParameters_s(position, params["R"], params["signed"], pnt)
                        du = abs((u[k] - eu + math.pi) % (2 * math.pi) - math.pi)
                        self.assertLess(du, 1e-9, f"{name} {kind} u")
                        scale = max(1.0, abs(ev))
                        self.assertLess(abs(v[k] - ev), 1e-9 * scale, f"{name} {kind} v")

        def test_the_fast_forms_bound_the_direct_forms(self):
            rng = np.random.default_rng(4)
            for name, shape in shapes().items():
                solid = fd.SolidFaces(shape.wrapped)
                lo, hi = solid.lo.min(axis=0), solid.hi.max(axis=0)
                points = lo - 0.3 * (hi - lo) + rng.random((200, 3)) * 1.6 * (hi - lo)
                for kind, arrays in solid.arrays.items():
                    direct = fd.DISTANCES[kind](points, arrays)
                    fast = fd.FAST[kind](points, arrays, points.mean(axis=0))
                    self.assertTrue(np.all(fast <= direct + fd.FAST_MARGIN), f"{name} {kind}: fast form above the direct form")
                    if kind != "cone":  # the cone's fast form is the distance to the generatrix lines
                        self.assertLess(float(np.abs(fast - direct).max()), fd.FAST_MARGIN, f"{name} {kind}")

    def _ax3(params):
        from OCP.gp import gp_Ax3, gp_Dir, gp_Pnt as Pnt

        frame = gp_Ax3(Pnt(*[float(c) for c in params["o"]]), gp_Dir(*[float(c) for c in params["z"]]), gp_Dir(*[float(c) for c in params["x"]]))
        if float(np.dot(np.cross(params["z"], params["x"]), params["y"])) < 0.0:
            frame.YReverse()  # a surface with an indirect (left-handed) frame, as a reversed solid has
        return frame

    class Measurement(unittest.TestCase):
        def sample(self, shape, n, seed):
            """Points a little off the boundary of a solid: its faces' bounding-box corners and random
            interior points, so that some feet fall outside faces."""
            rng = np.random.default_rng(seed)
            solid = fd.SolidFaces(shape.wrapped)
            lo, hi = solid.lo.min(axis=0), solid.hi.max(axis=0)
            return solid, lo + rng.random((n, 3)) * (hi - lo)

        def brute(self, solid, points):
            return max(min(solid.occt_distance(f, p) for f in range(len(solid))) for p in points)

        def test_a_solid_of_analytic_faces_is_measured_over_all_points_in_closed_form(self):
            for name, shape in shapes().items():
                with self.subTest(name):
                    solid, points = self.sample(shape, 400, 5)
                    (row,), phases = fd.measure([solid], [points], workers=1, occt_points=0, block=64)
                    self.assertEqual(row["points"], row["points_available"], row)
                    scale = float(np.linalg.norm(solid.hi.max(axis=0) - solid.lo.min(axis=0)))
                    self.assertLessEqual(abs(row["worst"] - self.brute(solid, points)), 1e-9 * scale)
                    self.assertEqual(row["points_occt"], row["points_occt_needed"])

        def test_the_point_the_maximum_comes_from_is_reported(self):
            solid, points = self.sample(shapes()["torus"], 300, 6)
            (row,), _ = fd.measure([solid], [points], workers=1, occt_points=0, block=64)
            at = np.array(row["at"])
            found = min(solid.occt_distance(f, at) for f in range(len(solid)))
            self.assertAlmostEqual(found, row["worst"], places=7)

        def test_other_surfaces_are_measured_by_opencascade_on_a_capped_stride(self):
            shape = spline_solid()
            solid, points = self.sample(shape, 300, 7)
            self.assertFalse(solid.analytic.all(), "the test solid needs a non-analytic face")
            (full,), _ = fd.measure([solid], [points], workers=1, occt_points=0, block=64)
            self.assertEqual(full["points"], full["points_available"])
            scale = float(np.linalg.norm(solid.hi.max(axis=0) - solid.lo.min(axis=0)))
            self.assertLessEqual(abs(full["worst"] - self.brute(solid, points)), 1e-9 * scale)
            (capped,), _ = fd.measure([solid], [points], workers=1, occt_points=5, block=64)
            self.assertLessEqual(capped["points_occt"], 5)
            self.assertLess(capped["points"], capped["points_available"], "a capped run says it did not cover every point")
            self.assertLessEqual(capped["worst"], full["worst"] + 1e-9 * scale)
            self.assertGreaterEqual(capped["points_occt_needed"], capped["points_occt"])

        def test_workers_give_the_same_answer_as_one_process(self):
            solid, points = self.sample(shapes()["cone"], 500, 8)
            (one,), _ = fd.measure([solid], [points], workers=1, occt_points=0, block=100)
            (three,), _ = fd.measure([solid], [points], workers=3, occt_points=0, block=100)
            self.assertAlmostEqual(one["worst"], three["worst"], places=12)
            self.assertEqual(one["points"], three["points"])

    class Tessellation(unittest.TestCase):
        def setUp(self):
            self.dir = tempfile.TemporaryDirectory()
            self.addCleanup(self.dir.cleanup)
            self.path = Path(self.dir.name)

        def write_step(self, name, parts):
            step = self.path / f"{name}.step"
            assembly = cq.Assembly(name=name)
            for part_name, shape in parts:
                assembly.add(shape, name=part_name)
            assembly.export(str(step), "STEP")
            return step

        def tessellate(self, step, parts, chord_cm=0.1, angular=0.1, cache=None):
            import argparse

            tags = self.path / "tags.json"
            tags.write_text(json.dumps([{"step_index": i, "tag": n} for i, (n, _s) in enumerate(parts)]))
            out = self.path / "tess"
            args = argparse.Namespace(
                step=step, shape_unit_m=1e-3, tags=tags, out_dir=out, chord_cm=chord_cm, angular=angular,
                workers=2, shape_cache=cache, report=out / "tessellation.json",
            )
            out.mkdir(exist_ok=True)
            self.assertEqual(faris_cad.tessellate(args), 0)
            return json.loads((out / "tessellation.json").read_text()), out

        def test_each_solid_is_meshed_closed_and_with_its_analytic_volume(self):
            parts = [
                ("block", cq.Workplane().box(40, 30, 20).val()),
                ("ball", cq.Workplane().sphere(12).translate((100, 0, 0)).val()),
                ("ring", cq.Workplane().add(cq.Solid.makeTorus(30, 5)).translate((0, 100, 0)).val()),
            ]
            step = self.write_step("three", parts)
            report, out = self.tessellate(step, parts, chord_cm=0.02, angular=0.2)
            self.assertEqual(report["problems"], [])
            self.assertIn("BRepMesh_IncrementalMesh", report["method"])
            facets = faris_facets.unpack_facets((out / "solids.facets.bin").read_bytes())
            self.assertEqual(facets["flags"], faris_facets.FLAG_UNIMPRINTED)
            vertices, triangles = np.array(facets["vertices"]), np.array(facets["triangles"])
            expected = {1: 40e-3 * 30e-3 * 20e-3, 2: 4 / 3 * math.pi * 12e-3**3, 3: 2 * math.pi**2 * 30e-3 * (5e-3) ** 2}
            for volume, analytic in expected.items():
                owned = [t for s in facets["surfaces"] if s[2] == volume for t in range(s[0], s[0] + s[1])]
                p = vertices[triangles[owned]]
                signed = float(np.einsum("ij,ij->i", p[:, 0], np.cross(p[:, 1], p[:, 2])).sum() / 6.0)
                self.assertGreater(signed, 0.0, "outward orientation")
                self.assertLess(abs(signed - analytic) / analytic, 0.03, f"volume {volume}")

        def test_an_open_mesh_is_reported_as_a_tessellation_problem_and_not_repaired(self):
            closed = np.array([[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]])
            self.assertEqual(faris_cad.open_edge_report(closed)[:2], (0, 0))
            unpaired, repeated, first = faris_cad.open_edge_report(closed[:3])
            self.assertEqual((unpaired, repeated), (3, 0))
            self.assertIn(first, (0, 1, 2))
            flipped = closed.copy()
            flipped[0] = flipped[0][::-1]
            unpaired, repeated, _ = faris_cad.open_edge_report(flipped)
            self.assertGreater(unpaired + repeated, 0, "an inconsistently oriented triangle is not a closed mesh")

        def test_the_solids_are_cached_by_step_hash_and_helper_version(self):
            parts = [("block", cq.Workplane().box(10, 10, 10).val())]
            step = self.write_step("cache", parts)
            cache = self.path / "shapes"
            first = faris_cad.load_solids(step, cache)
            self.assertTrue((cache / "solids.brep").is_file())
            meta = json.loads((cache / "solids.json").read_text())
            self.assertEqual(meta["step_sha256"], faris_cad.sha256_file(step))
            self.assertEqual(meta["helper"], faris_cad.HELPER_VERSION)
            second = faris_cad.load_solids(step, cache)
            self.assertEqual(len(first), len(second))
            volumes = [faris_cad._volume_m3(s, 1.0) for s in (first[0], second[0])]
            self.assertAlmostEqual(volumes[0], volumes[1], places=9)
            # another STEP file never reads this cache
            other = self.write_step("other", [("a", cq.Workplane().box(10, 10, 10).val()), ("b", cq.Workplane().box(5, 5, 5).translate((50, 0, 0)).val())])
            self.assertEqual(len(faris_cad.load_solids(other, cache)), 2)


if __name__ == "__main__":
    unittest.main()
