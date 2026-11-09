# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure activation-grid contract tests; dimensions are synthetic, not RM-M solver evidence."""
from __future__ import annotations

import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402


class R3ActivationGridTest(unittest.TestCase):
    def test_aligned_mesh_covers_exact_bounds_and_reports_estimate(self):
        grid = sc.activation_grid_spec((-230, -310, -90), (450, 330, 210), 100.0)
        self.assertEqual(grid["lower_left_cm"], [-300.0, -400.0, -100.0])
        self.assertEqual(grid["upper_right_cm"], [500.0, 400.0, 300.0])
        self.assertEqual(grid["dimension"], [8, 8, 4])
        self.assertEqual(grid["cell_count"], 256)
        self.assertEqual(grid["estimated_mesh_array_bytes"], 256 * (8 * 46 + 128))
        self.assertIn("excludes", grid["memory_estimate_scope"])

    def test_cell_and_memory_caps_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "cap"):
            sc.activation_grid_spec((0, 0, 0), (1000, 1000, 1000), 1.0, max_cells=100)
        with self.assertRaisesRegex(ValueError, "cell size"):
            sc.activation_grid_spec((0, 0, 0), (1, 1, 1), 0.0)
        with self.assertRaisesRegex(ValueError, "finite"):
            sc.activation_grid_spec((0, 0, 0), (1, float("inf"), 1), 1.0)


if __name__ == "__main__":
    unittest.main()
