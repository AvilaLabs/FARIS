# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the CAD helper. Standard library only in the test process.

STEP files are generated and inspected by the CAD interpreter in a subprocess
(FARIS_CAD_PYTHON, default ~/.cache/avila-night/feasibility-cad/cadvenv), so
these tests run under any Python and skip when no CAD interpreter exists.
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
import faris_cad  # noqa: E402  (imports no CAD module until a function needs one)

CAD_PYTHON = Path(
    os.environ.get(
        "FARIS_CAD_PYTHON",
        str(Path.home() / ".cache/avila-night/feasibility-cad/cadvenv/bin/python"),
    )
)

WRITE = """
import sys, cadquery as cq
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs
unit, out, mode = sys.argv[1], sys.argv[2], sys.argv[3]
box = cq.Workplane().box(10, 20, 30).val()
cyl = cq.Workplane().center(50, 0).circle(5).extrude(10).val()
if mode == "named":
    asm = cq.Assembly()
    asm.add(box, name="Box One")
    asm.add(cyl, name="cyl")
    shape = asm.toCompound().wrapped
    # names need the assembly writer; unit is set through the same static
    Interface_Static.SetCVal_s("write.step.unit", unit)
    asm.save(out)
    sys.exit(0)
if mode == "two-boxes":
    other = cq.Workplane().center(100, 0).box(4, 6, 8).val()
    shape = cq.Compound.makeCompound([box, other]).wrapped
elif mode == "overlapping-boxes":
    other = cq.Workplane().center(4, 0).box(10, 20, 30).val()
    shape = cq.Compound.makeCompound([box, other]).wrapped
else:
    shape = cq.Compound.makeCompound([box, cyl]).wrapped
writer = STEPControl_Writer()
Interface_Static.SetCVal_s("write.step.unit", unit)
writer.Transfer(shape, STEPControl_AsIs)
writer.Write(out)
"""


def make_step(path, unit="MM", mode="plain"):
    subprocess.run(
        [str(CAD_PYTHON), "-c", WRITE, unit, str(path), mode],
        check=True,
        capture_output=True,
        timeout=30,
    )


def inspect(step, *extra):
    out = Path(step).with_suffix(".json")
    result = subprocess.run(
        [str(CAD_PYTHON), str(HERE / "faris_cad.py"), "step-inspect", str(step), "--out", str(out), *extra],
        capture_output=True, text=True,
    )
    return result, (json.loads(out.read_text()) if out.exists() else None)


def close(a, b, rel=1e-9, scale=1.0):
    return abs(a - b) <= rel * max(abs(b), scale)


