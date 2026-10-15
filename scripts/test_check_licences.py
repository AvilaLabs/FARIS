# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for the licence policy check (LEG-003, SEC-039)."""
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_licences as cl  # noqa: E402

POLICY = {"allowed": ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "Unicode-3.0"], "exceptions": []}


def pkg(name, licence, version="1.0.0"):
    return {"name": name, "version": version, "license": licence, "license_file": None}


class LicenceTests(unittest.TestCase):
    def problems(self, licence, policy=POLICY):
        return cl.check([pkg("x", licence)], policy)[0]

    def test_or_needs_one_allowed_branch_and_needs_all_parts(self):
        # Verifies: LEG-003
        self.assertEqual(self.problems("MIT OR Apache-2.0"), [])
        self.assertEqual(self.problems("Apache-2.0 OR GPL-2.0-only"), [])
        self.assertEqual(self.problems("MIT AND Unicode-3.0"), [])
        self.assertEqual(self.problems("(MIT OR Apache-2.0) AND Unicode-3.0"), [])
        self.assertTrue(self.problems("GPL-2.0-only"))
        self.assertTrue(self.problems("MIT AND GPL-2.0-only"))
        self.assertTrue(self.problems("GPL-2.0-only OR LGPL-2.1-only"))

    def test_older_slash_separators_and_exceptions_with_with(self):
        # Verifies: LEG-003
        self.assertEqual(self.problems("MIT/Apache-2.0"), [])
        self.assertEqual(self.problems("MIT / Apache-2.0"), [])
        self.assertEqual(self.problems("Apache-2.0 WITH LLVM-exception"), [])
        self.assertTrue(self.problems("Apache-2.0 WITH Other-exception"))

    def test_missing_and_unparseable_licences_fail(self):
        # Verifies: LEG-003
        self.assertIn("no licence declared", self.problems(None)[0])
        self.assertIn("cannot parse", self.problems("MIT OR")[0])
        self.assertIn("cannot parse", self.problems("(MIT")[0])

    def test_an_exception_matches_the_exact_expression_and_unused_ones_fail(self):
        # Verifies: LEG-003
        policy = {**POLICY, "exceptions": [{"package": "x", "licence": "OFL-1.1", "reason": "font data"}]}
        problems, used = cl.check([pkg("x", "OFL-1.1")], policy)
        self.assertEqual((problems, used), ([], 1))
        self.assertTrue(cl.check([pkg("x", "OFL-1.1 OR GPL-2.0-only")], policy)[0])  # changed declaration
        self.assertTrue(any("matches no package" in p for p in cl.check([pkg("y", "MIT")], policy)[0]))
        with self.assertRaises(ValueError):
            cl.check([], {**POLICY, "exceptions": [{"package": "x", "licence": "A", "reason": " "}]})

    def test_the_repository_policy_is_well_formed_and_fails_a_gpl2_only_dependency(self):
        # Verifies: SEC-039
        policy = json.loads(cl.POLICY.read_text(encoding="utf-8"))
        self.assertNotIn("GPL-2.0-only", policy["allowed"])
        self.assertNotIn("GPL-2.0-or-later", policy["allowed"])
        self.assertIn("AGPL-3.0-only", policy["allowed"])
        self.assertTrue(all(e["reason"].strip() for e in policy["exceptions"]))
        policy = {**policy, "exceptions": []}
        self.assertTrue(cl.check([pkg("bad", "GPL-2.0-only")], policy)[0])
        self.assertEqual(cl.check([pkg("ok", "MIT OR Apache-2.0")], policy)[0], [])


if __name__ == "__main__":
    unittest.main()
