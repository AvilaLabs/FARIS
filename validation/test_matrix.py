# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import matrix as M  # noqa: E402
from validation import report as R  # noqa: E402
from validation.fixtures import IDENT, clone, manifest, run_record  # noqa: E402
from validation.manifest import load_manifest  # noqa: E402

REGISTRY = {"schema": M.REGISTRY_SCHEMA, "quantities": [
    {"id": "flux", "label": "flux", "kind": "calculated", "benchmarks": [{"case_id": "fixture", "response_class": "flux", "scope": "fixture flux"}]},
    {"id": "tbr", "label": "tbr", "kind": "calculated", "benchmarks": []}]}


class Matrix(unittest.TestCase):
    def suite(self, with_run=False):
        m = manifest()
        records = {"fixture": run_record(m, [d["reference"]["value"] * 1.01 for d in m["detectors"]])} if with_run else {}
        return R.build_suite([m], records, IDENT if with_run else None, registry=REGISTRY)

    # Verifies: VAL-050
    def test_every_calculated_quantity_has_a_row(self):
        matrix = self.suite()["matrix"]
        self.assertEqual({r["quantity"] for r in matrix["rows"]}, {"flux", "tbr"})
        self.assertEqual(M.check_matrix(matrix, REGISTRY), [])

    # Verifies: VAL-051
    def test_a_quantity_with_no_benchmark_is_labelled_not_validated_with_a_reason(self):
        row = next(r for r in self.suite()["matrix"]["rows"] if r["quantity"] == "tbr")
        self.assertEqual((row["benchmark_id"], row["status"]), ("none", "not validated"))
        self.assertTrue(row["reason"] and row["next_step"])

    # Verifies: VAL-051
    def test_blank_cells_are_found(self):
        matrix = self.suite()["matrix"]
        for blank in ("", "   ", None):
            bad = clone(matrix)
            bad["rows"][0]["last_run"] = blank
            self.assertTrue(any("blank cell last_run" in p for p in M.check_matrix(bad, REGISTRY)), repr(blank))
        bad = clone(matrix)
        del bad["rows"][1]["reason"]
        self.assertTrue(any("blank cell reason" in p for p in M.check_matrix(bad)))

    # Verifies: VAL-051
    def test_no_benchmark_row_that_claims_validation_is_refused(self):
        bad = clone(self.suite()["matrix"])
        next(r for r in bad["rows"] if r["benchmark_id"] == "none")["status"] = "validated within range"
        self.assertTrue(any("must be labelled 'not validated'" in p for p in M.check_matrix(bad)))

    # Verifies: VAL-051
    def test_a_quantity_missing_from_the_matrix_or_the_registry_diff_is_found(self):
        bad = clone(self.suite()["matrix"])
        bad["rows"] = [r for r in bad["rows"] if r["quantity"] != "tbr"]
        self.assertIn("quantity tbr has no matrix row", M.check_matrix(bad))
        bigger = clone(REGISTRY)
        bigger["quantities"].append({"id": "dpa", "label": "dpa", "kind": "calculated", "benchmarks": []})
        self.assertIn("matrix quantities differ from the quantity registry", M.check_matrix(self.suite()["matrix"], bigger))
        self.assertEqual(M.check_matrix({"schema": "x"}), ["matrix schema must be 'faris.validation-matrix/1.0.0'"])

    # Verifies: VAL-052
    def test_a_scored_benchmark_carries_bias_and_a_run_status(self):
        row = next(r for r in self.suite(with_run=True)["matrix"]["rows"] if r["quantity"] == "flux")
        self.assertEqual(row["status"], "validated within range")
        self.assertIn("mean log C/E", row["bias"])
        self.assertEqual(row["last_run"], "run date not recorded in the run record")

    def test_a_benchmark_that_is_not_a_registered_response_is_an_error(self):
        bad = clone(REGISTRY)
        bad["quantities"][0]["benchmarks"][0]["response_class"] = "nothing"
        with self.assertRaises(Exception):
            R.build_suite([manifest()], {}, None, registry=bad)

    # Verifies: VAL-006
    def test_the_committed_matrix_is_what_the_registry_and_manifests_generate(self):
        registry = M.load_registry()
        manifests = [load_manifest(p) for p in R.discover()]
        generated = R.build_suite(manifests, {}, None, registry=registry)["matrix"]
        self.assertEqual(json.loads(M.MATRIX.read_text(encoding="utf-8")), generated, "regenerate with: python3 -m validation matrix")
        self.assertEqual(M.check_matrix(generated, registry), [])
        self.assertEqual({r["status"] for r in generated["rows"]}, {"not validated"})  # no FARIS run has been scored

    def test_registered_quantities_are_the_twelve_of_val_050(self):
        ids = {q["id"] for q in M.load_registry()["quantities"]}
        self.assertEqual(len(ids), 12)
        self.assertLessEqual({"tbr", "heating", "flux", "dpa", "decay_heat", "net_electric_power"}, ids)

    def test_matrix_command_writes_the_file(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "matrix.json"
            self.assertEqual(R.main(["matrix", "--out", str(out)]), 0)
            self.assertEqual(json.loads(out.read_text(encoding="utf-8")), json.loads(M.MATRIX.read_text(encoding="utf-8")))


if __name__ == "__main__":
    unittest.main()
