# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import advisory_log as al  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
TODAY = date(2026, 10, 17)


def advisory(**changes):
    d = {"id": "RUSTSEC-2026-0001", "package": "ureq", "severity": "critical", "published": "2026-10-01",
         "status": "fixed", "fixed_on": "2026-10-07", "fixed_in": "0.3.0"}
    d.update(changes)
    return d


class AdvisoryLogTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "Cargo.lock").write_text('[[package]]\nname = "ureq"\nversion = "2.12.1"\n', encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def problems(self, *advisories, packages=("ureq",)):
        return al.lint({"packages": list(packages), "advisories": list(advisories)}, self.root, TODAY)

    # Verifies: SEC-039
    def test_the_real_log_passes_and_names_the_signin_stack(self):
        data = json.loads((REPO / al.LOG).read_text(encoding="utf-8"))
        self.assertEqual(al.lint(data, REPO, TODAY), [])
        self.assertTrue({"avila-account", "ureq", "rustls", "ring"} <= set(data["packages"]))

    # Verifies: SEC-039
    def test_a_critical_fix_is_in_time_at_7_days_and_late_at_8(self):
        self.assertEqual(self.problems(advisory()), [])
        self.assertEqual(self.problems(advisory(fixed_on="2026-10-08")), [])
        late = self.problems(advisory(fixed_on="2026-10-09"))
        self.assertIn("allows 7", late[0])

    # Verifies: SEC-039
    def test_a_high_fix_is_in_time_at_30_days_and_late_at_31(self):
        self.assertEqual(self.problems(advisory(severity="high", published="2026-09-01", fixed_on="2026-10-01")), [])
        self.assertIn("allows 30", self.problems(advisory(severity="high", published="2026-09-01", fixed_on="2026-10-02"))[0])

    # Verifies: SEC-039
    def test_an_open_advisory_is_overdue_after_its_limit(self):
        open_one = {"status": "open", "fixed_on": None, "fixed_in": None}
        self.assertEqual(self.problems(advisory(published="2026-10-10", **open_one)), [])
        self.assertIn("open 16 days", self.problems(advisory(published="2026-10-01", **open_one))[0])
        self.assertEqual(self.problems(advisory(severity="medium", published="2026-01-01", **open_one)), [])

    # Verifies: SEC-039
    def test_fields_and_the_package_list_are_checked(self):
        for change in ({"package": "serde"}, {"severity": "urgent"}, {"published": "soon"}, {"status": "closed"},
                       {"fixed_on": None}, {"fixed_on": "2026-09-01"}):
            self.assertTrue(self.problems(advisory(**change)), change)
        self.assertTrue(self.problems(advisory(), advisory()))
        self.assertTrue(self.problems(packages=("ureq", "not-in-the-lock-file")))
        self.assertTrue(self.problems(packages=()))


if __name__ == "__main__":
    unittest.main()
