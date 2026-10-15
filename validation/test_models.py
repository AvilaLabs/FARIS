# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import matrix as M  # noqa: E402
from validation import models as MD  # noqa: E402
from validation import report as R  # noqa: E402
from validation.fixtures import FIXTURE_REGISTRY, IDENT, RUST_CATALOGUE, clone, manifest, run_record, write_tree  # noqa: E402


class Tree(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self.suite = write_tree(self.root)

    def models(self):
        return json.loads((self.root / "validation" / "models.json").read_text(encoding="utf-8"))

    def save_models(self, data):
        (self.root / "validation" / "models.json").write_text(json.dumps(data), encoding="utf-8")

    def committed(self):
        v = self.root / "validation"
        return (json.loads((v / "model_matrix.json").read_text(encoding="utf-8")), json.loads((v / "matrix.json").read_text(encoding="utf-8")))


class ModelLint(Tree):
    # Verifies: VAL-001
    def test_a_complete_tree_has_no_findings_and_the_model_row_is_generated_from_the_catalogue(self):
        self.assertEqual(MD.lint_models(self.root), [])
        row = self.suite["model_matrix"]["rows"][0]
        self.assertEqual(row["quantities"], ["flux", "tbr"])
        self.assertEqual([o["name"] for o in row["reported_outputs"]], ["flux_n_m2_s", "breeding", "histories"])
        self.assertEqual(row["status"], "not validated")
        self.assertEqual(MD.check_model_matrix(self.suite["model_matrix"]), [])

    # Verifies: VAL-001
    def test_the_real_tree_has_a_row_for_every_reported_model_and_output(self):
        self.assertEqual(MD.lint_models(MD.ROOT), [])
        catalogues = MD.read_catalogues(MD.ROOT)
        self.assertGreaterEqual(len(catalogues), 4)
        self.assertEqual(MD.check_model_matrix(json.loads(MD.MODEL_MATRIX.read_text(encoding="utf-8"))), [])

    # Verifies: VAL-001
    def test_a_catalogue_no_model_claims_and_a_quantity_missing_from_the_registry_are_found(self):
        (self.root / "crates" / "fx" / "src" / "more.rs").write_text(
            'pub const OTHER_OUTPUTS: &[Output] = &[\n    calculated("dose", Quantity::ShutdownDoseRate),\n];\n', encoding="utf-8")
        registry = clone(FIXTURE_REGISTRY)
        problems = MD.lint_models(self.root, registry=registry)
        self.assertTrue(any("OTHER_OUTPUTS" in p and "no model row claims it" in p for p in problems), problems)
        self.assertTrue(any("shutdown_dose_rate" in p and "not in the quantity registry" in p for p in problems), problems)

    # Verifies: VAL-001
    def test_evidence_that_does_not_exist_or_has_no_single_class_is_found(self):
        data = self.models()
        ev = data["models"][0]["verification"]
        ev.append({"class": "verification", "kind": "test", "file": "crates/fx/src/lib.rs", "name": "no_such_test", "what": "x"})
        ev.append({"class": "validation", "kind": "test", "file": "crates/fx/src/lib.rs", "what": "x"})
        ev.append({"class": "verification", "kind": "control", "file": "controls/missing.py", "what": "x"})
        self.save_models(data)
        text = "\n".join(MD.lint_models(self.root))
        self.assertIn("no_such_test is not defined", text)
        self.assertIn("class 'validation' is not exactly one of", text)
        self.assertIn("controls/missing.py does not exist", text)

    # Verifies: VAL-001
    def test_a_model_with_no_catalogue_must_say_what_it_reports(self):
        data = self.models()
        data["models"].append({"id": "bare", "label": "bare", "page": "docs/fixture.md", "catalogues": [], "quantities": [], "sources": ["docs/fixture.md"],
                               "verification": data["models"][0]["verification"], "not_covered": ["x"]})
        self.save_models(data)
        self.assertTrue(any("does not say what it reports" in p for p in MD.lint_models(self.root)))

    # Verifies: VAL-001
    def test_an_unlabelled_or_two_label_matrix_record_is_rejected(self):
        matrix = clone(self.suite["matrix"])
        row = next(r for r in matrix["rows"] if r["benchmark_id"] != "none")
        for bad in ("", "experiment or verification", None):
            row["evidence_class"] = bad
            self.assertTrue(any("not exactly one of" in p or "blank cell evidence_class" in p for p in M.check_matrix(matrix, FIXTURE_REGISTRY)), repr(bad))
        row["evidence_class"] = "plant-reproduction"
        self.assertEqual(M.check_matrix(matrix, FIXTURE_REGISTRY), [])
        none_row = next(r for r in matrix["rows"] if r["benchmark_id"] == "none")
        none_row["evidence_class"] = "experiment"
        self.assertTrue(any("class must be 'none'" in p for p in M.check_matrix(matrix, FIXTURE_REGISTRY)))
        mm = clone(self.suite["model_matrix"])
        mm["rows"][0]["verification"][0]["class"] = ""
        self.assertTrue(any("no single valid evidence class" in p for p in MD.check_model_matrix(mm)))
        mm["rows"][0]["not_covered"] = " "
        self.assertTrue(any("blank cell not_covered" in p for p in MD.check_model_matrix(mm)))


class Staleness(Tree):
    # Verifies: VAL-037
    def test_a_tree_equal_to_its_generation_is_current(self):
        self.assertEqual(MD.staleness(self.root, *self.committed()), [])

    # Verifies: VAL-037
    def test_changing_a_model_source_marks_the_model_stale(self):
        src = self.root / "crates" / "fx" / "src" / "lib.rs"
        src.write_text(RUST_CATALOGUE + "// changed kernel\n", encoding="utf-8")
        stale = MD.staleness(self.root, *self.committed())
        self.assertEqual(len(stale), 1, stale)
        self.assertIn("STALE", stale[0])
        self.assertIn("crates/fx/src/lib.rs", stale[0])

    # Verifies: VAL-037
    def test_changing_a_case_the_registry_or_the_rules_marks_the_matrices_stale(self):
        case = self.root / "validation" / "cases" / "fixture" / "manifest.json"
        m = manifest()
        m["qualified_range"]["materials"] = ["Fe"]
        case.write_text(json.dumps(m), encoding="utf-8")
        stale = "\n".join(MD.staleness(self.root, *self.committed()))
        self.assertIn("STALE matrix.json", stale)
        self.assertIn("case fixture changed", stale)
        # A new reported quantity: the registry hash and the matrix both move.
        case.write_text(json.dumps(manifest()), encoding="utf-8")
        registry = clone(FIXTURE_REGISTRY)
        registry["quantities"].append({"id": "dpa", "label": "dpa", "kind": "calculated", "benchmarks": []})
        (self.root / "validation" / "quantities.json").write_text(json.dumps(registry), encoding="utf-8")
        stale = "\n".join(MD.staleness(self.root, *self.committed()))
        self.assertIn("row dpa/none is new in the tree", stale)
        self.assertIn("quantities.json changed", stale)

    # Verifies: VAL-037
    def test_a_model_added_to_the_models_file_has_no_committed_row(self):
        data = self.models()
        second = clone(data["models"][0])
        second["id"] = "second"
        data["models"].append(second)
        second["catalogues"] = []
        second["reports"] = "something"
        self.save_models(data)
        self.assertTrue(any("model second has no committed row" in p for p in MD.staleness(self.root, *self.committed())))


class Claims(unittest.TestCase):
    def suite(self, calc_factor=1.01):
        m = manifest()
        records = {"fixture": run_record(m, [d["reference"]["value"] * calc_factor for d in m["detectors"]])}
        return R.build_suite([m], records, IDENT, registry=FIXTURE_REGISTRY)

    # Verifies: VAL-006
    def test_a_matrix_derived_from_scored_cases_has_no_findings(self):
        suite = self.suite()
        flux = next(r for r in suite["matrix"]["rows"] if r["quantity"] == "flux")
        self.assertEqual(flux["status"], "validated within range")
        self.assertEqual(M.check_claims(suite["matrix"], suite["cases"], FIXTURE_REGISTRY), [])

    # Verifies: VAL-006
    def test_a_hand_edited_cell_fails_the_check(self):
        suite = self.suite(calc_factor=3.0)
        matrix = clone(suite["matrix"])
        row = next(r for r in matrix["rows"] if r["quantity"] == "flux")
        self.assertEqual(row["status"], "validated but out of range")
        row["status"] = "validated within range"
        problems = M.check_claims(matrix, suite["cases"], FIXTURE_REGISTRY)
        self.assertTrue(any("cell status differs from the value derived" in p for p in problems), problems)

    # Verifies: VAL-006
    def test_a_validated_row_without_a_passing_case_record_fails_the_check(self):
        m = manifest()
        no_run = R.build_suite([m], {}, None, registry=FIXTURE_REGISTRY)
        matrix = clone(no_run["matrix"])
        row = next(r for r in matrix["rows"] if r["quantity"] == "flux")
        row["status"] = "validated within range"
        problems = M.check_claims(matrix, no_run["cases"], FIXTURE_REGISTRY)
        self.assertTrue(any("without a case record with a PASS or FAIL verdict" in p for p in problems), problems)

    # Verifies: VAL-006
    def test_edited_verdict_counts_or_a_verdict_without_a_run_are_found(self):
        suite = self.suite()
        cases = clone(suite["cases"])
        cases[0]["classes"]["flux"]["verdict_counts"]["FAIL"] = 0
        cases[0]["classes"]["flux"]["verdict_counts"]["PASS"] += 1
        self.assertTrue(any("verdict counts do not equal" in p for p in M.check_claims(suite["matrix"], cases, FIXTURE_REGISTRY)))
        cases = clone(suite["cases"])
        cases[0]["run"] = None
        self.assertTrue(any("without a run record" in p for p in M.check_claims(suite["matrix"], cases, FIXTURE_REGISTRY)))


if __name__ == "__main__":
    unittest.main()
