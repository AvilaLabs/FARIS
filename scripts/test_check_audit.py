# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for the advisory check with expiring waivers (SEC-031)."""
import json
import sys
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_audit as ca  # noqa: E402

TODAY = date(2026, 10, 15)


def report(vulns=(), unmaintained=(), yanked=()):
    adv = lambda i: {"id": i, "title": f"title {i}"}  # noqa: E731
    pkg = lambda n: {"name": n, "version": "1.0.0"}  # noqa: E731
    warnings = {}
    if unmaintained:
        warnings["unmaintained"] = [{"kind": "unmaintained", "package": pkg(n), "advisory": adv(i)} for i, n in unmaintained]
    if yanked:
        warnings["yanked"] = [{"kind": "yanked", "package": pkg(n), "advisory": None} for n in yanked]
    return {"vulnerabilities": {"found": bool(vulns), "list": [{"advisory": adv(i), "package": pkg(n)} for i, n in vulns]},
            "warnings": warnings}


def waiver(ident, added="2026-10-01", expiry="2026-12-01", **over):
    return {"id": ident, "package": "p", "owner": "o", "reason": "r", "added": added, "expiry": expiry, **over}


class AuditTests(unittest.TestCase):
    def test_a_clean_report_passes_and_any_unwaived_finding_fails(self):
        # Verifies: SEC-031
        self.assertEqual(ca.check(report(), [], TODAY), [])
        self.assertTrue(ca.check(report(vulns=[("RUSTSEC-1", "a")]), [], TODAY))
        self.assertTrue(ca.check(report(unmaintained=[("RUSTSEC-2", "b")]), [], TODAY))
        self.assertTrue(ca.check(report(yanked=["c"]), [], TODAY))

    def test_a_valid_waiver_covers_unmaintained_and_yanked_findings(self):
        # Verifies: SEC-031
        waivers = [waiver("RUSTSEC-2"), waiver("yanked:c-1.0.0")]
        self.assertEqual(ca.check(report(unmaintained=[("RUSTSEC-2", "b")], yanked=["c"]), waivers, TODAY), [])

    def test_expired_incomplete_overlong_and_unused_waivers_fail(self):
        # Verifies: SEC-031
        r = report(unmaintained=[("RUSTSEC-2", "b")])
        self.assertTrue(any("expired" in p for p in ca.check(r, [waiver("RUSTSEC-2", expiry="2026-10-01")], TODAY)))
        self.assertTrue(any("lacks owner" in p for p in ca.check(r, [waiver("RUSTSEC-2", owner="")], TODAY)))
        self.assertTrue(any("more than 90 days" in p for p in ca.check(r, [waiver("RUSTSEC-2", expiry="2027-06-01")], TODAY)))
        self.assertTrue(any("matches no finding" in p for p in ca.check(report(), [waiver("RUSTSEC-9")], TODAY)))

    def test_the_repository_allowlist_is_complete_and_within_the_ninety_day_rule(self):
        # Verifies: SEC-031
        waivers = json.loads(ca.ALLOWLIST.read_text(encoding="utf-8"))["waivers"]
        self.assertTrue(waivers)
        self.assertEqual(ca.waiver_problems(waivers, date(2026, 10, 15)), [])


if __name__ == "__main__":
    unittest.main()