@unittest.skipUnless(CAD_PYTHON.is_file(), "no CAD interpreter (set FARIS_CAD_PYTHON)")
class StepInspect(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)

    def check_box_and_cylinder(self, report, scale=1.0):
        box, cyl = report["solids"]
        # analytic: 10x20x30 mm box at the origin; r=5 h=10 mm cylinder on z=0 at x=50 mm
        self.assertTrue(close(box["cad_volume_m3"], 6000e-9))
        self.assertTrue(close(cyl["cad_volume_m3"], math.pi * 25 * 10 * 1e-9))
        for got, want in zip(box["centroid_m"], [0, 0, 0]):
            self.assertLess(abs(got - want), 1e-12)
        for got, want in zip(cyl["centroid_m"], [50e-3, 0, 5e-3]):
            self.assertLess(abs(got - want), 1e-9 * 50e-3)
        self.assertEqual([s["step_index"] for s in report["solids"]], [0, 1])
        self.assertTrue(all(s["occt_valid"] for s in report["solids"]))
        # model box: x -5 to 55 mm, y +-10 mm, z +-15 mm
        diagonal = math.sqrt(0.060**2 + 0.020**2 + 0.030**2)
        self.assertTrue(close(report["model_bbox_diagonal_m"], diagonal, rel=1e-6))

    # Verifies: GEO-010, GEO-011
    def test_box_and_cylinder_in_mm(self):
        step = self.path / "mm.step"
        make_step(step, "MM")
        result, report = inspect(step)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report["schema"], "faris-step-inspect/v1")
        self.assertEqual(report["unit"]["declared"], "mm")
        self.assertEqual(report["unit"]["problems"], [])
        self.assertFalse(report["unit"]["used_is_assumed"])
        self.check_box_and_cylinder(report)
        self.assertEqual(len(report["step"]["sha256"]), 64)
        self.assertEqual(report["step"]["sha256"], faris_cad.sha256_file(step))

    # Verifies: GEO-011
    def test_same_model_in_metres_and_inches_gives_same_si_numbers(self):
        for unit, label in (("M", "m"), ("INCH", "inch")):
            step = self.path / f"{label}.step"
            make_step(step, unit)
            result, report = inspect(step)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(report["unit"]["declared"], label)
            self.check_box_and_cylinder(report)

    def test_unnamed_and_named_solids(self):
        plain = self.path / "plain.step"
        make_step(plain, "MM", "plain")
        _, report = inspect(plain)
        self.assertEqual([s["step_name"] for s in report["solids"]], [None, None])
        named = self.path / "named.step"
        make_step(named, "MM", "named")
        _, report = inspect(named)
        self.assertEqual([s["step_name"] for s in report["solids"]], ["Box One", "cyl"])
        self.check_box_and_cylinder(report)

    def test_two_boxes(self):
        step = self.path / "boxes.step"
        make_step(step, "MM", "two-boxes")
        _, report = inspect(step)
        small = report["solids"][1]
        self.assertTrue(close(small["cad_volume_m3"], 4 * 6 * 8 * 1e-9))
        self.assertTrue(close(small["centroid_m"][0], 100e-3))

    def test_no_declared_unit_uses_assumed_unit_and_says_so(self):
        step = self.path / "nounit.step"
        make_step(step, "MM")
        text = step.read_text(encoding="latin-1")
        step.write_text(text.replace("LENGTH_UNIT()", "UNKNOWN_UNIT()"), encoding="latin-1")
        result, report = inspect(step)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNone(report["unit"]["declared"])
        self.assertEqual(report["unit"]["problems"], [])
        self.assertIsNone(report["solids"][0]["cad_volume_m3"])
        # the same numbers read as millimetres give the analytic SI values
        result, report = inspect(step, "--assume-unit", "mm")
        self.assertTrue(report["unit"]["used_is_assumed"])
        self.check_box_and_cylinder(report)
        _, report = inspect(step, "--assume-unit", "m")
        self.assertTrue(close(report["solids"][0]["cad_volume_m3"], 6000.0))

    # Verifies: GEO-011
    def test_several_length_units_are_a_problem_and_give_no_si_numbers(self):
        step = self.path / "mixed.step"
        make_step(step, "MM", "named")
        text = step.read_text(encoding="latin-1")
        first = text.index("SI_UNIT(.MILLI.,.METRE.)")
        second = text.index("SI_UNIT(.MILLI.,.METRE.)", first + 1)
        text = text[:second] + "SI_UNIT($,.METRE.)" + text[second + len("SI_UNIT(.MILLI.,.METRE.)"):]
        step.write_text(text, encoding="latin-1")
        result, report = inspect(step)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(report["unit"]["problems"])
        self.assertIsNone(report["unit"]["used"])
        self.assertIsNone(report["solids"][0]["cad_volume_m3"])

    def test_unreadable_file_is_reported_not_raised(self):
        step = self.path / "bad.step"
        step.write_text("not a step file")
        result, report = inspect(step)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(report["problems"])
        self.assertEqual(report["solids"], [])

    def test_missing_file_exits_2(self):
        result, report = inspect(self.path / "missing.step")
        self.assertEqual(result.returncode, 2)
        self.assertIsNone(report)


