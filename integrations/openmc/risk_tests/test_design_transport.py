"""Tests for ../design_transport.py: the pure helpers. Nothing here runs OpenMC."""

import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import design_transport as DT  # noqa: E402

SAMPLING = {"target_relative_error": 0.05, "particles_per_batch": 1000, "min_batches": 10, "max_batches": 100,
            "max_seconds": 60.0, "seed": 1, "threads": 2}
REQUEST = {
    "schema": DT.REQUEST_SCHEMA,
    "h5m": "model.h5m",
    "cross_sections": "cross_sections.xml",
    "void_solid_ids": ["plasma"],
    "materials": [{"solid_id": "coil", "nuclide_atom_fractions": {"Fe56": 1.0}, "density_kg_m3": 7874.0, "temperature_k": None}],
    "plasma": {},
    "response": {"solid_ids": ["coil"], "energy_min_ev": 1.0e5},
    "sampling": SAMPLING,
}


def stats_of(values):
    stats = DT.RunningSum()
    for value in values:
        stats.add(value)
    return stats


class RunningSumTest(unittest.TestCase):
    def test_mean_and_standard_error_match_the_textbook_values(self):
        stats = stats_of([1.0, 2.0, 3.0, 4.0])
        self.assertAlmostEqual(stats.mean, 2.5)
        # Sample variance 5/3; standard error of the mean sqrt(5/3/4).
        self.assertAlmostEqual(stats.standard_error, (5.0 / 3.0 / 4.0) ** 0.5)

    def test_one_batch_or_no_score_has_no_relative_error(self):
        self.assertIsNone(stats_of([1.0]).relative_error)
        self.assertIsNone(stats_of([0.0, 0.0, 0.0]).relative_error)


class StopRuleTest(unittest.TestCase):
    def test_target_is_not_accepted_before_the_minimum_batches(self):
        stats = stats_of([1.0] * 5)
        self.assertEqual(stats.relative_error, 0.0)
        self.assertIsNone(DT.stop_reason(stats, SAMPLING, 1.0))

    def test_target_reached_wins_over_a_limit_met_at_the_same_batch(self):
        stats = stats_of([1.0] * 100)
        self.assertEqual(DT.stop_reason(stats, SAMPLING, 1.0e9), "target_reached")

    def test_limits_stop_a_run_that_is_not_converged(self):
        noisy = [0.0, 10.0] * 50
        self.assertEqual(DT.stop_reason(stats_of(noisy), SAMPLING, 1.0), "history_limit")
        self.assertEqual(DT.stop_reason(stats_of(noisy[:20]), SAMPLING, 61.0), "time_limit")
        self.assertIsNone(DT.stop_reason(stats_of(noisy[:20]), SAMPLING, 1.0))

    def test_a_stopped_run_says_why_and_what_to_do(self):
        why, next_step = DT.explain("time_limit", stats_of([0.0, 10.0] * 10), SAMPLING)
        self.assertIn("time limit", why)
        self.assertIn("5 %", why)
        self.assertIn("times as many histories", next_step)
        why, next_step = DT.explain("history_limit", stats_of([0.0] * 20), SAMPLING)
        self.assertIn("no nonzero score", why)
        self.assertIn("weight windows", next_step)


class RequestTest(unittest.TestCase):
    def test_the_reference_request_is_valid(self):
        DT.validate_request(REQUEST)

    def test_bad_requests_are_refused(self):
        def changed(path, value):
            request = copy.deepcopy(REQUEST)
            target = request
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value
            return request

        for path, value in [
            (("schema",), "other"),
            (("sampling", "target_relative_error"), 0.0),
            (("sampling", "min_batches"), 1),
            (("sampling", "threads"), 64),
            (("response", "solid_ids"), []),
            (("response", "solid_ids"), ["missing"]),
            (("void_solid_ids",), ["coil"]),
            (("materials", 0, "density_kg_m3"), 0.0),
        ]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                DT.validate_request(changed(path, value))


class VoidNamesTest(unittest.TestCase):
    def test_only_the_void_solids_are_found(self):
        names = [b"mat:plasma".ljust(32, b"\0"), b"mat:plasma-facing".ljust(32, b"\0"), b"mat:coil".ljust(32, b"\0"),
                 b"picked".ljust(32, b"\0")]
        self.assertEqual(DT.void_names(names, ["plasma"]), [0])


class PlasmaTest(unittest.TestCase):
    def test_lengths_are_converted_to_centimetres(self):
        plasma = {"major_radius_m": 3.3, "minor_radius_m": 1.13, "elongation": 1.84, "triangularity": 0.375,
                  "shafranov_shift_m": 0.0, "pedestal_radius_m": 1.017, "mode": "H", "fuel": {"D": 0.5, "T": 0.5},
                  "ion_density_m3": {"centre": 1.5e20, "pedestal": 1.1e20, "separatrix": 4e19, "peaking_factor": 1.0},
                  "ion_temperature_ev": {"centre": 2e4, "pedestal": 4e3, "separatrix": 100.0, "peaking_factor": 1.5, "beta": 1.5}}
        arguments = DT.tokamak_arguments(plasma)
        self.assertAlmostEqual(arguments["major_radius"], 330.0)
        self.assertAlmostEqual(arguments["pedestal_radius"], 101.7)


if __name__ == "__main__":
    unittest.main()
