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
    import OCP  # noqa: F401

    HAVE_CAD = True
except ImportError:
    HAVE_CAD = False


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
