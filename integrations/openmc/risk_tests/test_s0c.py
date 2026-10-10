"""Tests for the S0c helpers (s0c_common). Standard library only; no OpenMC, CadQuery or numpy.

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
import s0c_common as S


def row(spacing, fom, sigma, unbiased=True):
    return {"spacing": spacing, "fom": fom, "sigma": sigma, "unbiased": unbiased}


class SlabLayers(unittest.TestCase):
    def test_layers_follow_the_inboard_build(self):
        layers = S.p1_layers()
        names = [x["name"] for x in layers]
        self.assertEqual(names[0], S.PLASMA_LAYER_NAME)
        self.assertEqual(names[1:3], ["scrape_off_layer", "first_wall_tungsten"])
        self.assertEqual(names[-2:], ["gap_to_tf", "tf_inboard_leg"])
        self.assertEqual(layers[-1]["tag"], "tf_coil")
        self.assertAlmostEqual(layers[-1]["thickness_cm"], 64.0)

    def test_layers_are_contiguous_and_total_200(self):
        layers = S.p1_layers()
        self.assertAlmostEqual(layers[0]["x0_cm"], 0.0)
        for a, b in zip(layers, layers[1:]):
            self.assertAlmostEqual(a["x1_cm"], b["x0_cm"])
        self.assertAlmostEqual(layers[-1]["x1_cm"], S.SLAB_TOTAL_CM)
        self.assertAlmostEqual(sum(x["thickness_cm"] for x in layers), S.SLAB_TOTAL_CM)

    def test_plasma_layer_fills_the_rest(self):
        layers = S.p1_layers()
        self.assertAlmostEqual(layers[0]["thickness_cm"], 47.0)
        self.assertEqual(layers[0]["tag"], "void")

    def test_edges(self):
        e = S.layer_edges(S.p1_layers())
        self.assertEqual(len(e), len(S.p1_layers()) + 1)
        self.assertEqual(e[0], 0.0)
        self.assertEqual(e[-1], 200.0)


class SourceRegions(unittest.TestCase):
    def test_mesh_dimension(self):
        self.assertEqual(S.mesh_dimension(200.0, 25.0), 8)
        self.assertEqual(S.mesh_dimension(200.0, 2.5), 80)
        with self.assertRaises(ValueError):
            S.mesh_dimension(200.0, 30.0)

    def test_count_is_union_of_boundaries_and_mesh_edges(self):
        # two layers split at 50; a 100 cm mesh of 4 cells at 25: edges 0,25,50,75,100 -> 4 regions
        self.assertEqual(S.source_region_count([0.0, 50.0, 100.0], 25.0), 4)
        # a layer boundary at 60 is not on the mesh: one more region
        self.assertEqual(S.source_region_count([0.0, 60.0, 100.0], 25.0), 5)

    def test_count_for_p1(self):
        e = S.layer_edges(S.p1_layers())
        counts = [S.source_region_count(e, d) for d in S.SPACINGS_CM]
        self.assertEqual(counts, [19, 31, 49, 89])
        self.assertEqual(counts, sorted(counts))

    def test_estimate_3d_is_volume_over_cell(self):
        self.assertAlmostEqual(S.source_region_estimate_3d([1000.0], 10.0), 1.0)
        self.assertAlmostEqual(S.source_region_estimate_3d([8000.0, 1.0], 10.0), 9.0)


class WindowFloor(unittest.TestCase):
    def test_raises_small_positive_bounds(self):
        r = S.apply_floor([1.0, 1e-12, 0.5], [5.0, 5e-12, 2.5], 1e-8)
        self.assertEqual(r["raised"], 1)
        self.assertAlmostEqual(r["floor_value"], 1e-8)
        self.assertAlmostEqual(r["lower"][1], 1e-8)
        self.assertEqual(r["lower"][0], 1.0)

    def test_upper_scales_with_lower(self):
        r = S.apply_floor([1.0, 1e-12], [5.0, 5e-12], 1e-8)
        self.assertAlmostEqual(r["upper"][1] / r["lower"][1], 5.0)

    def test_bound_at_the_floor_is_not_raised(self):
        r = S.apply_floor([1.0, 1e-8], [5.0, 5e-8], 1e-8)
        self.assertEqual(r["raised"], 0)

    def test_non_positive_bounds_are_left_alone(self):
        r = S.apply_floor([1.0, 0.0, -1.0], [5.0, 0.0, -1.0], 1e-8)
        self.assertEqual(r["non_positive"], 2)
        self.assertEqual(r["lower"], [1.0, 0.0, -1.0])

    def test_length_mismatch(self):
        with self.assertRaises(ValueError):
            S.apply_floor([1.0], [1.0, 2.0])

    def test_range(self):
        r = S.window_range([0.0, 1e-3, 1.0])
        self.assertEqual(r["min_positive"], 1e-3)
        self.assertEqual(r["max"], 1.0)
        self.assertAlmostEqual(r["dynamic_range"], 1000.0)
        self.assertAlmostEqual(r["fraction_positive"], 2.0 / 3.0)
        self.assertIsNone(S.window_range([0.0, -1.0])["dynamic_range"])


class ResolutionRule(unittest.TestCase):
    def test_coarsest_within_one_sigma_of_best(self):
        rows = [row(25.0, 1.0, 0.1), row(10.0, 2.0, 0.2), row(5.0, 3.0, 0.3), row(2.5, 3.2, 0.3)]
        # best 3.2 (2.5): 5 cm is 0.2 below, combined sigma 0.42 -> within; 10 cm 1.2 below, combined 0.36 -> not
        r = S.delta_star(rows)
        self.assertEqual(r["delta_star_cm"], 5.0)
        self.assertEqual(r["best_spacing_cm"], 2.5)
        self.assertEqual(r["within_one_sigma_of_best"], [2.5, 5.0])

    def test_exactly_one_sigma_counts(self):
        rows = [row(25.0, 3.0, 0.3), row(10.0, 4.0, 0.4)]   # gap 1.0, combined 0.5 -> not within
        self.assertEqual(S.delta_star(rows)["delta_star_cm"], 10.0)
        rows = [row(25.0, 3.5, 0.3), row(10.0, 4.0, 0.4)]   # gap 0.5 == combined 0.5 -> within
        self.assertEqual(S.delta_star(rows)["delta_star_cm"], 25.0)

    def test_biased_and_not_evaluated_are_excluded(self):
        rows = [row(25.0, 1.0, 0.1), row(10.0, 9.0, 0.1, unbiased=False), row(5.0, 2.0, 0.1, unbiased=None)]
        r = S.delta_star(rows)
        self.assertEqual(r["best_spacing_cm"], 25.0)
        self.assertEqual(r["delta_star_cm"], 25.0)
        self.assertEqual(sorted(r["excluded"]), [5.0, 10.0])

    def test_no_row(self):
        self.assertIsNone(S.delta_star([row(25.0, None, None)])["delta_star_cm"])

    def test_row_unbiased(self):
        self.assertTrue(S.row_unbiased(1.0, 0.1, 1.2, 0.1)["unbiased"])
        self.assertFalse(S.row_unbiased(1.0, 0.1, 2.0, 0.1)["unbiased"])

    def test_confirmation_needed_only_when_finer_than_25(self):
        self.assertTrue(S.rm_m_confirmation_required(10.0))
        self.assertFalse(S.rm_m_confirmation_required(25.0))
        self.assertFalse(S.rm_m_confirmation_required(None))


class FloorRule(unittest.TestCase):
    def fs(self, r, t=100.0, n=201):
        return C.fom_with_sigma(r, t, n)

    def test_adopted_when_better_and_unbiased(self):
        r = S.floor_rule(self.fs(0.2), self.fs(0.1), True)
        self.assertTrue(r["adopted"])

    def test_not_adopted_when_biased(self):
        self.assertFalse(S.floor_rule(self.fs(0.2), self.fs(0.1), False)["adopted"])
        self.assertFalse(S.floor_rule(self.fs(0.2), self.fs(0.1), None)["adopted"])

    def test_not_adopted_within_one_sigma(self):
        self.assertFalse(S.floor_rule(self.fs(0.1), self.fs(0.099), True)["adopted"])

    def test_not_adopted_when_worse(self):
        self.assertFalse(S.floor_rule(self.fs(0.1), self.fs(0.2), True)["adopted"])


class PreviewRule(unittest.TestCase):
    def test_keeps_numbers(self):
        r = S.q2_rule([{"tbr_r": 0.01, "a_r": 0.15}, {"tbr_r": 0.02, "a_r": 0.20}])
        self.assertTrue(r["perf_025_keeps_numbers"])
        self.assertIn("loaded session", r["outcome"])

    def test_one_run_misses(self):
        r = S.q2_rule([{"tbr_r": 0.01, "a_r": 0.15}, {"tbr_r": 0.01, "a_r": 0.21}])
        self.assertFalse(r["perf_025_keeps_numbers"])
        self.assertEqual(r["outcome"], "to change control")

    def test_tbr_misses(self):
        self.assertFalse(S.q2_rule([{"tbr_r": 0.03, "a_r": 0.1}, {"tbr_r": 0.01, "a_r": 0.1}])["perf_025_keeps_numbers"])


class H2Rule(unittest.TestCase):
    def mesh(self, r20, r10, r2, s=0.01):
        return {20.0: {"ratio": r20, "sigma": s}, 10.0: {"ratio": r10, "sigma": s}, 2.0: {"ratio": r2, "sigma": s}}

    def test_ratio_row(self):
        r = S.ratio_row(1.1, 0.01, 1.0, 0.01)
        self.assertAlmostEqual(r["ratio"], 1.1)
        self.assertAlmostEqual(r["sigma"], 1.1 * math.hypot(0.01, 0.01))
        self.assertIsNone(S.ratio_row(0.0, 0.1, 1.0, 0.1)["ratio"])

    def test_confirmed(self):
        r = S.h2_rule(self.mesh(1.4, 1.2, 1.02), {"ratio": 0.99, "sigma": 0.01})
        self.assertEqual(r["outcome"], "CONFIRMED")

    def test_rejected_when_ten_cm_ratio_near_one(self):
        r = S.h2_rule(self.mesh(1.04, 1.03, 1.0), {"ratio": 1.0, "sigma": 0.01})
        self.assertEqual(r["outcome"], "REJECTED")

    def test_rejected_when_not_monotone(self):
        r = S.h2_rule(self.mesh(1.2, 1.4, 1.02), {"ratio": 1.0, "sigma": 0.01})
        self.assertEqual(r["outcome"], "REJECTED")

    def test_exactly_five_percent_is_within(self):
        r = S.h2_rule(self.mesh(1.4, 1.2, 1.05), {"ratio": 0.95, "sigma": 0.01})
        self.assertEqual(r["outcome"], "CONFIRMED")
        r = S.h2_rule(self.mesh(1.4, 1.2, 1.0501), {"ratio": 0.95, "sigma": 0.01})
        self.assertEqual(r["outcome"], "NEITHER")

    def test_neither_when_cell_based_is_off(self):
        r = S.h2_rule(self.mesh(1.4, 1.2, 1.02), {"ratio": 1.3, "sigma": 0.01})
        self.assertEqual(r["outcome"], "NEITHER")

    def test_high_needs_three_sigma_and_five_percent(self):
        # excess 0.08 but sigma 0.03 -> 3 sigma = 0.09 -> not high; not within 5 % either -> NEITHER
        r = S.h2_rule(self.mesh(1.3, 1.08, 1.0, s=0.03), {"ratio": 1.0, "sigma": 0.01})
        self.assertFalse(r["ten_cm_high"])
        self.assertEqual(r["outcome"], "NEITHER")

    def test_missing_ratio(self):
        r = S.h2_rule(self.mesh(1.4, None, 1.0), {"ratio": 1.0, "sigma": 0.01})
        self.assertEqual(r["outcome"], "NOT_EVALUATED")


class Ledger(unittest.TestCase):
    def test_init_add_and_budget(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "ledger.json"
            S.ledger_init(path, cap_s=3600.0)
            self.assertAlmostEqual(S.budget_left(path), 3600.0)
            C.ledger_add(path, "Q1", "smoke", 120.0)
            C.ledger_add(path, "Q1", "run", 480.0)
            self.assertAlmostEqual(S.budget_left(path), 3000.0)
            self.assertAlmostEqual(json.loads(path.read_text())["job_wall_total_s"], 600.0)

    def test_fits(self):
        self.assertTrue(S.fits(100.0, 100.0))
        self.assertFalse(S.fits(100.0, 100.0, reserve_s=1.0))


class Seeds(unittest.TestCase):
    def test_disjoint_from_s0_and_s0b_and_each_other(self):
        flat = [S.SEEDS["mgxs"], S.SEEDS["analog"], S.SEEDS["q2"], S.SEEDS["q3_d1s"], S.SEEDS["q3_neutron"], S.SEEDS["q3_photon"]]
        flat += list(S.SEEDS["gen"].values()) + list(S.SEEDS["prod"].values())
        self.assertEqual(len(flat), len(set(flat)))
        old = set(R1.GENERATION_SEEDS.values()) | set(R1.PRODUCTION_SEEDS.values()) | {R1.ANALOG_SEED, R1.MGXS_SEED}
        old |= set(R1.S0B_GENERATION_SEEDS.values()) | set(R1.S0B_PRODUCTION_SEEDS.values()) | {R1.S0B_MGXS_SEED}
        self.assertFalse(old & set(flat))


if __name__ == "__main__":
    unittest.main()
