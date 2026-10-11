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


if __name__ == "__main__":
    unittest.main()
