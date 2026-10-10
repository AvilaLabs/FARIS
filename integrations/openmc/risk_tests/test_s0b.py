"""Tests for the S0b diagnostic helpers (s0b_common, the S0b configs). Standard library only; no OpenMC, CadQuery or numpy.

Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import json
import math
import tempfile
import unittest
from pathlib import Path

import r1_configs as R1
import s0b_common as C


class FigureOfMerit(unittest.TestCase):
    def test_fom_definition(self):
        self.assertAlmostEqual(C.fom(0.1, 100.0), 1.0)
        self.assertIsNone(C.fom(None, 10.0))
        self.assertIsNone(C.fom(0.0, 10.0))
        self.assertIsNone(C.fom(0.1, 0.0))

    def test_relative_sigma_is_sqrt_two_over_n_minus_one(self):
        self.assertAlmostEqual(C.fom_relative_sigma(3), 1.0)
        self.assertAlmostEqual(C.fom_relative_sigma(201), 0.1)
        with self.assertRaises(ValueError):
            C.fom_relative_sigma(1)

    def test_fom_with_sigma(self):
        f = C.fom_with_sigma(0.1, 100.0, 201)
        self.assertAlmostEqual(f["fom"], 1.0)
        self.assertAlmostEqual(f["sigma"], 0.1)

    def test_gain_adds_relative_sigmas_in_quadrature(self):
        a = C.fom_with_sigma(0.05, 100.0, 201)
        ref = C.fom_with_sigma(0.2, 100.0, 801)
        g = C.gain_with_sigma(a, ref)
        self.assertAlmostEqual(g["gain"], 16.0)
        self.assertAlmostEqual(g["sigma"], 16.0 * math.hypot(0.1, math.sqrt(2.0 / 800)))

    def test_gain_without_fom(self):
        self.assertIsNone(C.gain_with_sigma(C.fom_with_sigma(None, 1.0, 10), C.fom_with_sigma(0.1, 1.0, 10))["gain"])

    def test_improved_needs_more_than_one_combined_sigma(self):
        g0 = {"fom": 1.0, "sigma": 0.3}
        self.assertTrue(C.improved_over({"fom": 1.6, "sigma": 0.4}, g0))      # 0.6 > hypot(0.3, 0.4) = 0.5
        self.assertFalse(C.improved_over({"fom": 1.4, "sigma": 0.4}, g0))     # 0.4 < 0.5
        self.assertFalse(C.improved_over({"fom": 1.5, "sigma": 0.4}, g0))     # exactly 0.5 is not more than
        self.assertFalse(C.improved_over({"fom": None, "sigma": None}, g0))


class Statistics(unittest.TestCase):
    def test_z_score(self):
        self.assertAlmostEqual(C.z_score(10.0, 3.0, 6.0, 4.0), 0.8)
        self.assertIsNone(C.z_score(1.0, 0.0, 2.0, 0.0))

    def test_unbiased_is_within_three_sigma(self):
        self.assertTrue(C.unbiased(2.99))
        self.assertTrue(C.unbiased(-3.0))
        self.assertFalse(C.unbiased(3.01))
        self.assertIsNone(C.unbiased(None))

    def test_ratio_sigma(self):
        self.assertAlmostEqual(C.ratio_sigma(2.0, 0.2, 4.0, 0.2), 0.5 * math.hypot(0.1, 0.05))
        self.assertIsNone(C.ratio_sigma(0.0, 1.0, 1.0, 1.0))

    def test_ratio_of_means_matches_independent_formula(self):
        num = [1.0, 2.0, 3.0, 4.0]
        den = [10.0, 10.0, 10.0, 10.0]  # constant denominator: no covariance, no variance
        r = C.ratio_of_means_sigma(num, den)
        self.assertAlmostEqual(r["ratio"], 0.25)
        sd = math.sqrt(sum((x - 2.5) ** 2 for x in num) / 3)
        self.assertAlmostEqual(r["std_error"], sd / math.sqrt(4) / 10.0)

    def test_perfectly_correlated_series_have_no_ratio_noise(self):
        den = [1.0, 2.0, 3.0, 4.0]
        num = [0.5 * x for x in den]
        r = C.ratio_of_means_sigma(num, den)
        self.assertAlmostEqual(r["ratio"], 0.5)
        self.assertAlmostEqual(r["std_error"], 0.0, places=12)

    def test_ratio_of_means_needs_two_batches(self):
        with self.assertRaises(ValueError):
            C.ratio_of_means_sigma([1.0], [1.0])


class D1Rule(unittest.TestCase):
    @staticmethod
    def run_(rate, lost=0, a=1.0, tbr=1.2):
        return {"rate": rate, "histories": 1_000_000, "lost": lost, "a": {"mean": a, "std_error": 0.05},
                "tbr": {"mean": tbr, "std_error": 0.01}}

    def test_adopts_when_all_hold(self):
        out = C.d1_rule(self.run_(3000.0), self.run_(500.0))
        self.assertTrue(out["adopt_double_down"])
        self.assertEqual(out["selected"], "openmc0153dd")
        self.assertAlmostEqual(out["speedup"], 6.0)

    def test_speedup_below_three_keeps_moab(self):
        out = C.d1_rule(self.run_(1499.0), self.run_(500.0))
        self.assertFalse(out["speedup_ok"])
        self.assertEqual(out["selected"], "w003env")

    def test_exactly_three_is_enough(self):
        self.assertTrue(C.d1_rule(self.run_(1500.0), self.run_(500.0))["speedup_ok"])

    def test_one_lost_particle_in_a_million_is_allowed_two_are_not(self):
        self.assertTrue(C.d1_rule(self.run_(3000.0, lost=1), self.run_(500.0))["lost_ok"])
        self.assertFalse(C.d1_rule(self.run_(3000.0, lost=2), self.run_(500.0))["lost_ok"])

    def test_response_disagreement_beyond_three_sigma_rejects(self):
        out = C.d1_rule(self.run_(3000.0, a=1.0 + 0.05 * math.sqrt(2) * 3.1), self.run_(500.0))
        self.assertFalse(out["a_ok"])
        self.assertFalse(out["adopt_double_down"])
        out = C.d1_rule(self.run_(3000.0, tbr=1.2 + 0.01 * math.sqrt(2) * 3.1), self.run_(500.0))
        self.assertFalse(out["tbr_ok"])


class G5Selection(unittest.TestCase):
    G0 = {"fom": 10.0, "sigma": 2.0}

    @staticmethod
    def f(v, s=2.0):
        return {"fom": v, "sigma": s}

    def test_nothing_improved_means_g5_equals_g0(self):
        out = C.select_g5(self.G0, self.f(11.0), self.f(9.0), self.f(10.5), self.f(8.0))
        self.assertIsNone(out["mesh_and_shape"])
        self.assertFalse(out["regenerate"])
        self.assertTrue(out["identical_to_g0"])

    def test_best_of_g1_and_g2_is_taken_when_both_improved(self):
        out = C.select_g5(self.G0, self.f(20.0), self.f(25.0), None, None)
        self.assertEqual(out["mesh_and_shape"], "G2")
        self.assertTrue(out["regenerate"])

    def test_only_the_improved_one_of_g1_g2_counts(self):
        out = C.select_g5(self.G0, self.f(30.0), self.f(11.0), None, None)
        self.assertEqual(out["mesh_and_shape"], "G1")

    def test_g3_and_g4_are_added_independently(self):
        out = C.select_g5(self.G0, None, None, self.f(20.0), self.f(9.0))
        self.assertTrue(out["max_split_1000"])
        self.assertFalse(out["checkpoints"])
        self.assertFalse(out["regenerate"])
        self.assertFalse(out["identical_to_g0"])

    def test_biased_configuration_is_not_eligible(self):
        out = C.select_g5(self.G0, self.f(30.0), self.f(20.0), None, None, unbiased_flags={"G1": False})
        self.assertEqual(out["mesh_and_shape"], "G2")

    def test_arithmetic_is_recorded(self):
        out = C.select_g5(self.G0, self.f(20.0), None, None, None)
        a = out["arithmetic"]["G1"]
        self.assertAlmostEqual(a["difference"], 10.0)
        self.assertAlmostEqual(a["combined_sigma"], math.hypot(2.0, 2.0))
        self.assertFalse(out["arithmetic"]["G2"]["improved"])


class BestGain(unittest.TestCase):
    def test_largest_gain_among_unbiased(self):
        gains = {"G0": {"gain": 5.0}, "G1": {"gain": 40.0}, "G2": {"gain": 80.0}}
        out = C.best_unbiased(gains, {"G0": True, "G1": True, "G2": False})
        self.assertEqual(out["best"], "G1")
        self.assertEqual(out["band"], "10 <= G* < 100")

    def test_not_evaluated_is_not_eligible(self):
        out = C.best_unbiased({"G0": {"gain": 5.0}}, {"G0": None})
        self.assertIsNone(out["best"])

    def test_bands(self):
        self.assertEqual(C.best_unbiased({"a": {"gain": 100.0}}, {"a": True})["band"], "G* >= 100")
        self.assertEqual(C.best_unbiased({"a": {"gain": 9.99}}, {"a": True})["band"], "G* < 10")


class D4Rule(unittest.TestCase):
    def test_both_runs_must_pass(self):
        ok = {"tbr_r": 0.02, "a_r": 0.2}
        bad = {"tbr_r": 0.021, "a_r": 0.2}
        self.assertTrue(C.d4_rule([ok, ok])["perf_025_keeps_numbers"])
        self.assertFalse(C.d4_rule([ok, bad])["perf_025_keeps_numbers"])
        self.assertFalse(C.d4_rule([ok, {"tbr_r": None, "a_r": 0.1}])["perf_025_keeps_numbers"])


class Bateman(unittest.TestCase):
    def test_short_irradiation_is_production_times_time(self):
        lam = 1.0e-12
        out = C.bateman_single_step(2.0, lam, 1000.0, 0.0)
        self.assertAlmostEqual(out["atoms_end_irradiation"], 2000.0, places=3)

    def test_saturation(self):
        lam = 1.0e-3
        out = C.bateman_single_step(5.0, lam, 1.0e6, 0.0)
        self.assertAlmostEqual(out["atoms_end_irradiation"], 5.0 / lam)
        self.assertAlmostEqual(out["activity_cooled"], 5.0)

    def test_cooling_is_exponential(self):
        lam = math.log(2.0) / 100.0
        out = C.bateman_single_step(1.0, lam, 1.0e9, 300.0)
        self.assertAlmostEqual(out["atoms_cooled"] / out["atoms_end_irradiation"], 0.125)

    def test_time_correction_matches_openmc_d1s_form(self):
        # openmc.deplete.d1s: h = rate * (1 - exp(-lam T)) after one step, then h * exp(-lam tau) after decay
        lam, T, tau = 3.0e-8, 365.25 * 86400.0, 86400.0
        out = C.bateman_single_step(1.0, lam, T, tau)
        self.assertAlmostEqual(out["time_correction"], (1.0 - math.exp(-lam * T)) * math.exp(-lam * tau))
        self.assertAlmostEqual(out["activity_cooled"], out["time_correction"])

    def test_decay_constant_must_be_positive(self):
        with self.assertRaises(ValueError):
            C.bateman_single_step(1.0, 0.0, 1.0, 1.0)


class D5aRule(unittest.TestCase):
    def test_defect_needs_three_sigma_and_two_percent(self):
        self.assertTrue(C.d5a_rule(0.7, 0.01, 1.0, 0.01)["defect"])        # 30 %, 21 sigma
        self.assertFalse(C.d5a_rule(0.985, 0.001, 1.0, 0.001)["defect"])   # 1.5 % is below the 2 % floor
        self.assertFalse(C.d5a_rule(0.95, 0.04, 1.0, 0.04)["defect"])      # 5 % but under 3 sigma
        self.assertTrue(C.d5a_rule(1.05, 0.005, 1.0, 0.005)["defect"])

    def test_boundary_values_are_not_defects(self):
        # 1.9 % with a tiny error is not more than 2 %; 2.1 % is
        self.assertFalse(C.d5a_rule(0.981, 1.0e-6, 1.0, 1.0e-6)["defect"])
        self.assertTrue(C.d5a_rule(0.979, 1.0e-6, 1.0, 1.0e-6)["defect"])

    def test_zero_reference_is_not_evaluated(self):
        self.assertIsNone(C.d5a_rule(1.0, 0.1, 0.0, 0.0)["defect"])

    def test_zero_d1s_uses_its_error_over_the_reference(self):
        out = C.d5a_rule(0.0, 0.01, 1.0, 0.0)
        self.assertAlmostEqual(out["ratio"], 0.0)
        self.assertAlmostEqual(out["sigma"], 0.01)
        self.assertTrue(out["defect"])

    def test_h1_confirmation(self):
        # ratio 0.40 +- 0.02 and a partial fraction 0.62 +- 0.01: 1 - ratio = 0.60, difference -0.02, sigma 0.0224
        self.assertTrue(C.h1_confirmed(0.40, 0.02, 0.62, 0.01)["confirmed"])
        # ratio 0.40 +- 0.005 and a fraction 0.10 +- 0.005: difference 0.5
        self.assertFalse(C.h1_confirmed(0.40, 0.005, 0.10, 0.005)["confirmed"])

    def test_shares_and_one_percent_threshold(self):
        a = C.shares({"x": 98.0, "y": 1.0, "z": 1.0})
        b = C.shares({"x": 99.5, "y": 0.4, "z": 0.1})
        self.assertEqual(C.nuclides_above(a, b), ["x", "y", "z"])
        b2 = C.shares({"x": 99.0, "y": 0.9, "z": 0.1})
        a2 = C.shares({"x": 99.5, "y": 0.4, "z": 0.1})
        self.assertEqual(C.nuclides_above(a2, b2), ["x"])
        self.assertEqual(C.shares({"x": 0.0}), {"x": 0.0})


class D5cRule(unittest.TestCase):
    def test_seed_spread(self):
        s = C.seed_spread([9.0, 10.0, 11.0])
        self.assertAlmostEqual(s["mean"], 10.0)
        self.assertAlmostEqual(s["std_dev"], 1.0)
        self.assertAlmostEqual(s["relative_std_dev"], 0.1)

    def test_understated_when_spread_exceeds_three_times_photon_r(self):
        self.assertTrue(C.d5c_rule([9.0, 10.0, 11.0], 0.03)["uncertainty_understated"])    # 0.1 > 0.09
        self.assertFalse(C.d5c_rule([9.0, 10.0, 11.0], 0.034)["uncertainty_understated"])  # 0.1 < 0.102
        self.assertFalse(C.d5c_rule([9.0, 10.0, 11.0], 1.0 / 30.0)["uncertainty_understated"])  # 0.1 is not more than 0.1

    def test_needs_two_seeds(self):
        with self.assertRaises(ValueError):
            C.seed_spread([1.0])


class LogParsing(unittest.TestCase):
    TRANSPORT = """
 Total time for initialization     = 5.5292e+00 seconds
   Reading cross sections          = 3.2752e+00 seconds
 Total time in simulation          = 9.2524e+01 seconds
   Time in transport only          = 9.2099e+01 seconds
 Total time elapsed                = 9.8061e+01 seconds
 Calculation Rate (active)         = 10808 particles/second
 Leakage Fraction            = 0.00004 +/- 0.00001
 Using the DOUBLE-DOWN interface to Embree