@unittest.skipUnless(CAD_PYTHON.is_file(), "no CAD interpreter (set FARIS_CAD_PYTHON)")
class OverlapPipeline(unittest.TestCase):
    """Exercise the real OCCT per-solid tessellator and boolean used before DAGMC imprinting."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)

    # Verifies: GEO-021
    def test_overlapping_step_solids_are_faceted_individually_and_boolean_reports_the_common_volume(self):
        step = self.path / "overlap.step"
        make_step(step, "MM", "overlapping-boxes")
        tags = self.path / "tags.json"
        tags.write_text(json.dumps([
            {"step_index": 0, "tag": "core"},
            {"step_index": 1, "tag": "shell"},
        ]))

        facets_dir = self.path / "tessellated"
        tess_report = facets_dir / "tessellation.json"
        tess = subprocess.run(
            [str(CAD_PYTHON), str(HERE / "faris_cad.py"), "tessellate", str(step),
             "--shape-unit-m", "0.001", "--tags", str(tags), "--out-dir", str(facets_dir),
             "--report", str(tess_report), "--workers", "1"],
            capture_output=True, text=True,
            timeout=90,
        )
        self.assertEqual(tess.returncode, 0, tess.stderr)
        self.assertEqual(json.loads(tess_report.read_text())["problems"], [])
        self.assertTrue((facets_dir / "solids.facets.bin").is_file())

        pairs = self.path / "pairs.json"
        pairs.write_text("[[0, 1]]\n")
        boolean_report = self.path / "booleans.json"
        boolean = subprocess.run(
            [str(CAD_PYTHON), str(HERE / "faris_cad.py"), "overlap-booleans", str(step),
             "--shape-unit-m", "0.001", "--pairs", str(pairs), "--report", str(boolean_report),
             "--workers", "1", "--pair-timeout", "30"],
            capture_output=True, text=True,
            timeout=45,
        )
        self.assertEqual(boolean.returncode, 0, boolean.stderr)
        result = json.loads(boolean_report.read_text())["pairs"][0]
        self.assertEqual(result["status"], "decided", result)
        # The boxes are displaced 4 mm along x, so their 6 x 20 x 30 mm intersection is 3,600 mm3.
        self.assertAlmostEqual(result["common_volume_m3"], 3_600e-9, delta=3_600e-9 * 1e-8)

    # Verifies: GEO-012
    def test_clean_step_runs_through_cad_to_dagmc_and_exports_facets(self):
        available = subprocess.run(
            [str(CAD_PYTHON), "-c", "import cad_to_dagmc, h5py"],
            capture_output=True, text=True, timeout=90,
        )
        if available.returncode:
            self.skipTest("CAD interpreter lacks cad_to_dagmc or h5py")

        step = self.path / "clean.step"
        make_step(step, "MM", "two-boxes")
        tags = self.path / "tags.json"
        tags.write_text(json.dumps([
            {"step_index": 0, "tag": "core"},
            {"step_index": 1, "tag": "shell"},
        ]))
        out_dir = self.path / "conversion"
        report = out_dir / "conversion.json"
        converted = subprocess.run(
            [str(CAD_PYTHON), str(HERE / "faris_cad.py"), "convert", str(step),
             "--shape-unit-m", "0.001", "--tags", str(tags), "--out-dir", str(out_dir),
             "--report", str(report), "--chord-cm", "0.1", "--angular", "0.1",
             "--imprint-threads", "1"],
            capture_output=True, text=True, timeout=180,
        )
        self.assertEqual(converted.returncode, 0, converted.stderr)
        result = json.loads(report.read_text())
        self.assertEqual(result["schema"], "faris-conversion/v1")
        self.assertEqual(result["tags"], json.loads(tags.read_text()))
        self.assertEqual(result["counts"]["volumes"], 2)
        self.assertEqual(result["h5m"]["sha256"], faris_cad.sha256_file(out_dir / "model.h5m"))
        self.assertTrue((out_dir / "model.producer.h5m").is_file())
        correction = result["h5m"]["faceting_tolerance_metadata"]
        with h5py.File(out_dir / "model.h5m", "r") as converted_h5m:
            dagmc_root_set_handle = int(converted_h5m["tstt/sets/list"].attrs["start_id"])
        self.assertEqual(correction["tag_name"], "FACETING_TOL")
        self.assertEqual(correction["tag_class"], "MB_TAG_SPARSE (1)")
        self.assertGreater(dagmc_root_set_handle, 0)
        self.assertEqual(correction["entity_handle"], dagmc_root_set_handle)
        with h5py.File(out_dir / "model.h5m", "r") as converted_h5m:
            self.assertEqual(
                converted_h5m["tstt/tags/FACETING_TOL/id_list"][()].tolist(),
                [dagmc_root_set_handle],
            )
        self.assertEqual(correction["declared_mesher_tolerance_cm"], 0.1)
        self.assertTrue(correction["source_unchanged"])
        self.assertIn("not a certified maximum", correction["interpretation"])
        self.assertTrue((out_dir / result["facets"]["bin"]).is_file())


class UnitText(unittest.TestCase):
    """Text-parse logic, no CAD kernel."""

    def parse(self, body):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "u.step"
            path.write_text(body, encoding="latin-1")
            return faris_cad.text_length_units(path)

    def test_si_prefixes(self):
        for prefix, factor in ((".MILLI.", 1e-3), (".CENTI.", 1e-2), ("$", 1.0)):
            factors, problems = self.parse(
                "#3 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#1,#2));\n"
                f"#1 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT({prefix},.METRE.) );\n"
                "#2 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );"
            )
            self.assertEqual((factors, problems), ([factor], []))

    def test_inch_by_conversion_does_not_count_its_base_unit(self):
        body = (
            "#9 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#1));\n"
            "#1 = ( CONVERSION_BASED_UNIT('INCH',#2) LENGTH_UNIT() NAMED_UNIT(#3) );\n"
            "#2 = LENGTH_MEASURE_WITH_UNIT(25.4,#4);\n"
            "#4 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\n"
        )
        factors, problems = self.parse(body)
        self.assertEqual(problems, [])
        self.assertEqual([faris_cad.unit_label(f) for f in factors], ["inch"])

    def test_inch_with_wrapped_measure(self):
        body = (
            "#9 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#1));\n"
            "#1 = ( CONVERSION_BASED_UNIT('INCH',#2) LENGTH_UNIT() NAMED_UNIT(#3) );\n"
            "#2 = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#4);\n"
            "#4 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\n"
        )
        factors, _ = self.parse(body)
        self.assertEqual([faris_cad.unit_label(f) for f in factors], ["inch"])

    def test_distinct_units_are_listed_once_each(self):
        body = (
            "#7 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#1));\n#8 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#2));\n"
            "#9 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#3));\n"
            "#1 = ( LENGTH_UNIT() SI_UNIT(.MILLI.,.METRE.) );\n#2 = ( LENGTH_UNIT() SI_UNIT(.MILLI.,.METRE.) );\n"
            "#3 = ( LENGTH_UNIT() SI_UNIT($,.METRE.) );"
        )
        factors, _ = self.parse(body)
        self.assertEqual(factors, [1e-3, 1.0])

    def test_no_unit(self):
        self.assertEqual(self.parse("#1 = CARTESIAN_POINT('',(0.,0.,0.));"), ([], []))

    def test_unreadable_unit_is_a_problem(self):
        factors, problems = self.parse(
            "#9 = GLOBAL_UNIT_ASSIGNED_CONTEXT((#1));\n#1 = ( LENGTH_UNIT() NAMED_UNIT(*) MYSTERY_UNIT() );"
        )
        self.assertEqual(factors, [])
        self.assertTrue(problems)


if __name__ == "__main__":
    unittest.main()
