"""Tests for integrations/openmc/material_audit.py.

The script needs OpenMC, h5py and the audited cross_sections.xml, so the tests
run it in a subprocess and skip unless both are configured:

    FARIS_OPENMC_PYTHON=/path/to/python FARIS_CROSS_SECTIONS=/path/to/cross_sections.xml \\
        python3 -m unittest discover -s integrations/cad -p 'test_*.py'
"""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "openmc" / "material_audit.py"
PYTHON = os.environ.get("FARIS_OPENMC_PYTHON")
XS = os.environ.get("FARIS_CROSS_SECTIONS")

REQUEST = {
    "materials": [
        {"id": "tungsten", "kind": "nuclides", "atom_fractions": {"W182": 0.5, "W184": 0.5}},
        {
            "id": "pack",
            "kind": "recipe",
            "basis": "weight",
            "components": [
                {"element": "Cu", "fraction": 0.6},
                {"element": "Fe", "fraction": 0.3},
                {"element": "Li", "fraction": 0.1, "isotopes": {"Li6": 0.9, "Li7": 0.1}},
            ],
        },
        {"id": "atoms", "kind": "recipe", "basis": "atom",
         "components": [{"nuclide": "Be9", "fraction": 1.0}]},
        {"id": "bad-element", "kind": "recipe", "basis": "atom",
         "components": [{"element": "Xx", "fraction": 1.0}]},
        {"id": "missing", "kind": "nuclides", "atom_fractions": {"Fe999": 1.0}},
    ]
}


@unittest.skipUnless(PYTHON and XS and Path(PYTHON).is_file() and Path(XS).is_file(),
                     "set FARIS_OPENMC_PYTHON and FARIS_CROSS_SECTIONS")
class MaterialAudit(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        request = Path(cls.dir.name) / "request.json"
        out = Path(cls.dir.name) / "audit.json"
        request.write_text(json.dumps(REQUEST))
        result = subprocess.run(
            [PYTHON, str(SCRIPT), "--request", str(request), "--cross-sections", XS, "--out", str(out)],
            capture_output=True, text=True,
        )
        assert result.returncode == 0, result.stderr
        cls.report = json.loads(out.read_text())
        cls.materials = {m["id"]: m for m in cls.report["materials"]}

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def test_schema_and_versions(self):
        self.assertEqual(self.report["schema"], "faris-material-audit/v1")
        self.assertTrue(self.report["openmc"])
        self.assertEqual(len(self.report["cross_sections_sha256"]), 64)

    def test_a_ready_vector_passes_through(self):
        self.assertEqual(self.materials["tungsten"]["nuclides"], {"W182": 0.5, "W184": 0.5})

    def test_weight_basis_with_isotope_split_expands_to_atom_fractions(self):
        pack = self.materials["pack"]["nuclides"]
        self.assertAlmostEqual(sum(pack.values()), 1.0, places=12)
        # weight 0.6 Cu, 0.3 Fe, 0.1 Li -> moles; masses 63.546, 55.845, Li 6.0151*0.9+7.016*0.1
        li_mass = 0.9 * 6.0151228874 + 0.1 * 7.0160034366
        moles = {"Cu": 0.6 / 63.546, "Fe": 0.3 / 55.845, "Li": 0.1 / li_mass}
        total = sum(moles.values())
        cu = pack["Cu63"] + pack["Cu65"]
        li = pack["Li6"] + pack["Li7"]
        self.assertAlmostEqual(cu, moles["Cu"] / total, delta=2e-3)
        self.assertAlmostEqual(li, moles["Li"] / total, delta=2e-3)
        self.assertAlmostEqual(pack["Li6"] / pack["Li7"], 9.0, places=9)
        self.assertIn("Fe56", pack)  # natural abundance for the element with no isotope list

    def test_atom_basis_nuclide(self):
        self.assertEqual(self.materials["atoms"]["nuclides"], {"Be9": 1.0})

    def test_an_unknown_element_is_an_error_not_a_crash(self):
        self.assertIsNone(self.materials["bad-element"]["nuclides"])
        self.assertIn("Xx", self.materials["bad-element"]["error"])

    def test_library_entries_are_audited(self):
        n = self.report["nuclides"]
        for name in ("W182", "Cu63", "Li6", "Be9", "Fe56"):
            self.assertTrue(n[name]["present"] and n[name]["readable"], name)
            self.assertIsNone(n[name]["error"], name)
            self.assertTrue(n[name]["temperatures_k"], name)
        self.assertFalse(n["Fe999"]["present"])
        self.assertIn("no entry", n["Fe999"]["error"])


if __name__ == "__main__":
    unittest.main()
