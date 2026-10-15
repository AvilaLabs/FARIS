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
    "solids": [{"solid_id": "plasma", "volume_id": 1, "material_id": None},
               {"solid_id": "coil-a", "volume_id": 2, "material_id": "tf-coil"},
               {"solid_id": "coil-b", "volume_id": 3, "material_id": "tf-coil"}],
    "materials": [{"material_id": "tf-coil", "nuclide_atom_fractions": {"Fe56": 1.0}, "density_kg_m3": 7874.0,
                   "temperature_k": None}],
    "plasma": {},
    "chamber_solid_id": "plasma",
    "response": {"solid_ids": ["coil-a", "coil-b"], "energy_min_ev": 1.0e5},
    "sampling": SAMPLING,
    "variance_reduction": "none",
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

    # Verifies: NUC-043
    # Verifies: NUC-041, UNC-005, UNC-007
    def test_a_run_one_batch_dominates_is_not_accepted(self):
        # 400 batches agree to 1 %, one carries a heavy history: the error is 0.5 %, well under the target, but
        # the variance of the variance is near 1, so the run goes on.
        values = [1.0 + 0.01 * (-1) ** i for i in range(399)] + [3.0]
        stats = stats_of(values)
        self.assertLess(stats.relative_error, SAMPLING["target_relative_error"])
        self.assertGreater(stats.variance_of_variance, 0.5)
        self.assertIsNone(DT.stop_reason(stats, {**SAMPLING, "max_batches": 1000}, 1.0))
        settled = stats_of([1.0 + 0.01 * (-1) ** i for i in range(400)])
        self.assertLess(settled.variance_of_variance, DT.VOV_LIMIT)
        self.assertEqual(DT.stop_reason(settled, SAMPLING, 1.0), "target_reached")
        why, next_step = DT.explain("history_limit", stats, SAMPLING)
        self.assertIn("variance of the variance", why)
        self.assertIn("heavy history", why)
        self.assertIn("weight windows", next_step)

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
            (("variance_reduction",), "magic"),
            (("sampling", "target_relative_error"), 0.0),
            (("sampling", "min_batches"), 1),
            (("sampling", "threads"), 64),
            (("response", "solid_ids"), []),
            (("response", "solid_ids"), ["missing"]),
            (("response", "solid_ids"), ["plasma"]),
            (("chamber_solid_id",), "missing"),
            (("solids", 1, "volume_id"), 1),
            (("solids", 1, "material_id"), "unknown"),
            (("materials", 0, "density_kg_m3"), 0.0),
            (("materials", 0, "material_id"), DT.FILLER),
        ]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                DT.validate_request(changed(path, value))


class RetagTest(unittest.TestCase):
    def test_solids_take_their_material_and_empty_space_is_void(self):
        names = DT.retag_map(REQUEST["solids"], random_ray=False)
        self.assertEqual(names, {b"mat:plasma": b"mat:void", b"mat:coil-a": b"mat:tf-coil", b"mat:coil-b": b"mat:tf-coil"})

    def test_random_ray_fills_empty_space_and_the_outside(self):
        names = DT.retag_map(REQUEST["solids"], random_ray=True)
        self.assertEqual(names[b"mat:plasma"], f"mat:{DT.FILLER}".encode())
        self.assertEqual(names[b"mat:void_comp"], f"mat:{DT.FILLER}_comp".encode())

    def test_only_exact_names_are_rewritten(self):
        values = [b"mat:plasma".ljust(32, b"\0"), b"mat:plasma-facing".ljust(32, b"\0"), b"picked".ljust(32, b"\0")]
        new, hits = DT.retagged(values, {b"mat:plasma": b"mat:void"}, 32)
        self.assertEqual(hits, 1)
        self.assertEqual(new[0], b"mat:void".ljust(32, b"\0"))
        self.assertEqual(new[1:], values[1:])

    def test_a_tag_too_long_for_the_file_is_refused(self):
        with self.assertRaises(ValueError):
            DT.retagged([b"mat:a".ljust(8, b"\0")], {b"mat:a": b"mat:far-too-long"}, 8)


