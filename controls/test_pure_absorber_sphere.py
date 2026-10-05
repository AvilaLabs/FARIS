"""Small independent checks for the analytic control's math and input bounds."""
import argparse
import math
import unittest

from pure_absorber_sphere import (
    analytic,
    batch_count,
    positive_float,
    positive_int,
    thread_count,
    validate_history_budget,
)


class AnalyticControlTests(unittest.TestCase):
    def test_exact_values_and_path_length_identity(self):
        values = analytic(10.0, 0.3)
        self.assertAlmostEqual(values["escape_probability"], math.exp(-3.0), places=15)
        self.assertAlmostEqual(values["absorption_probability"], 1.0 - math.exp(-3.0), places=15)
        self.assertAlmostEqual(
            0.3 * values["integrated_track_length_cm_per_source"],
            values["absorption_probability"], places=15,
        )

    def test_rejects_invalid_physics_inputs(self):
        for args in ((0.0, 0.3), (-1.0, 0.3), (math.inf, 0.3), (10.0, 0.0), (10.0, math.nan)):
            with self.subTest(args=args), self.assertRaises(ValueError):
                analytic(*args)

    # Verifies: NUC-048
    def test_rejects_invalid_cli_values(self):
        for raw in ("0", "-2", "nan", "inf"):
            with self.subTest(raw=raw), self.assertRaises(argparse.ArgumentTypeError):
                positive_float(raw)
        for raw in ("0", "-1"):
            with self.subTest(raw=raw), self.assertRaises(argparse.ArgumentTypeError):
                positive_int(raw)
        for raw in ("1", "29"):
            with self.subTest(raw=raw), self.assertRaises(argparse.ArgumentTypeError):
                batch_count(raw)
        self.assertEqual(batch_count("30"), 30)
        self.assertEqual(thread_count("8"), 8)
        with self.assertRaises(argparse.ArgumentTypeError):
            thread_count("33")
        validate_history_budget(100, 10000)
        with self.assertRaises(ValueError):
            validate_history_budget(1001, 10000)


if __name__ == "__main__":
    unittest.main()
