# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import importlib.util
import math
import subprocess
import sys
import unittest
from pathlib import Path
from validation.manifest import load_manifest
from validation import report as report_mod
from validation.matrix import load_registry

ROOT = Path(__file__).resolve().parent.parent
CASE = ROOT / "validation" / "cases" / "fns-tof-jade-v1"
sys.path.insert(0, str(CASE))
sys.path.insert(0, str(ROOT))


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


B = load("fns_tof_benchmark", CASE / "benchmark.py")
R = load("fns_tof_run_case", CASE / "run_case.py")
BM = load("fns_tof_build_manifest", CASE / "build_manifest.py")


DECK = """FNS synthetic mapping fixture
c
1 1 -1.0 -1
2 0 1
1 so 1000
mode n
sdef erg=d1 pos=0 0 -5
# SI1 SP1
1.0 0.0
2.0 0.25
4.0 0.75
m1 4009.41c 1
f5:n 0 0 10 1 nd
fc5 Neutron leakage flux at 0 degrees
fm5 2.0
f25:n 2 0 10 1 nd
fc25 Neutron leakage flux at 24.9 degrees
fm25 2.1
e0 0.5
1.0
"""


def test_manifest():
    return {"detectors": [
        {"id": "angle-0", "reference": {"value": 1.0}},
        {"id": "angle-24", "reference": {"value": 2.0}},
    ], "adapter": {"tables": [
        {"angle_deg": 0.0, "tally_number": 5, "mcnp_FM": 2.0, "coarse": False,
         "detector_ids": [{"detector_id": "angle-0", "bin_index": 0, "lo_mev": 0.1, "hi_mev": 0.5}]},
        {"angle_deg": 24.9, "tally_number": 25, "mcnp_FM": 2.1, "coarse": True,
         "detector_ids": [{"detector_id": "angle-24", "bin_index": 1, "lo_mev": 0.1, "hi_mev": 1.0}]},
    ]}}


