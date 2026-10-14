# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import checks as C  # noqa: E402
from validation import matrix as M  # noqa: E402
from validation.fixtures import clone, manifest, run_record  # noqa: E402


def case_dir(tmp: Path, m: dict) -> Path:
    d = tmp / m["case_id"]
    d.mkdir()
    (d / "manifest.json").write_text(json.dumps(m), encoding="utf-8")
    return d


def row(m: dict, **over) -> dict:
    r = {"quantity": "flux", "benchmark_id": f"{m['case_id']}/flux", "status": "not validated", "reason": "r",
         "not_covered": "; ".join(m["not_covered"]), "range_sha256": M.range_sha256(m["qualified_range"])}
    r.update(over)
    return r


class Checks(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self.rules = self.tmp / "rule_hashes.json"

    # Verifies: VAL-005
    def test_a_recorded_rule_passes_and_a_changed_rule_needs_a_new_version(self):
        m = manifest()
        d = case_dir(self.tmp, m)
        C.append_rule(d, self.rules, recorded="2026-10-09")
        self.assertEqual(C.check_rule_hashes([d], self.rules), [])
        changed = clone(m)
        changed["compatibility"]["k"] = 3.0
        (d / "manifest.json").write_text(json.dumps(changed), encoding="utf-8")
        self.assertTrue(any("rule changed" in msg for _, msg in C.check_rule_hashes([d], self.rules)))
        changed["compatibility"]["declared"] = "2026-10-10"
        (d / "manifest.json").write_text(json.dumps(changed), encoding="utf-8")
        C.append_rule(d, self.rules, recorded="2026-10-10")
        self.assertEqual(C.check_rule_hashes([d], self.rules), [])
        with self.assertRaises(SystemExit):
            C.append_rule(d, self.rules)  # unchanged: nothing to append

    # Verifies: VAL-005
    def test_a_record_dated_before_the_rule_and_a_case_without_an_entry_are_found(self):
        m = manifest()
        d = case_dir(self.tmp, m)
        self.assertTrue(any("missing" in msg for _, msg in C.check_rule_hashes([d], self.rules)))
        C.append_rule(d, self.rules)
        (d / "records").mkdir()
        (d / "records" / "x.run.json").write_text(json.dumps({"run_date": "2026-10-01"}), encoding="utf-8")
        self.assertTrue(any("not dated on or after" in msg for _, msg in C.check_rule_hashes([d], self.rules)))

    # Verifies: VAL-005
    def test_an_edited_entry_hash_is_found(self):
        m = manifest()
        d = case_dir(self.tmp, m)
        C.append_rule(d, self.rules)
        data = json.loads(self.rules.read_text())
        data["entries"][0]["rule_sha256"] = "0" * 64
        self.rules.write_text(json.dumps(data))
        self.assertTrue(C.check_rule_hashes([d], self.rules))

    # Verifies: VAL-070, VAL-075
    def test_range_hash_and_not_covered_are_checked_per_row(self):
        m = manifest()
        self.assertEqual(C.check_matrix_rows({"rows": [row(m)]}, {m["case_id"]: m}), [])
        found = C.check_matrix_rows({"rows": [row(m, range_sha256="0" * 64, not_covered="")]}, {m["case_id"]: m})
        self.assertEqual({r for r, _ in found}, {"VAL-070", "VAL-075"})

    # Verifies: VAL-041
    def test_code_to_code_rows_are_never_validated_and_name_shared_inputs(self):
        m = manifest()
        m["evidence_class"] = "code-to-code"
        bad = C.check_matrix_rows({"rows": [row(m, status="validated within range")]}, {m["case_id"]: m})
        self.assertIn("VAL-041", {r for r, _ in bad})
        m["not_covered"] = ["shares nuclear data, geometry and source with the reference code"]
        self.assertEqual(C.check_matrix_rows({"rows": [row(m)]}, {m["case_id"]: m}), [])

    # Verifies: QA-091
    def test_vendored_upstream_file_is_found(self):
        m = manifest()
        d = case_dir(self.tmp, m)
        self.assertEqual(C.check_provenance([d]), [])
        (d / "a.xml").write_text("x")
        self.assertTrue(any("stored in the repository" in msg for _, msg in C.check_provenance([d])))

    # Verifies: VAL-093
    def test_cross_thread_records_agree_within_two_sigma(self):
        m = manifest()
        a = run_record(m, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        b = clone(a)
        a["context"], b["context"] = {"threads": 1}, {"threads": 4}
        self.assertEqual(C.cross_thread_check(a, b), [])
        b["results"][2]["value"] = 3.5  # u_mc is 0.001: far outside 2 sigma
        self.assertTrue(any("d2" in p for p in C.cross_thread_check(a, b)))
        b["context"]["threads"] = 1
        self.assertTrue(any("different counts" in p for p in C.cross_thread_check(a, b)))

    # Verifies: VAL-005, VAL-041, VAL-070, VAL-075, QA-091
    def test_the_tree_passes_its_own_checks(self):
        self.assertEqual(C.run_all(C.ROOT), 0)


if __name__ == "__main__":
    unittest.main()
