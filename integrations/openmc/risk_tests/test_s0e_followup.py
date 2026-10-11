"""Tests for the S0e follow-up rules (s0e_followup, Amendment 2). Standard library only.

Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import json
import tempfile
import unittest
from pathlib import Path

import s0e_followup as F
import s0e_run as R


def row(gain, sigma, unbiased=True):
    return {"gain": gain, "sigma": sigma, "unbiased": unbiased}


def s0e(h5_outcome):
    return {"rows": {"E0": row(20.0, 1.5), "E1": row(30.0, 2.0)}, "rule": {"H5": {"outcome": h5_outcome}}}


class Summary(unittest.TestCase):
    def test_h6_prime_is_judged_against_s0e_e0(self):
        out = F.summarise(s0e("H5 confirmed"), {"E2′": row(40.0, 3.0)}, R.Budget(cap_s=F.CAP_S), {})
        self.assertEqual(out["rule"]["H6′"]["outcome"], "H6 confirmed")
        self.assertTrue(out["rule"]["E3′_required"])
        self.assertEqual(out["rule"]["best"], "E2′")

    def test_e3_prime_needs_h5_too(self):
        out = F.summarise(s0e("H5 rejected"), {"E2′": row(40.0, 3.0)}, R.Budget(cap_s=F.CAP_S), {})
        self.assertFalse(out["rule"]["E3′_required"])

    def test_a_stalled_e2_prime_leaves_e0_or_e1_as_best(self):
        out = F.summarise(s0e("H5 confirmed"), {"E2′": row(None, None, None)}, R.Budget(cap_s=F.CAP_S), {})
        self.assertEqual(out["rule"]["H6′"]["outcome"], "NOT_EVALUATED")
        self.assertEqual(out["rule"]["best"], "E1")
        self.assertEqual(out["rule"]["g_star"], 30.0)


class Depth(unittest.TestCase):
    def test_reads_the_generation_record(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "gen.json"
            p.write_text(json.dumps({"windows": {"lower_min_positive": 1e-9, "lower_max": 0.5,
                                                 "fraction_above_1e-9_of_max": 0.2, "fraction_positive": 0.22}}))
            self.assertEqual(F.window_depth(p)["lower_min_positive"], 1e-9)
            self.assertEqual(F.window_depth(Path(d) / "missing.json"), {})


if __name__ == "__main__":
    unittest.main()