class WindowHelpersTest(unittest.TestCase):
    def test_library_entries_tolerate_rewritten_characters(self):
        self.assertEqual(DT.library_entries(["tf-coil", "flibe"], ["flibe", "tf_coil", "other"]),
                         {"tf-coil": "tf_coil", "flibe": "flibe"})
        with self.assertRaises(ValueError):
            DT.library_entries(["tf-coil"], ["flibe"])
        with self.assertRaises(ValueError):
            DT.library_entries(["tf-coil"], ["tf_coil", "tf.coil"])

    def test_group_edges_gain_a_threshold_that_is_not_an_edge(self):
        self.assertEqual(DT.group_edges(1.0e5), DT.GROUP_EDGES_EV)
        edges = DT.group_edges(2.0e5)
        self.assertEqual(len(edges), len(DT.GROUP_EDGES_EV) + 1)
        self.assertEqual(edges, sorted(edges))
        self.assertIn(2.0e5, edges)

    def test_response_lines_cover_each_group_above_the_threshold(self):
        lines = DT.response_lines(DT.group_edges(1.0e5), 1.0e5)
        self.assertEqual(len(lines), 10)
        self.assertAlmostEqual(lines[0], (1.0e5 * 5.0e5) ** 0.5)
        everything = DT.response_lines(DT.group_edges(0.0), 0.0)
        self.assertEqual(len(everything), 17)
        self.assertAlmostEqual(everything[0], 0.029)
        self.assertTrue(all(a < b for a, b in zip(everything, everything[1:])))

    def test_a_kept_aimed_plan_brings_its_capped_windows_file_and_the_runs_model(self):
        import json
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            kept = work / "aim-kept"
            kept.mkdir()
            (kept / "plan.json").write_text(json.dumps({"mode": "aimed", "library_supported": True, "env": {}}))
            model = work / "solver" / "model.xml"
            model.parent.mkdir()
            model.write_text("<model>this run</model>")
            # a plan kept without its windows file cannot be used
            self.assertIsNone(DT.reuse_aim(model, work, True))
            (kept / DT.CAPPED_WINDOWS).write_bytes(b"capped windows")
            plan = DT.reuse_aim(model, work, True)
            self.assertTrue(plan["reused"])
            self.assertEqual(Path(plan["model"]).read_text(), "<model>this run</model>")
            self.assertEqual(Path(plan["windows"]).read_bytes(), b"capped windows")
            self.assertEqual(Path(plan["windows"]), work / "aim" / DT.CAPPED_WINDOWS)
            # another kind of OpenMC build does not reuse it
            self.assertIsNone(DT.reuse_aim(model, work, False))

    def test_the_window_seeds_are_reserved(self):
        import copy as c
        request = c.deepcopy(REQUEST)
        request["sampling"]["seed"] = DT.WINDOW_SEED
        with self.assertRaises(ValueError):
            DT.validate_request(request)

    # Verifies: NUC-053
    def test_generation_and_reported_run_seeds_are_disjoint(self):
        import copy as c
        import re
        self.assertNotEqual(DT.MGXS_SEED, DT.WINDOW_SEED)
        for reserved in (DT.MGXS_SEED, DT.WINDOW_SEED):
            request = c.deepcopy(REQUEST)
            request["sampling"]["seed"] = reserved
            with self.assertRaises(ValueError):
                DT.validate_request(request)
        request = c.deepcopy(REQUEST)
        self.assertNotIn(request["sampling"]["seed"], (DT.MGXS_SEED, DT.WINDOW_SEED))
        DT.validate_request(request)
        # The only seeds the worker sets are the two reserved ones and the request's own, so no
        # step can draw its histories from another step's stream.
        import window_generator
        source = Path(DT.__file__).read_text() + Path(window_generator.__file__).read_text()
        assigned = re.findall(r"\.seed = ([^\n]+)", source)
        self.assertEqual(sorted(assigned), sorted(["MGXS_SEED", "WINDOW_SEED", 'int(sampling["seed"])']))

    def test_window_grid_reproduces_the_research_mesh_on_the_reference_model(self):
        lower, upper, dimension = DT.window_grid([-645.0, -645.0, -381.0], [645.0, 645.0, 381.0])
        self.assertEqual((lower, upper), ([-650.0, -650.0, -386.0], [650.0, 650.0, 386.0]))
        self.assertEqual(dimension, [52, 52, 31])

    def test_a_stopped_windowed_run_does_not_suggest_windows(self):
        _, next_step = DT.explain("time_limit", stats_of([0.0, 10.0] * 10), SAMPLING, windows=True)
        self.assertNotIn("weight windows", next_step)


