# SPDX-License-Identifier: AGPL-3.0-only
"""The materials database lint passes on the shipped files and catches each kind of damage (MAT-009)."""
import importlib.util
import json
import shutil
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
SPEC = importlib.util.spec_from_file_location("materials_db_lint", HERE / "materials_db_lint.py")
LINT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LINT)

DATA = Path("crates") / "faris-model" / "data"


class MaterialsDatabaseLintTests(unittest.TestCase):
    def copy_data(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        shutil.copytree(ROOT / DATA, root / DATA)
        return root

    def edit(self, root, name, change):
        path = root / DATA / name
        text = path.read_text(encoding="utf-8")
        path.write_text(change(text), encoding="utf-8")

    def test_the_shipped_files_pass(self):
        # Verifies: MAT-009
        self.assertEqual(LINT.lint(ROOT), [])

    def test_a_stored_value_that_differs_from_its_quote_is_found(self):
        # Verifies: MAT-009, MAT-050
        root = self.copy_data()
        self.edit(root, "materials/materials_db.json", lambda t: t.replace('"max_wt_ppm": 10,', '"max_wt_ppm": 11,', 1))
        self.assertTrue(any("differs from the written value" in p for p in LINT.lint(root)))

    def test_a_missing_citation_and_an_unknown_source_are_found(self):
        # Verifies: MAT-009
        root = self.copy_data()
        self.edit(root, "materials/materials_db.json", lambda t: t.replace('"source": "wang-1996-jnm"', '"source": "nobody-1999"', 1))
        self.assertTrue(any("unknown source nobody-1999" in p for p in LINT.lint(root)))

    def test_a_duplicate_limit_id_is_found(self):
        # Verifies: MAT-009
        root = self.copy_data()
        self.edit(root, "materials/materials_db.json", lambda t: t.replace('"id": "tungsten-limits"', '"id": "cucrzr-dpa-divertor"', 1))
        self.assertTrue(any("appears twice" in p for p in LINT.lint(root)))

    def test_a_label_outside_the_five_is_found(self):
        # Verifies: MAT-007, MAT-009
        root = self.copy_data()
        self.edit(root, "materials/materials_db.json", lambda t: t.replace('"label": "conditional",\n     "value"', '"label": "probably",\n     "value"', 1))
        self.assertTrue(any("probably" in p for p in LINT.lint(root)))

    def test_a_changed_waste_table_value_is_found_against_the_snapshot(self):
        # Verifies: ACT-030, ACT-031, ACT-032
        root = self.copy_data()
        self.edit(root, "waste/nrc_10cfr61_55.json", lambda t: t.replace('"limit": 0.2}', '"limit": 0.3}', 1))
        self.assertTrue(any("Nb-94 in activated metal" in p for p in LINT.lint(root)))

    def test_an_edited_snapshot_is_found_by_its_hash(self):
        # Verifies: ACT-030
        root = self.copy_data()
        self.edit(root, "waste/ecfr_10cfr61.55.xml", lambda t: t.replace("Waste classification", "Waste classifications", 1))
        self.assertTrue(any("SHA-256" in p for p in LINT.lint(root)))

    def test_a_duplicate_clearance_nuclide_is_found(self):
        # Verifies: ACT-036
        root = self.copy_data()
        path = root / DATA / "waste" / "iaea_gsr3_table_i2.json"
        doc = json.loads(path.read_text(encoding="utf-8"))
        doc["levels"].append(dict(doc["levels"][0]))
        path.write_text(json.dumps(doc), encoding="utf-8")
        self.assertTrue(any("appears twice" in p for p in LINT.lint(root)))


if __name__ == "__main__":
    unittest.main()
