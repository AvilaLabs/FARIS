"""Tests for the S0d rules (s0d_common). Standard library only; no OpenMC or numpy.

Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import math
import unittest

import r1_configs as R1
import s0b_common as C
import s0c_common as S0C
import s0d_common as S


def row(gain, sigma, unbiased=True):
    return {"gain": gain, "sigma": sigma, "unbiased": unbiased}


class Seeds(unittest.TestCase):
    def test_seeds_are_disjoint_from_earlier_stages(self):
        s0d = set(S.SEEDS["gen"].values()) | set(S.SEEDS["prod"].values())
        s0b = set(R1.S0B_GENERATION_SEEDS.values()) | set(R1.S0B_PRODUCTION_SEEDS.values()) | {R1.S0B_MGXS_SEED}
        s0c = {v for v in S0C.SEEDS.values() if isinstance(v, int)}
        for v in S0C.SEEDS.values():
            if isinstance(v, dict):
                s0c |= set(v.values())
        self.assertFalse(s0d & s0b)
        self.assertFalse(s0d & s0c)
        self.assertEqual(len(s0d), len(S.SEEDS["gen"]) + len(S.SEEDS["prod"]))


class Midpoints(unittest.TestCase):
    def test_one_line_inside_each_response_group(self):
        edges = R1.fast_response_edges("CASMO-8+0.1MeV")
        lines = S.group_midpoints(edges)
        self.assertEqual(len(lines), len(edges) - 1)
        for (a, b), e in zip(zip(edges, edges[1:]), lines):
            self.assertTrue(a < e < b)

    def test_rejects_bad_edges(self):
        for bad in ([1.0], [2.0, 1.0], [0.0, 1.0]):
            with self.assertRaises(ValueError):
                S.group_midpoints(bad)


class H4(unittest.TestCase):
    def test_confirmed_when_t1_doubles_and_t3_does_not_rise(self):
        out = S.h4_rule({"T1": row(30.0, 2.0), "T3": row(9.0, 0.7)})
        self.assertEqual(out["outcome"], "H4 confirmed")
        self.assertEqual(out["best"], "T1")
        self.assertEqual(out["g_star"], 30.0)

    def test_confirmed_without_t3(self):
        self.assertEqual(S.h4_rule({"T1": row(30.0, 2.0)})["outcome"], "H4 confirmed")

    def test_a_rise_below_a_factor_of_two_is_rejected(self):
        # 3 sigma above G6 but under 2x: not a confirmation
        out = S.h4_rule({"T1": row(14.0, 0.5)})
        self.assertEqual(out["outcome"], "H4 rejected")

    def test_a_biased_configuration_does_not_count(self):
        out = S.h4_rule({"T1": row(50.0, 3.0, unbiased=False), "T2": row(9.0, 0.6)})
        self.assertEqual(out["outcome"], "H4 rejected")
        self.assertEqual(out["best"], "T2")

    def test_version_rise_blocks_attribution(self):
        out = S.h4_rule({"T1": row(40.0, 3.0), "T3": row(25.0, 2.0)})
        self.assertEqual(out["outcome"], "attribution not established")

    def test_t2_alone_can_confirm(self):
        out = S.h4_rule({"T1": row(9.5, 0.7), "T2": row(60.0, 4.0)})
        self.assertEqual(out["outcome"], "H4 confirmed")
        self.assertEqual(out["best"], "T2")

    def test_nothing_evaluated(self):
        self.assertEqual(S.h4_rule({})["outcome"], "NOT_EVALUATED")
        self.assertEqual(S.h4_rule({"T1": row(None, None)})["outcome"], "NOT_EVALUATED")


class T3Trigger(unittest.TestCase):
    def test_runs_only_on_a_3_sigma_difference(self):
        self.assertFalse(S.t3_required(row(9.5, 0.7)))
        self.assertTrue(S.t3_required(row(20.0, 1.5)))
        self.assertTrue(S.t3_required(row(4.0, 0.3)))
        self.assertFalse(S.t3_required(None))


class Nuc050(unittest.TestCase):
    def test_bands(self):
        self.assertTrue(S.nuc050_band(120.0).startswith(">= 100"))
        self.assertTrue(S.nuc050_band(40.0).startswith("10-100"))
        self.assertTrue(S.nuc050_band(8.0).startswith("< 10"))
        self.assertEqual(S.nuc050_band(None), "NOT_EVALUATED")


class Exceeds(unittest.TestCase):
    def test_combined_sigma(self):
        # difference 3.0, combined sigma hypot(0.6, 0.62) = 0.863 -> 3.48 sigma
        self.assertTrue(S.exceeds(11.87, 0.6, 8.87, 0.62))
        self.assertFalse(S.exceeds(10.0, 0.6, 8.87, 0.62))
        self.assertFalse(S.exceeds(None, None, 8.87, 0.62))
        self.assertAlmostEqual(math.hypot(0.6, 0.62), 0.8628, places=3)

    def test_g6_reference_matches_s0b(self):
        # FOM 0.295 +- 0.019 against D2's 0.0332 +- 0.0008 (S0b record): gain 8.87 +- 0.62
        g = C.gain_with_sigma({"fom": 0.29457, "relative_sigma": 0.019 / 0.295}, {"fom": 0.033182, "relative_sigma": 0.02561})
        self.assertAlmostEqual(g["gain"], S.G6_GAIN, delta=0.01)
        self.assertAlmostEqual(g["sigma"], S.G6_GAIN_SIGMA, delta=0.03)


if __name__ == "__main__":
    unittest.main()
