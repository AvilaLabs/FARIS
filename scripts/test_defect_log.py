# SPDX-License-Identifier: AGPL-3.0-only
import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import defect_log as dl  # noqa: E402

REPO = Path(__file__).resolve().parent.parent


def entry(**changes):
    d = {"id": "FARIS-001", "title": "t", "severity": "S3", "status": "fixed", "found": "2026-10-01",
         "affected_versions": ["unreleased"], "fixed_in": "abc1234", "disposition": "Wrong; fixed.",
         "regression_test": "tests/t.rs::it_works"}
    d.update(changes)
    return d


class DefectLogTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "tests").mkdir()
        (self.root / "tests" / "t.rs").write_text("fn it_works() {}\n", encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def problems(self, *entries):
        return dl.lint({"defects": list(entries)}, self.root)

    # Verifies: QA-080
    def test_the_real_log_passes(self):
        data = json.loads((REPO / dl.LOG).read_text(encoding="utf-8"))
        self.assertEqual(dl.lint(data, REPO), [])
        self.assertTrue(data["defects"])

    # Verifies: QA-080
    def test_a_complete_entry_passes(self):
        self.assertEqual(self.problems(entry()), [])

    # Verifies: QA-080
    def test_each_required_field_is_checked(self):
        for field in ("title", "severity", "status", "found", "affected_versions", "disposition",
                      "regression_test", "fixed_in"):
            d = entry()
            del d[field]
            found = self.problems(d)
            self.assertTrue(found, field)
            self.assertTrue(all(p.startswith("FARIS-001:") for p in found), found)

    # Verifies: QA-080
    def test_values_are_checked(self):
        for change in ({"severity": "S5"}, {"status": "closed"}, {"found": "2026-13-01"}, {"id": "BUG-1"},
                       {"affected_versions": []}, {"affected_versions": ["v1"]}, {"disposition": " "}):
            self.assertTrue(self.problems(entry(**change)), change)

    # Verifies: QA-080
    # Verifies: QA-085
    def test_the_regression_test_must_exist(self):
        self.assertTrue(any("does not exist" in p for p in self.problems(entry(regression_test="tests/x.rs::a"))))
        self.assertTrue(any("does not occur" in p for p in self.problems(entry(regression_test="tests/t.rs::nope"))))
        self.assertTrue(any("path::test_name" in p for p in self.problems(entry(regression_test="tests/t.rs"))))
        self.assertTrue(any("empty" in p for p in self.problems(entry(regression_test=None))))

    # Verifies: QA-080
    def test_open_and_wont_fix_entries(self):
        self.assertEqual(self.problems(entry(status="open", fixed_in=None, regression_test=None)), [])
        self.assertEqual(self.problems(entry(status="wont-fix", fixed_in=None, regression_test="Out of scope.")), [])
        self.assertTrue(self.problems(entry(status="wont-fix", fixed_in=None, regression_test="")))
        self.assertTrue(self.problems(entry(status="fixed", waiver={"owner": "a"})))
        self.assertEqual(self.problems(entry(status="open", severity="S2", regression_test=None,
                                             waiver={"owner": "a", "reason": "b", "expiry": "2026-12-01"})), [])

    # Verifies: QA-080
    def test_duplicate_ids_and_a_missing_list_are_reported(self):
        self.assertTrue(any("twice" in p for p in self.problems(entry(), entry())))
        self.assertTrue(dl.lint({}, self.root))
        self.assertTrue(dl.lint({"defects": ["x"]}, self.root))

    # Verifies: QA-080
    def test_every_problem_is_listed_and_the_exit_status_follows(self):
        log = self.root / "log.json"
        bad = [entry(severity="S9"), entry(id="FARIS-002", disposition="")]
        log.write_text(json.dumps({"defects": bad}), encoding="utf-8")
        self.assertEqual(dl.main(["--root", str(self.root), "--log", str(log)]), 1)
        log.write_text(json.dumps({"defects": [entry()]}), encoding="utf-8")
        self.assertEqual(dl.main(["--root", str(self.root), "--log", str(log)]), 0)
        self.assertEqual(dl.main(["--root", str(self.root), "--log", str(self.root / "none.json")]), 1)

    def test_lint_does_not_change_its_input(self):
        d = entry()
        before = copy.deepcopy(d)
        self.problems(d)
        self.assertEqual(d, before)


if __name__ == "__main__":
    unittest.main()
