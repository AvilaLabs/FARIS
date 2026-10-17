# SPDX-License-Identifier: AGPL-3.0-only
import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import a11y_regression as A  # noqa: E402

MARK = "// Verif" + "ies: "  # kept apart so the trace scan does not read this file's fixtures as markers


def tests(**verdicts):
    return {"commit": "c" * 40, "checks": [{"name": f"faris-app::{n}", "verdict": v} for n, v in verdicts.items()]}


class A11yRegression(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.d = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        src = self.d / "crates" / "faris-app" / "src"
        src.mkdir(parents=True)
        (src / "a.rs").write_text(f"{MARK}A11Y-001, A11Y-002\nfn focus_walk() {{}}\n\n{MARK}A11Y-002\nfn contrast() {{}}\n\n"
                                  f"{MARK}QA-001\nfn unrelated() {{}}\n\n{MARK}A11Y-099\nfn out_of_range() {{}}\n", encoding="utf-8")

    # Verifies: A11Y-055
    def test_a_snapshot_derives_one_verdict_per_id_from_the_marked_tests(self):
        snap = A.snapshot(tests(focus_walk="PASS", contrast="FAIL"), self.d)
        self.assertEqual(snap["commit"], "c" * 40)
        self.assertEqual(len(snap["results"]), 46)
        self.assertEqual((snap["results"]["A11Y-001"], snap["results"]["A11Y-002"], snap["results"]["A11Y-003"]),
                         ("PASS", "FAIL", "NOT_EVALUATED"))
        self.assertNotIn("A11Y-099", snap["results"])
        with self.assertRaises(ValueError):
            A.snapshot({"commit": "c", "checks": []}, self.d)

    # Verifies: A11Y-055
    def test_a_check_that_passed_before_and_fails_or_goes_unrun_now_is_a_new_failure(self):
        before = {"results": {"A11Y-001": "PASS", "A11Y-002": "PASS", "A11Y-003": "FAIL", "A11Y-004": "NOT_EVALUATED"}}
        now = {"results": {"A11Y-001": "PASS", "A11Y-002": "FAIL", "A11Y-003": "FAIL", "A11Y-004": "FAIL", "A11Y-005": "FAIL"}}
        new, known = A.compare(before, now)
        self.assertEqual(new, ["A11Y-002: PASS -> FAIL", "A11Y-004: NOT_EVALUATED -> FAIL", "A11Y-005: NOT_EVALUATED -> FAIL"])
        self.assertIn("A11Y-003: FAIL", known)
        gone, _ = A.compare(before, {"results": {"A11Y-001": "PASS"}})
        self.assertEqual(gone, ["A11Y-002: PASS -> NOT_EVALUATED"])

    # Verifies: A11Y-055
    def test_no_change_and_improvement_are_not_failures(self):
        same = {"results": {"A11Y-001": "PASS", "A11Y-002": "FAIL"}}
        self.assertEqual(A.compare(same, same)[0], [])
        better = {"results": {"A11Y-001": "PASS", "A11Y-002": "PASS"}}
        self.assertEqual(A.compare(same, better)[0], [])

    # Verifies: A11Y-055
    def test_the_command_line_exits_1_on_a_new_failure(self):
        evidence = self.d / "tests.json"
        evidence.write_text(json.dumps(tests(focus_walk="PASS", contrast="PASS")), encoding="utf-8")
        first, second = self.d / "one.json", self.d / "two.json"
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(A.main(["snapshot", "--tests", str(evidence), "--out", str(first), "--root", str(self.d)]), 0)
            evidence.write_text(json.dumps(tests(focus_walk="PASS", contrast="FAIL")), encoding="utf-8")
            self.assertEqual(A.main(["snapshot", "--tests", str(evidence), "--out", str(second), "--root", str(self.d)]), 0)
            self.assertEqual(A.main(["compare", "--baseline", str(first), "--current", str(first)]), 0)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(A.main(["compare", "--baseline", str(first), "--current", str(second)]), 1)
        self.assertIn("NEW FAILURE A11Y-002: PASS -> FAIL", out.getvalue())


if __name__ == "__main__":
    unittest.main()
