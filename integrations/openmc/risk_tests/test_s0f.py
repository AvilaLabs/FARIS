"""Tests for the S0f helpers and rules (s0f_common). Standard library only.

Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import math
import unittest

import s0f_common as F


class Neighbours(unittest.TestCase):
    def test_interior_bin_has_six(self):
        dims = (3, 3, 3)
        centre = 1 + 3 * (1 + 3 * 1)
        self.assertEqual(len(F.neighbours(centre, dims)), 6)

    def test_corner_bin_has_three(self):
        self.assertEqual(F.neighbours(0, (3, 3, 3)), [1, 3, 9])

    def test_periodic_axis_wraps(self):
        dims = (1, 4, 1)  # one radial bin, four phi bins
        self.assertEqual(F.neighbours(0, dims, periodic_axis=1), [1, 3])
        self.assertEqual(F.neighbours(0, dims), [1])


class Fill(unittest.TestCase):
    def test_single_hole_is_geometric_mean_of_neighbours(self):
        dims = (3, 1, 1)
        out, n = F.fill_invalid([1e-6, -1.0, 1e-8], dims)
        self.assertEqual(n, 1)
        self.assertAlmostEqual(math.log10(out[1]), -7.0)
        self.assertEqual(out[0], 1e-6)
        self.assertEqual(out[2], 1e-8)

    def test_hole_inside_a_hole_fills_in_two_sweeps(self):
        dims = (5, 1, 1)
        out, n = F.fill_invalid([1.0, -1.0, -1.0, -1.0, 1.0], dims)
        self.assertEqual(n, 3)
        self.assertTrue(all(v > 0 for v in out))
        self.assertAlmostEqual(out[2], 1.0)

    def test_order_independent(self):
        dims = (4, 1, 1)
        a, _ = F.fill_invalid([1e-2, -1.0, -1.0, 1e-6], dims)
        b, _ = F.fill_invalid(list(reversed([1e-2, -1.0, -1.0, 1e-6])), dims)
        self.assertEqual([round(math.log10(v), 12) for v in a], [round(math.log10(v), 12) for v in reversed(b)])

    def test_untouched_when_all_valid_or_all_invalid(self):
        self.assertEqual(F.fill_invalid([1.0, 2.0], (2, 1, 1)), ([1.0, 2.0], 0))
        self.assertEqual(F.fill_invalid([-1.0, -1.0], (2, 1, 1)), ([-1.0, -1.0], 0))

    def test_size_mismatch_raises(self):
        with self.assertRaises(ValueError):
            F.fill_invalid([1.0], (2, 1, 1))


class Production(unittest.TestCase):
    def test_batch_stats_constant_offsets(self):
        st = F.batch_stats([1.0, 3.0, 1.0, 3.0], per_batch=10)
        self.assertAlmostEqual(st["mean"], 2.0)
        var = 4.0 / 3.0  # sample variance of the four values
        self.assertAlmostEqual(st["var_per_history"], var * 10 / 4.0)
        self.assertAlmostEqual(st["relative_error"], math.sqrt(var / 4) / 2.0)
        self.assertAlmostEqual(st["vov"], 4 * 1.0 / 16.0 - 0.25)

    def test_vov_flags_a_single_spike(self):
        st = F.batch_stats([1.0] * 99 + [1000.0], per_batch=1)
        self.assertGreater(st["vov"], F.VOV_LIMIT)

    def test_seed_lower_bound(self):
        b = F.seed_lower_bound([100.0, 120.0, 140.0, 160.0])
        self.assertAlmostEqual(b["mean"], 130.0)
        self.assertAlmostEqual(b["lower95"], 130.0 - 2.353 * b["sd"] / 2.0)
        with self.assertRaises(ValueError):
            F.seed_lower_bound([1.0, 2.0])

    def test_nuc050_bands(self):
        self.assertTrue(F.nuc050_outcome(150, 110, True).startswith(">= 100 at the 95"))
        self.assertTrue(F.nuc050_outcome(150, 80, True).startswith(">= 100 at the point"))
        self.assertTrue(F.nuc050_outcome(50, 30, True).startswith("10-100"))
        self.assertTrue(F.nuc050_outcome(5, 3, True).startswith("< 10"))
        self.assertTrue(F.nuc050_outcome(150, 110, False).startswith("biased"))
        self.assertEqual(F.nuc050_outcome(None, None, True), "NOT_EVALUATED")

    def test_h7(self):
        self.assertEqual(F.h7_outcome([1.5, 2.0, 1.1, 3.0], 1.8), "H7 confirmed")
        self.assertEqual(F.h7_outcome([1.5, 0.9, 1.1, 3.0], 1.8), "H7 rejected")
        self.assertEqual(F.h7_outcome([1.5, 2.0, 1.1, 3.0], 1.1), "H7 rejected")

    def test_protocol2_rules(self):
        self.assertTrue(F.nuc050_outcome2(50, 30, True).startswith("10-100: target kept; trace"))
        self.assertTrue(F.nuc050_outcome2(150, 110, False).startswith("biased: collision aiming"))
        self.assertEqual(F.h8_outcome([1.5, 2.0, 1.1, 3.0], 1.8), "H8 confirmed")
        self.assertEqual(F.h8_outcome([1.5, 0.9, 1.1, 3.0], 1.8), "H8 rejected")
        self.assertEqual(F.h9_outcome([1.5, 2.0, 1.1, 3.0], 1.8), "H9 confirmed")

    def test_env_matches(self):
        self.assertTrue(F.env_matches("F0", {}))
        self.assertFalse(F.env_matches("F0", {"OPENMC_WW_SKIP_VOID": "1"}))
        f4 = dict(F.CONFIG_ENV["F4"])
        self.assertTrue(F.env_matches("F4", f4))
        f4["OPENMC_WW_ROULETTE_FACTOR"] = "10"
        self.assertFalse(F.env_matches("F4", f4))
        self.assertFalse(F.env_matches("F1", F.CONFIG_ENV["F4"]))
        self.assertFalse(F.env_matches("F5", F.CONFIG_ENV["F6"]))
        self.assertTrue(F.env_matches("F6", F.CONFIG_ENV["F6"]))

    def test_d2_z(self):
        self.assertAlmostEqual(F.z_against_d2(F.D2_MEAN, 1e-5), 0.0)


class LosWindows(unittest.TestCase):
    def setUp(self):
        try:
            import numpy  # noqa: F401
        except ImportError:
            self.skipTest("numpy not available")

    def test_solid_angle_far_and_sides(self):
        import numpy as np
        import s0f_los_windows as W
        c, u, v = np.array([0.0, 0, 0]), np.array([0.0, 1, 0]), np.array([0.0, 0, 1])  # normal +x, area 4
        far = W.solid_angle(np.array([[-100.0, 0, 0]]), c, u, v)[0]
        self.assertAlmostEqual(far, 4 / 100.0 ** 2, delta=1e-6)
        self.assertEqual(W.solid_angle(np.array([[100.0, 0, 0]]), c, u, v)[0], 0.0)  # front side
        near = W.solid_angle(np.array([[-1e-6, 0, 0]]), c, u, v)[0]
        self.assertAlmostEqual(near, 2 * math.pi, places=3)  # half sphere when touching the centre

    def test_reach_probability_matches_solid_angle(self):
        import numpy as np
        import s0f_los_windows as W
        # A point 100 cm in front of a 2 x 2 cm rectangle: phi = (1/A) cos/pi / d^2 per source neutron, so P = omega/4pi.
        area, d = 4.0, 100.0
        phi = (1.0 / area) * (1.0 / math.pi) / d ** 2 * area
        p = W.reach_probability(np.array([phi * 8.0]), area, 1.0, 8.0)[0]
        self.assertAlmostEqual(p, (area / d ** 2) / (4 * math.pi), delta=1e-9)

    def test_cap_bounds_never_raises_and_skips_unresolved(self):
        import numpy as np
        import s0f_los_windows as W
        lower = np.array([[10.0, 10.0, -1.0, 10.0]])
        out = W.cap_bounds(lower, np.array([1.0]), np.array([[0.5, 0.01, 0.5, 0.5]]), np.array([[0.1, 0.1, 0.1, 0.9]]))
        self.assertEqual(out.tolist(), [[2.0, 10.0, -1.0, 10.0]])

    def test_flat_index_matches_fortran_order(self):
        import numpy as np
        import s0f_los_windows as W
        dims, n_e = (3, 4, 5), 2
        a = np.arange(3 * 4 * 5 * n_e).reshape(3, 4, 5, n_e)
        flat = a.ravel("F")
        self.assertEqual(flat[W.flat_index(2, 1, 3, 1, dims)], a[2, 1, 3, 1])
        cen = W.voxel_centres([0, 0, 0], [1, 1, 1], dims)
        self.assertTrue(np.allclose(cen[W.flat_index(2, 1, 3, 0, dims)], [2.5, 1.5, 3.5]))


if __name__ == "__main__":
    unittest.main()