"""
    RANDOM_RAY = """
 =====>     FORWARD FLUX SOLVE     <====
 =====================>     SIMULATION STATISTICS     <=====================
 Total Iterations                  = 60
 Number of Rays per Iteration      = 20000
 Source Regions (SRs)              = 172333
 SRs Containing External Sources   = 11614
 Avg SR Miss Rate per Iteration    = 6.0354%
 Energy Groups                     = 8
 Adjoint Flux Mode                 = OFF
 Source Shape                      = Flat
 =====================>     SIMULATION STATISTICS     <=====================
 Total Iterations                  = 60
 Source Regions (SRs)              = 172333
 SRs Containing External Sources   = 155424
 Adjoint Flux Mode                 = ON
 Source Shape                      = Flat
"""

    def test_transport_log(self):
        out = C.parse_transport_log(self.TRANSPORT)
        self.assertAlmostEqual(out["initialization_s"], 5.5292)
        self.assertAlmostEqual(out["reading_cross_sections_s"], 3.2752)
        self.assertAlmostEqual(out["rate_active_particles_per_s"], 10808.0)
        self.assertTrue(out["double_down"])
        self.assertFalse(C.parse_transport_log("no timing here")["double_down"])

    def test_random_ray_log_splits_forward_and_adjoint(self):
        out = C.parse_random_ray_log(self.RANDOM_RAY)
        self.assertEqual(len(out["solves"]), 2)
        self.assertEqual(out["forward"]["source_regions_with_external_source"], 11614.0)
        self.assertEqual(out["adjoint"]["source_regions_with_external_source"], 155424.0)
        self.assertEqual(out["forward"]["source_regions"], 172333.0)
        self.assertEqual(out["forward"]["avg_sr_miss_rate_percent"], 6.0354)


class Ledger(unittest.TestCase):
    def test_entries_accumulate(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "ledger.json"
            p.write_text(json.dumps({"start_epoch": 0.0, "entries": []}), encoding="utf-8")
            C.ledger_add(p, "D1", "dd", 100.0, returncode=0)
            data = C.ledger_add(p, "D1", "moab", 250.5)
            self.assertAlmostEqual(data["job_wall_total_s"], 350.5)
            self.assertEqual(len(data["entries"]), 2)
            self.assertEqual(json.loads(p.read_text(encoding="utf-8"))["entries"][0]["returncode"], 0)


class S0bConfigs(unittest.TestCase):
    def test_seeds_are_new_and_disjoint_from_s0(self):
        self.assertTrue(R1.s0b_seeds_disjoint_from_s0())

    def test_g1_rays_are_fifteen_point_six_times_g0(self):
        self.assertEqual(R1.S0B_CONFIGS["G1"]["rays"], round(R1.S0B_CONFIGS["G0"]["rays"] * R1.S0B_RAYS_FACTOR_G1))
        self.assertEqual(R1.S0B_CONFIGS["G1"]["ww_cell_cm"], 10.0)
        self.assertEqual(R1.S0B_CONFIGS["G2"]["source_shape"], "linear")
        self.assertEqual(R1.S0B_CONFIGS["G0"]["ww_cell_cm"], 25.0)

    def test_g0_is_s0_c2(self):
        self.assertEqual(R1.S0B_CONFIGS["G0"]["structure"], R1.CONFIGS["C2"]["structure"])

    def test_g6_adds_a_zero_point_one_mev_edge_to_casmo_8(self):
        g6 = R1.STRUCTURES[R1.S0B_CONFIGS["G6"]["structure"]]
        self.assertEqual(len(g6), len(R1.CASMO_8) + 1)
        self.assertIn(1.0e5, g6)
        self.assertEqual(sorted(set(g6) - {1.0e5}), R1.CASMO_8)
        # the adjoint response then covers exactly E > 0.1 MeV
        self.assertEqual(R1.fast_response_edges(R1.S0B_CONFIGS["G6"]["structure"]), [1.0e5, 821000.0, 2.0e7])
        self.assertEqual(R1.fast_response_edges("CASMO-8"), [821000.0, 2.0e7])


if __name__ == "__main__":
    unittest.main()