class LostParticleTest(unittest.TestCase):
    # Verifies: GEO-025
    def test_more_than_one_lost_in_a_million_fails_once_the_run_can_tell(self):
        self.assertFalse(DT.lost_too_many(0, 5_000_000))
        self.assertFalse(DT.lost_too_many(None, 5_000_000))
        self.assertFalse(DT.lost_too_many(5, 5_000_000))
        self.assertTrue(DT.lost_too_many(6, 5_000_000))
        # One loss in a short run is reported but not judged.
        self.assertFalse(DT.lost_too_many(1, 600_000))
        self.assertTrue(DT.lost_too_many(2, 1_000_000))


class PeakTest(unittest.TestCase):
    def test_ring_edges_cover_the_range_in_whole_steps(self):
        self.assertEqual(DT.ring_edges(0.0, 12.0, 5.0), [0.0, 5.0, 10.0, 15.0])
        self.assertEqual(DT.ring_edges(-10.0, 10.0, 10.0), [-10.0, 0.0, 10.0])
        self.assertEqual(DT.ring_edges(-3.0, 7.0, None), [-3.0, 7.0])

    def test_the_peak_is_the_highest_resolved_track_length_per_volume(self):
        # Three pieces over four batches: the second has less track but a smaller volume.
        peak = DT.peak_of([40.0, 24.0, 8.0], [400.5, 144.3, 16.1], [1000.0, 400.0, 1000.0], [True] * 3, 4)
        self.assertEqual(peak["index"], 1)
        self.assertAlmostEqual(peak["mean"], 6.0)
        self.assertAlmostEqual(peak["volume_cm3"], 400.0)
        # Batch values with mean 6 and mean square 36.075: variance of the mean 0.075 / 3.
        self.assertAlmostEqual(peak["standard_error"], 0.025 ** 0.5)

    def test_a_piece_one_rare_score_landed_in_does_not_lead(self):
        # Piece 0: 100 in one batch of four (mean 25, error 25). Piece 1: a steady 10 with a small error.
        peak = DT.peak_of([100.0, 40.0], [10000.0, 400.4], [100.0, 100.0], [True, True], 4)
        self.assertEqual(peak["index"], 1)
        # With nothing resolved above zero, the highest mean leads.
        peak = DT.peak_of([100.0, 40.0], [10000.0, 1600.0], [100.0, 100.0], [True, True], 4)
        self.assertEqual(peak["index"], 0)

    def test_unranked_pieces_are_skipped(self):
        peak = DT.peak_of([10.0, 9.0], [50.1, 40.6], [1000.0, 10.0], [True, False], 2)
        self.assertEqual(peak["index"], 0)
        self.assertIsNone(DT.peak_of([], [], [], [], 2))
        self.assertIsNone(DT.peak_of([1.0], [1.0], [5.0], [False], 2))

    def test_a_wanted_peak_holds_the_run_until_it_meets_the_target(self):
        stats = stats_of([10.0] * 5 + [10.2] * 5)
        self.assertEqual(DT.stop_reason(stats, SAMPLING, 1.0), "target_reached")
        # The piece is held to twice the 5 % target on the mean.
        self.assertIsNone(DT.stop_reason(stats, SAMPLING, 1.0, (0.2,)))
        self.assertIsNone(DT.stop_reason(stats, SAMPLING, 1.0, (None,)))
        self.assertEqual(DT.stop_reason(stats, SAMPLING, 1.0, (0.1,)), "target_reached")
        why, more = DT.explain("time_limit", stats, SAMPLING, True, (0.2,))
        self.assertIn("20.0 % on the highest piece (held to 10 %)", why)
        self.assertIn("4.0 times", more)

    def test_a_peak_voxel_must_be_two_sizes_of_a_centimetre_or_more(self):
        request = copy.deepcopy(REQUEST)
        request["response"]["peak_voxel_cm"] = [5.0, None]
        DT.validate_request(request)
        for bad in ([5.0], [0.1, 10.0], [5.0, 10.0, 1.0]):
            request["response"]["peak_voxel_cm"] = bad
            with self.assertRaises(ValueError):
                DT.validate_request(request)


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