class FnsTofHelpers(unittest.TestCase):
    def test_histogram_translation_has_equal_openmc_x_p_lengths_and_preserves_integral(self):
        translated = B.openmc_histogram([1.0, 2.0, 4.0], [0.0, 2.0, 6.0])
        self.assertEqual(len(translated["edges_ev"]), len(translated["pdf_per_ev"]))
        self.assertAlmostEqual(sum(p * (b - a) for a, b, p in zip(
            translated["edges_ev"][:-1], translated["edges_ev"][1:], translated["pdf_per_ev"][:-1])), 1.0)
        self.assertEqual(translated["pdf_per_ev"][-1], 0.0)

    def test_native_vertical_source_card_and_five_card_tally_mapping(self):
        parsed = B.input_deck(DECK)
        self.assertTrue(parsed["source_table_is_literal_input"])
        self.assertEqual([(d["tally"], d["angle_deg"]) for d in parsed["detectors"]], [(5, 0.0), (25, 24.9)])
        mapping = R.validate_response_mapping(test_manifest(), parsed)
        self.assertEqual(mapping["angle-0"]["tally"], 5)
        self.assertEqual(mapping["angle-24"]["tally"], 25)

    def test_published_absolute_error_is_preserved_and_inconsistent_rows_rejected(self):
        rows = B.read_rows(b"Energy,Value,Error,abs err,Angle\n1.0,2.0,0.1,0.2,0 deg\n", "fine.csv")
        self.assertEqual(rows[0]["error_absolute"], 0.2)
        self.assertEqual(rows[0]["absolute_error_source"], "abs err")
        with self.assertRaisesRegex(ValueError, r"does not match Value\*Error"):
            B.read_rows(b"Energy,Value,Error,abs err,Angle\n1.0,2.0,0.1,0.3,0 deg\n", "bad.csv")
        coarse = B.read_rows(b"Value,Error,Energy,Angle\n2.0,0.1,0.1 - 1.0,0 deg\n", "coarse.csv")
        self.assertEqual(coarse[0]["error_absolute"], 0.2)
        self.assertEqual(coarse[0]["absolute_error_source"], "Value*Error derived from source Error column")

    def test_mapping_rejects_positionally_plausible_but_wrong_tally_id(self):
        parsed = B.input_deck(DECK)
        manifest = test_manifest()
        manifest["adapter"]["tables"][0]["tally_number"] = 25
        with self.assertRaisesRegex(ValueError, "different tally ID"):
            R.validate_response_mapping(manifest, parsed)

    def test_mapping_rejects_missing_source_angle_instead_of_filling_by_order(self):
        parsed = B.input_deck(DECK)
        manifest = test_manifest()
        manifest["adapter"]["tables"][0]["angle_deg"] = 12.2
        with self.assertRaisesRegex(ValueError, "no exact FC-labeled tally"):
            R.validate_response_mapping(manifest, parsed)

    def test_mcnp_fm_is_applied_once_and_flux_is_per_unit_lethargy(self):
        result = R.tally_value(3.0, 0.3, 2.0, 1.0, 2.0)
        self.assertAlmostEqual(result["value"], 6.0 / math.log(2.0))
        self.assertAlmostEqual(result["u_mc"], 0.6 / math.log(2.0))
        self.assertAlmostEqual(B.lethargy_width(0.0, 0.01), math.log(0.01 / 1e-10))

    def test_outer_sphere_must_be_preserved_as_vacuum_boundary(self):
        class Surface:
            boundary_type = "vacuum"
        class Geometry:
            def get_all_surfaces(self):
                return {1: Surface()}
        class Model:
            geometry = Geometry()
        parsed = B.input_deck(DECK)
        self.assertEqual(R.require_vacuum_outer_sphere(Model(), parsed), 1)
        Model.geometry.get_all_surfaces = lambda: {1: type("Reflective", (), {"boundary_type": "reflective"})()}
        with self.assertRaisesRegex(ValueError, "expected vacuum"):
            R.require_vacuum_outer_sphere(Model(), parsed)

    def test_nonfinite_or_nonphysical_tallies_are_rejected(self):
        for values in ((math.nan, 0.1, 2.0, 1.0, 2.0), (1.0, math.inf, 2.0, 1.0, 2.0),
                       (1.0, -0.1, 2.0, 1.0, 2.0), (1.0, 0.1, -2.0, 1.0, 2.0)):
            with self.subTest(values=values), self.assertRaises(ValueError):
                R.tally_value(*values)

    def test_finite_volume_gate_requires_two_all_bin_successive_halvings(self):
        levels = [
            {"radius_cm": 2.5, "bins": {"a": {"value": 10.0, "u_mc": 0.1}, "b": {"value": 5.0, "u_mc": 0.05}}},
            {"radius_cm": 1.25, "bins": {"a": {"value": 10.02, "u_mc": 0.1}, "b": {"value": 5.01, "u_mc": 0.05}}},
            {"radius_cm": 0.625, "bins": {"a": {"value": 10.01, "u_mc": 0.1}, "b": {"value": 5.02, "u_mc": 0.05}}},
        ]
        self.assertTrue(B.qualify_halvings(levels)["qualified"])
        levels[-1]["bins"]["b"]["value"] = 5.2
        self.assertFalse(B.qualify_halvings(levels)["qualified"])

    def test_finite_volume_gate_keeps_rare_or_missing_rows_inconclusive(self):
        levels = [
            {"radius_cm": 2.5, "bins": {"rare": {"value": 0.0, "u_mc": 0.0}}},
            {"radius_cm": 1.25, "bins": {"rare": {"value": 1.0, "u_mc": 0.01}}},
            {"radius_cm": 0.625, "bins": {"rare": {"value": 1.0, "u_mc": 0.01}}},
        ]
        self.assertFalse(B.qualify_halvings(levels)["qualified"])
        levels[1]["bins"] = {}
        self.assertFalse(B.qualify_halvings(levels)["qualified"])

    def test_run_requires_explicit_cpu_consent_before_importing_solver(self):
        args = type("Args", (), {"i_have_the_cpu": False})()
        with self.assertRaisesRegex(ValueError, "--i-have-the-cpu"):
            R.command_run(args)

    def test_generated_public_manifests_keep_every_table_row_and_exact_tally_join(self):
        cases_root = ROOT / "validation" / "cases"
        manifests = sorted(cases_root.glob("fns-tof-*/manifest.json"))
        self.assertEqual(len(manifests), 14)
        for path in manifests:
            with self.subTest(manifest=path.name):
                self.assertLess(path.stat().st_size, 1_000_000)
                manifest = load_manifest(path)
                self.assertEqual(path.read_text(encoding="utf-8"), BM.render_manifest(manifest))
                self.assertEqual(path.parent.name, manifest["case_id"])
                points = manifest["adapter"]["detectors"]
                mapping = R.validate_response_mapping(manifest, {"detectors": points})
                self.assertEqual(set(mapping), {d["id"] for d in manifest["detectors"]})
                self.assertEqual(len(mapping), sum(len(t["detector_ids"]) for t in manifest["adapter"]["tables"]))
                self.assertTrue(manifest["files_are_external_not_vendored"])
                self.assertEqual(manifest["license"]["spdx"], "CC-BY-4.0")
                self.assertTrue(all(t["path_in_source"] for t in manifest["adapter"]["tables"]))
                self.assertTrue(all(t["geometry_factor_check"]["relative_difference"] < 0.004
                                    for t in manifest["adapter"]["tables"]))
                if manifest["case_id"] == "fns-tof-fe-60":
                    self.assertEqual(manifest["adapter"]["missing_table_angles_deg"], [66.8])
                if manifest["case_id"] == "fns-tof-c-5":
                    self.assertEqual(len({t["angle_deg"] for t in manifest["adapter"]["tables"]}), 4)

    def test_standard_discovery_registers_all_flux_rows_without_validating_them(self):
        discovered = [load_manifest(path) for path in report_mod.discover()]
        fns_ids = {m["case_id"] for m in discovered if m["case_id"].startswith("fns-tof-")}
        self.assertEqual(fns_ids, {path.parent.name for path in (ROOT / "validation" / "cases").glob("fns-tof-*/manifest.json")})
        suite = report_mod.build_suite(discovered, {}, None, registry=load_registry())
        rows = [row for row in suite["matrix"]["rows"] if row["benchmark_id"].startswith("fns-tof-")]
        self.assertEqual(len(rows), 14)
        self.assertTrue(all(row["status"] == "not validated" for row in rows))
        self.assertEqual({row["benchmark_id"].split("/")[0] for row in rows}, fns_ids)
        flux = next(q for q in load_registry()["quantities"] if q["id"] == "flux")
        registered = {b["case_id"] for b in flux["benchmarks"] if b["case_id"].startswith("fns-tof-")}
        self.assertEqual(registered, fns_ids)
        self.assertTrue(all("not validation" in b["scope"] and "two-halving" in b["scope"]
                            for b in flux["benchmarks"] if b["case_id"].startswith("fns-tof-")))

    def test_cli_help_is_callable_by_script_path(self):
        result = subprocess.run([sys.executable, str(CASE / "run_case.py"), "--help"],
                                check=True, capture_output=True, text=True)
        self.assertIn("three-radius transport qualification", result.stdout)

    def test_manifest_builder_help_is_callable_by_script_path(self):
        result = subprocess.run([sys.executable, str(CASE / "build_manifest.py"), "--help"],
                                check=True, capture_output=True, text=True)
        self.assertIn("clean IAEA-NDS/open-benchmarks checkout", result.stdout)


if __name__ == "__main__":
    unittest.main()
