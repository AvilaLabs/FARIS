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

    def test_window_block_is_replaced_whole(self):
        xml = '<model><settings><weight_windows id="1"><a>1</a></weight_windows><x/></settings></model>'
        block = DT.window_block(xml)
        self.assertEqual(block, '<weight_windows id="1"><a>1</a></weight_windows>')
        new = DT.with_window_block(xml, '<weight_windows id="1"><a>2</a></weight_windows>')
        self.assertEqual(new, xml.replace("<a>1</a>", "<a>2</a>"))

    def test_the_window_seeds_are_reserved(self):
        import copy as c
        request = c.deepcopy(REQUEST)
        request["sampling"]["seed"] = DT.WINDOW_SEED
        with self.assertRaises(ValueError):
            DT.validate_request(request)

    def test_window_grid_reproduces_the_research_mesh_on_the_reference_model(self):
        lower, upper, dimension = DT.window_grid([-645.0, -645.0, -381.0], [645.0, 645.0, 381.0])
        self.assertEqual((lower, upper), ([-650.0, -650.0, -386.0], [650.0, 650.0, 386.0]))
        self.assertEqual(dimension, [52, 52, 31])

    def test_a_stopped_windowed_run_does_not_suggest_windows(self):
        _, next_step = DT.explain("time_limit", stats_of([0.0, 10.0] * 10), SAMPLING, windows=True)
        self.assertNotIn("weight windows", next_step)


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
