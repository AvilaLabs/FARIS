import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).parent
spec = importlib.util.spec_from_file_location("build_nuclear_data_v2", HERE / "build_nuclear_data_v2.py")
build = importlib.util.module_from_spec(spec)
sys.modules["build_nuclear_data_v2"] = build
spec.loader.exec_module(build)
import audit_library  # noqa: E402  (path added by the module under test)

ABUNDANCE = {"Fe54": 0.058, "Fe56": 0.917, "Ta180_m1": 0.0001, "Ta181": 0.9999, "H1": 0.9998, "H2": 0.0002, "He3": 1e-6}

XML = """<?xml version='1.0'?><cross_sections>
<library materials="H1" path="neutron/H1.h5" type="neutron"/>
<library materials="Fe56 Fe54" path="neutron/Fe.h5" type="neutron"/>
<library materials="c_Be" path="thermal/c_Be.h5" type="thermal"/>
<library materials="H" path="photon/H.h5" type="photon"/>
</cross_sections>"""


class PureParts(unittest.TestCase):
    def test_parse_cross_sections(self):
        maps = build.parse_cross_sections(XML)
        self.assertEqual(maps["neutron"]["Fe54"], "neutron/Fe.h5")
        self.assertEqual(maps["thermal"], {"c_Be": "thermal/c_Be.h5"})
        self.assertEqual(maps["photon"], {"H": "photon/H.h5"})

    def test_element_of(self):
        self.assertEqual(build.element_of("Ta180_m1"), "Ta")
        self.assertEqual(build.element_of("H2"), "H")
        self.assertEqual(build.element_of("Fe56"), "Fe")
        with self.assertRaises(ValueError):
            build.element_of("123")

    def test_plan_neutron_prefers_fendl_and_lists_gap_fill(self):
        fendl = {"H1": "neutron/H1.h5", "Ta181": "neutron/Ta181.h5"}
        viii = {"H1": "neutron/H1.h5", "Ta180_m1": "neutron/Ta180_m1.h5", "Eu153": "neutron/Eu153.h5"}
        plan = build.plan_neutron(fendl, viii, ["H1", "Ta180_m1", "Ta181", "Xx999"])
        self.assertEqual(plan["gap_filled"], ["Ta180_m1"])
        self.assertEqual(plan["entries"]["H1"]["source_library"], "FENDL-3.2")
        self.assertEqual(plan["entries"]["Ta180_m1"]["source_library"], "ENDF/B-VIII.1")
        self.assertNotIn("Eu153", plan["entries"])
        self.assertEqual([g["name"] for g in plan["gaps"]], ["Xx999"])

    def test_missing_element_is_a_gap(self):
        gaps = build.needed_elements_without_isotopes(["Fe", "Eu"], {"Fe56"})
        self.assertEqual([g["name"] for g in gaps], ["Eu"])

    def test_photon_members(self):
        names = ["photoat/", "photoat/photoat-026_Fe_000.endf", "atomic_relax/atom-001_H_000.endf", "photoat/readme.txt"]
        self.assertEqual(build.photon_members(names, "photoat"), {"Fe": (26, "photoat/photoat-026_Fe_000.endf")})
        self.assertEqual(build.photon_members(names, "atomic_relax"), {"H": (1, "atomic_relax/atom-001_H_000.endf")})

    def test_thermal_plan(self):
        plan = build.thermal_plan({"c_Be": "thermal/c_Be.h5"}, ["c_Be", "c_Nope"])
        self.assertEqual(list(plan["tables"]), ["c_Be"])
        self.assertEqual(plan["gaps"][0]["name"], "c_Nope")

    def test_render_is_deterministic_and_relative(self):
        a = build.render_cross_sections({"Fe56": "neutron/Fe56.h5", "H1": "neutron/H1.h5"}, {"H": "photon/H.h5"}, {"c_Be": "thermal/c_Be.h5"})
        b = build.render_cross_sections({"H1": "neutron/H1.h5", "Fe56": "neutron/Fe56.h5"}, {"H": "photon/H.h5"}, {"c_Be": "thermal/c_Be.h5"})
        self.assertEqual(a, b)
        self.assertNotIn('path="/', a)
        self.assertLess(a.index("Fe56"), a.index('"H1"'))
        self.assertEqual(build.parse_cross_sections(a)["thermal"], {"c_Be": "thermal/c_Be.h5"})

    def test_existing_root_needs_force_and_marker(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "lib"
            build.validate_root(root, False)
            root.mkdir()
            with self.assertRaises(SystemExit):
                build.validate_root(root, False)
            with self.assertRaises(SystemExit):
                build.validate_root(root, True)
            (root / build.MARKER).write_text("x")
            build.validate_root(root, True)


class FusionSet(unittest.TestCase):
    def test_natural_isotopes_keep_metastable_and_skip_other_elements(self):
        self.assertEqual(audit_library.natural_isotopes("Ta", ABUNDANCE), ["Ta180_m1", "Ta181"])
        self.assertEqual(audit_library.natural_isotopes("H", ABUNDANCE), ["H1", "H2"])
        self.assertEqual(audit_library.natural_isotopes("He", ABUNDANCE), ["He3"])

    def test_selection_flags_elements_without_isotopes(self):
        selection = audit_library.fusion_materials_v1_selection(ABUNDANCE)
        self.assertIn("Fe56", selection["nuclides"])
        self.assertIn("Zr", selection["elements_without_natural_isotopes"])
        self.assertEqual(selection["name"], "fusion-materials-v1")

    def test_every_material_has_sources_and_known_elements(self):
        for name, entry in audit_library.FUSION_MATERIALS_V1.items():
            self.assertTrue(entry["source"], name)
            self.assertTrue(entry["elements"], name)
        required = {"SS316L", "Inconel 718", "EUROFER97", "CuCrZr", "WC", "V-4Cr-4Ti", "TZM", "FLiBe", "PbLi", "B4C",
                    "TiH2", "ZrH2", "Li4SiO4", "Li2TiO3", "Be12Ti", "REBCO tape", "Nb3Sn", "NbTi", "Copper"}
        self.assertLessEqual(required, set(audit_library.FUSION_MATERIALS_V1))


if __name__ == "__main__":
    unittest.main()
