# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fuzz_findings as ff  # noqa: E402

REPO = Path(__file__).resolve().parent.parent


def finding(**changes):
    d = {"id": "FUZZ-001", "target": "facets", "found": "2026-10-01", "status": "fixed", "summary": "panic on 0 bytes",
         "fixed_in": "abc1234", "reproducer": "crates/x/t.rs::empty_input_is_an_error"}
    d.update(changes)
    return d


class FuzzFindingsTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "fuzz" / "fuzz_targets").mkdir(parents=True)
        (self.root / "fuzz" / "fuzz_targets" / "facets.rs").write_text("", encoding="utf-8")
        (self.root / "crates" / "x").mkdir(parents=True)
        (self.root / "crates" / "x" / "t.rs").write_text("fn empty_input_is_an_error() {}\n", encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def problems(self, *findings, release=False):
        return ff.lint({"findings": list(findings)}, self.root, release)

    # Verifies: SEC-022
    def test_the_real_log_passes_even_at_a_release(self):
        data = json.loads((REPO / ff.LOG).read_text(encoding="utf-8"))
        self.assertEqual(ff.lint(data, REPO, release=True), [])

    # Verifies: SEC-022
    def test_a_fixed_finding_needs_a_reproducer_that_exists(self):
        self.assertEqual(self.problems(finding()), [])
        for change in ({"reproducer": None}, {"reproducer": "crates/x/t.rs"}, {"reproducer": "crates/x/none.rs::a"},
                       {"reproducer": "crates/x/t.rs::another_name"}, {"fixed_in": ""}):
            self.assertTrue(self.problems(finding(**change)), change)

    # Verifies: SEC-022
    def test_an_open_finding_fails_only_at_a_release(self):
        open_one = finding(status="open", reproducer=None, fixed_in=None)
        self.assertEqual(self.problems(open_one), [])
        self.assertIn("open at a release", self.problems(open_one, release=True)[0])

    # Verifies: SEC-022
    def test_fields_and_artifacts_are_checked(self):
        for change in ({"id": "BUG-1"}, {"target": "nothing"}, {"found": "yesterday"}, {"summary": " "}, {"status": "closed"}):
            self.assertTrue(self.problems(finding(**change)), change)
        self.assertTrue(self.problems(finding(), finding()))
        artifacts = self.root / "fuzz" / "artifacts" / "facets"
        artifacts.mkdir(parents=True)
        (artifacts / "crash-1").write_bytes(b"x")
        self.assertIn("fuzz/artifacts", self.problems()[0])
        self.assertEqual(self.problems(finding()), [])


if __name__ == "__main__":
    unittest.main()
