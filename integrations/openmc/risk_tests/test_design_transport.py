# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for ../design_transport.py: the pure helpers. Nothing here runs OpenMC."""

import copy
import json
import os
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

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

    # Verifies: GEO-025
    def test_lost_particle_guidance_does_not_mislabel_birth_positions_as_loss_locations(self):
        why, next_step = DT.lost_particle_guidance(12, 1_000_000)
        self.assertIn("12 of 1000000", why)
        self.assertIn("gaps or overlaps", next_step)
        self.assertIn("birth points", next_step)
        self.assertIn("not where particles left the geometry", next_step)

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

    def test_heating_request_requires_explicit_winding_pack_and_all_energy(self):
        request = copy.deepcopy(REQUEST)
        request["solids"][1]["classification"] = "winding_pack"
        request["response"] = {"solid_ids": ["coil-a"], "energy_min_ev": 0.0, "peak_voxel_cm": [5.0, 5.0]}
        DT.validate_heating_request(request)
        request["response"]["energy_min_ev"] = 1.0e5
        with self.assertRaisesRegex(ValueError, "all neutron energies"):
            DT.validate_heating_request(request)
        request["response"]["energy_min_ev"] = 0.0
        request["solids"][1]["classification"] = None
        with self.assertRaisesRegex(ValueError, "explicit winding-pack"):
            DT.validate_heating_request(request)

    def test_window_grid_reproduces_the_research_mesh_on_the_reference_model(self):
        lower, upper, dimension = DT.window_grid([-645.0, -645.0, -381.0], [645.0, 645.0, 381.0])
        self.assertEqual((lower, upper), ([-650.0, -650.0, -386.0], [650.0, 650.0, 386.0]))
        self.assertEqual(dimension, [52, 52, 31])

    def test_a_stopped_windowed_run_does_not_suggest_windows(self):
        _, next_step = DT.explain("time_limit", stats_of([0.0, 10.0] * 10), SAMPLING, windows=True)
        self.assertNotIn("weight windows", next_step)


class LostParticleTest(unittest.TestCase):
    def run_mocked_phase_run(self, root, final_status=DT.final_run_status):
        """Exercise phase_run's real post-batch result assembly against a tiny fake OpenMC API."""
        root = Path(root)
        work = root / "work"
        work.mkdir()
        h5m = root / "model.h5m"
        h5m.write_bytes(b"test geometry identity")

        class Vector(list):
            def __sub__(self, other):
                return Vector(a - b for a, b in zip(self, other))

            def sum(self):
                return sum(self)

        class NumpyFake(ModuleType):
            @staticmethod
            def zeros(length):
                return Vector([0.0] * length)

            @staticmethod
            def array(values, dtype=None):
                return Vector(values)

        class TallyResults:
            def __getitem__(self, key):
                if key != (slice(None), 0, 1):
                    raise AssertionError(f"unexpected fake tally selection: {key!r}")
                return Vector([1.0])

        class SourceList(list):
            def ravel(self):
                return self

        class FakeModel:
            def __init__(self):
                self.settings = SimpleNamespace()

            def export_to_model_xml(self, path):
                Path(path).write_text("<model/>\n")

        model = FakeModel()
        tally = SimpleNamespace(results=TallyResults())
        lib = ModuleType("openmc.lib")
        lib._dll = object()
        lib.tallies = {DT.TALLY_ID: tally}
        lib.init = lambda *args: None
        lib.simulation_init = lambda: None
        lib.next_batch = lambda: None
        lib.simulation_finalize = lambda: None
        lib.finalize = lambda: None
        openmc = ModuleType("openmc")
        openmc.__path__ = []
        openmc.__version__ = "mock-openmc"
        openmc.lib = lib
        openmc.Tally = lambda tally_id, name: SimpleNamespace(
            tally_id=tally_id, name=name, filters=[], scores=[], estimator=None
        )
        openmc.CellFilter = lambda cells: ("cell", cells)
        openmc.EnergyFilter = lambda energies: ("energy", energies)
        openmc.Tallies = lambda values: values
        plasma = ModuleType("openmc_plasma_source")
        plasma.tokamak_source = lambda **kwargs: SimpleNamespace(
            strength=1.0, sources=SourceList([SimpleNamespace(strength=1.0)])
        )

        def geometry_copy(_request, target, random_ray=False):
            target = Path(target)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"copied geometry")
            return target

        request = {
            "h5m": str(h5m),
            "plasma": {},
            "response": {"solid_ids": ["coil"], "energy_min_ev": 1.0e5},
            "sampling": {
                "target_relative_error": 0.05,
                "particles_per_batch": 600_000,
                "min_batches": 1,
                "max_batches": 1,
                "max_seconds": 10.0,
                "seed": 7,
                "threads": 1,
            },
            "variance_reduction": "none",
        }
        with (
            patch.dict(sys.modules, {
                "numpy": NumpyFake("numpy"),
                "openmc": openmc,
                "openmc.lib": lib,
                "openmc_plasma_source": plasma,
            }),
            patch.object(DT, "geometry_copy", side_effect=geometry_copy),
            patch.object(DT, "build_model", return_value=(model, None, {})),
            patch.object(DT, "tokamak_arguments", return_value={}),
            patch.object(DT, "volume_ids", return_value={"coil": 1}),
            patch.object(DT, "stop_reason", return_value="target_reached"),
            patch.object(DT, "lost_count", return_value=1),
            patch.object(DT, "lost_positions", return_value=[[1.0, 2.0, 3.0]]),
            patch.object(DT, "final_run_status", side_effect=final_status),
        ):
            return DT.phase_run(request, work)

    # Verifies: GEO-025
    def test_more_than_one_lost_in_a_million_fails_once_the_run_can_tell(self):
        self.assertFalse(DT.lost_too_many(0, 5_000_000))
        self.assertFalse(DT.lost_too_many(None, 5_000_000))
        self.assertFalse(DT.lost_too_many(5, 5_000_000))
        self.assertTrue(DT.lost_too_many(6, 5_000_000))
        # During warmup, one loss in a short run does not interrupt sampling.
        self.assertFalse(DT.lost_too_many(1, 600_000))
        self.assertTrue(DT.lost_too_many(2, 1_000_000))

    # Verifies: GEO-025
    def test_final_guard_rejects_a_short_target_or_budget_stop_above_the_final_fraction(self):
        # A short run may continue through warmup, but it may not be accepted after it stops.
        self.assertFalse(DT.lost_too_many(1, 600_000))
        reason, why, next_step = DT.final_run_status("target_reached", 1, 600_000)
        self.assertEqual(reason, "lost_particles")
        self.assertIn("1 of 600000", why)
        self.assertIn("birth points", next_step)
        self.assertIn("not where particles left the geometry", next_step)
        self.assertEqual(DT.final_run_status("history_limit", 2, 600_000)[0], "lost_particles")
        self.assertEqual(DT.final_run_status("time_limit", None, 600_000)[0], "time_limit")

    # Verifies: GEO-025
    def test_final_guard_uses_strictly_greater_than_the_one_in_a_million_limit(self):
        self.assertEqual(DT.final_run_status("target_reached", 1, 1_000_000)[0], "target_reached")
        self.assertEqual(DT.final_run_status("target_reached", 2, 1_000_000)[0], "lost_particles")
        self.assertEqual(DT.final_run_status("target_reached", 1, 1_000_001)[0], "target_reached")

    # Verifies: GEO-025
    def test_phase_run_finalization_overrides_a_short_target_stop_and_preserves_diagnostics(self):
        with tempfile.TemporaryDirectory() as before_root, tempfile.TemporaryDirectory() as after_root:
            # This bypass models the original finalization path, which trusted the sampling stop reason.
            before = self.run_mocked_phase_run(
                before_root, final_status=lambda reason, lost, histories: (reason, None, None)
            )
            after = self.run_mocked_phase_run(after_root)

        self.assertEqual(before["stop_reason"], "target_reached")
        self.assertTrue(before["target_reached"])
        self.assertEqual(after["stop_reason"], "lost_particles")
        self.assertFalse(after["target_reached"])
        self.assertEqual(after["lost_particles"], 1)
        self.assertEqual(after["lost_positions_cm"], [[1.0, 2.0, 3.0]])
        self.assertIn("1 of 600000", after["why"])
        self.assertIn("birth points", after["next_step"])
        self.assertIn("not where particles left the geometry", after["next_step"])

        evidence_dir = os.environ.get("FARIS_GEO025_EVIDENCE_DIR")
        if evidence_dir:
            target = Path(evidence_dir) / "geo025-short-run-loss-guard.json"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({
                "setup": "mocked OpenMC C API; one batch of 600000 histories; one lost neutron",
                "before_original_finalization": {
                    "stop_reason": before["stop_reason"],
                    "target_reached": before["target_reached"],
                    "lost_particles": before["lost_particles"],
                    "lost_positions_cm": before["lost_positions_cm"],
                    "why": before["why"],
                    "next_step": before["next_step"],
                },
                "after_final_fraction_guard": {
                    "stop_reason": after["stop_reason"],
                    "target_reached": after["target_reached"],
                    "lost_particles": after["lost_particles"],
                    "lost_positions_cm": after["lost_positions_cm"],
                    "why": after["why"],
                    "next_step": after["next_step"],
                },
            }, indent=2) + "\n")


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

    def test_coupled_heating_closure_uses_correlated_batch_residuals(self):
        total = [11.0, 9.0, 13.0, 7.0]
        components = [[8.0, 6.0, 10.0, 4.0], [1.0, 1.0, 1.0, 1.0],
                      [1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]]
        mean, se = DT.heating_batch_closure(total, components)
        # Each individual tally fluctuates, but their aligned per-batch residual is constant.
        self.assertEqual(mean, 1.0)
        self.assertEqual(se, 0.0)

    def test_coupled_heating_closure_rejects_misaligned_batches(self):
        with self.assertRaisesRegex(ValueError, "aligned batches"):
            DT.heating_batch_closure([1.0, 2.0], [[1.0]])

    def test_three_se_gate_accepts_only_noise_or_explicit_roundoff(self):
        tolerance = DT.heating_closure_quality(0.0, 0.0, 10.0)
        self.assertTrue(tolerance["within_3se"])
        self.assertGreater(tolerance["roundoff_tolerance_ev_per_source"], 0.0)
        self.assertFalse(DT.heating_closure_quality(1.0, 0.0, 10.0)["within_3se"])
        self.assertTrue(DT.heating_closure_quality(0.3, 0.1, 10.0)["within_3se"])

    def test_terminal_quality_checks_status_and_lost_count_even_for_short_runs(self):
        self.assertEqual(DT.heating_terminal_quality(1, 100, 100, "history_limit", 0, 100_000)[0], "CALCULATED")
        self.assertEqual(DT.heating_terminal_quality(1, 99, 100, "history_limit", 0, 99_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(0, 30, 100, "time_limit", 0, 30_000)[0], "CALCULATED")
        self.assertEqual(DT.heating_terminal_quality(0, 29, 100, "time_limit", 0, 29_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(0, 30, 100, "history_limit", 0, 30_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(2, 100, 100, "history_limit", 0, 100_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(99, 100, 100, "history_limit", 0, 100_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(1, 100, 100, "history_limit", None, 100_000)[0], "NOT_EVALUATED")
        self.assertEqual(DT.heating_terminal_quality(1, 100, 100, "history_limit", 1, 100)[0], "INCONCLUSIVE")
        self.assertEqual(DT.heating_terminal_quality(1, 1_000_000, 1_000_000, "history_limit", 1, 1_000_000)[0], "CALCULATED")
        self.assertEqual(DT.heating_terminal_quality(1, 1_000_000, 1_000_000, "history_limit", 2, 1_000_000)[0], "INCONCLUSIVE")

    def test_fake_c_api_exercises_actual_tally_batch_differencing(self):
        import numpy as np
        from types import SimpleNamespace

        class FakeLib:
            def __init__(self):
                self.batch = 0
                self.tallies = {}

            def next_batch(self):
                self.batch += 1
                for item in self.tallies.values():
                    item.results[:, 0, 1] += item.increments
                return 1 if self.batch == 30 else 0

        lib = FakeLib()
        components = ("neutron", "photon", "electron", "positron", "total")
        tally_refs, voxel_refs = {}, {}
        for i, name in enumerate(components):
            tid, vid = i + 1, i + 11
            # Two voxel bins for the peak-volume basis. Cumulative values advance each batch.
            component_value = 8.0 if name == "neutron" else (1.0 if name in ("photon", "electron") else 0.0)
            lib.tallies[tid] = SimpleNamespace(results=np.zeros((1, 1, 2)), increments=np.array([component_value]))
            voxel_values = [component_value * 0.75, component_value * 0.25]
            lib.tallies[vid] = SimpleNamespace(results=np.zeros((2, 1, 2)), increments=np.array(voxel_values))
            tally_refs[name], voxel_refs[name] = SimpleNamespace(id=tid), SimpleNamespace(id=vid)
        # Provide nonzero, fluctuating neutron tallies while preserving the same per-batch total.
        neutron = lib.tallies[tally_refs["neutron"].id]
        total = lib.tallies[tally_refs["total"].id]
        neutron.increments = np.array([8.0])
        total.increments = np.array([10.0])
        for lib_tally in (lib.tallies[voxel_refs["neutron"].id], lib.tallies[voxel_refs["total"].id]):
            lib_tally.increments = np.array([7.5, 2.5]) if lib_tally is lib.tallies[voxel_refs["total"].id] else np.array([6.0, 2.0])
        batch, voxel_batch, count, stop, status = DT.collect_heating_batches(
            lib, tally_refs, voxel_refs, 1, 2, 30, 1e9, 0.0)
        self.assertEqual((count, stop, status), (30, "history_limit", 1))
        self.assertEqual(batch["total"][0], [10.0] * 30)
        self.assertEqual(voxel_batch["total"][0][0].tolist(), [7.5, 2.5])
        self.assertEqual(DT.heating_batch_closure(batch["total"][0], [batch[n][0] for n in components[:-1]]), (0.0, 0.0))
        # Source normalization is a unit-strength convention; the worker never rescales tally results afterward.
        source_scores = [sum(batch[name][0]) / count for name in components]
        self.assertEqual(source_scores[-1], 10.0)
        self.assertEqual(DT.heating_closure_quality(0.0, 0.0, sum(source_scores)), {"within_3se": True,
                         "roundoff_tolerance_ev_per_source": DT.heating_closure_quality(0.0, 0.0, sum(source_scores))["roundoff_tolerance_ev_per_source"]})

    def test_heating_source_already_at_one_remains_one_source_per_history(self):
        from types import SimpleNamespace
        import numpy as np
        elements = [SimpleNamespace(strength=0.25), SimpleNamespace(strength=0.75)]
        source = SimpleNamespace(strength=1.0, sources=np.array(elements, dtype=object))
        DT.normalize_heating_source(source)
        self.assertEqual(source.strength, 1.0)
        self.assertEqual([element.strength for element in elements], [0.25, 0.75])

    def test_heating_tally_total_has_no_particle_filter_and_components_do(self):
        from types import SimpleNamespace
        class FakeOpenMC:
            @staticmethod
            def ParticleFilter(particles):
                return ("particle", particles)
            @staticmethod
            def MeshFilter(mesh):
                return ("mesh", mesh)
        cell, mesh = object(), object()
        self.assertEqual(DT.heating_tally_filters(FakeOpenMC, "total", cell), [cell])
        self.assertEqual(DT.heating_tally_filters(FakeOpenMC, "photon", cell), [cell, ("particle", ["photon"])])
        self.assertEqual(DT.heating_tally_filters(FakeOpenMC, "electron", cell, mesh),
                         [("mesh", mesh), cell, ("particle", ["electron"])])

    def test_fake_openmc_phase_writes_raw_report_for_engine_parser(self):
        import json
        import tempfile
        import types
        from unittest.mock import patch
        import numpy as np

        class FakeTally:
            def __init__(self, tally_id, name):
                self.id, self.name = tally_id, name

        class FakeLib(types.ModuleType):
            def __init__(self):
                super().__init__("openmc.lib")
                self.tallies, self.batch = {}, 0
                self._dll = object()
            def init(self, args):
                self.args = args
            def simulation_init(self):
                return None
            def next_batch(self):
                self.batch += 1
                for tally in self.tallies.values():
                    tally.results[:, 0, 1] += tally.increments
                return 1 if self.batch == 30 else 0
            def simulation_finalize(self):
                pass
            def finalize(self):
                pass

        lib = FakeLib()
        openmc = types.ModuleType("openmc")
        openmc.__version__, openmc.lib = "0.16.0", lib
        openmc.Tally = FakeTally
        openmc.Tallies = lambda values: list(values)
        openmc.CellFilter = lambda values: ("cells", values)
        openmc.ParticleFilter = lambda values: ("particle", values)
        openmc.MeshFilter = lambda mesh: ("mesh", mesh)
        openmc.CylindricalMesh = lambda **kwargs: ("cylindrical_mesh", kwargs)

        total_bias = {"value": 0.0}
        class FakeModel:
            def __init__(self):
                self.settings = types.SimpleNamespace()
            def export_to_model_xml(self, path):
                Path(path).write_text("fake model")
                increments = {"neutron": ([8.0], [6.0, 2.0]), "photon": ([1.0], [0.75, 0.25]),
                              "electron": ([1.0], [0.75, 0.25]), "positron": ([0.0], [0.0, 0.0]),
                              "total": ([10.0], [7.5, 2.5])}
                for tally in self.tallies:
                    name = tally.name.removeprefix("heating-voxel-").removeprefix("heating-")
                    is_voxel = "voxel" in tally.name
                    values = list(increments[name][1 if is_voxel else 0])
                    if name == "total" and not is_voxel:
                        values[0] += total_bias["value"]
                    lib.tallies[tally.id] = types.SimpleNamespace(
                        results=np.zeros((len(values), 1, 2)), increments=np.asarray(values))

        model = FakeModel()
        source_elements = [types.SimpleNamespace(strength=1.0)]
        source = types.SimpleNamespace(strength=1.0, sources=np.array(source_elements, dtype=object))
        plasma_source = types.ModuleType("openmc_plasma_source")
        plasma_source.tokamak_source = lambda **kwargs: source
        request = {
            "schema": DT.REQUEST_SCHEMA, "h5m": "model.h5m", "cross_sections": "cross_sections.xml",
            "solids": [{"solid_id": "coil", "volume_id": 2, "material_id": "steel",
                        "classification": "winding_pack"}],
            "materials": [], "plasma": {}, "response": {"solid_ids": ["coil"], "energy_min_ev": 0.0,
                                                               "peak_voxel_cm": [5.0, 5.0]},
            "sampling": {"particles_per_batch": 1000, "min_batches": 30, "max_batches": 30,
                         "max_seconds": 1000.0, "seed": 5, "threads": 1}}

        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            for name in ("model.h5m", "cross_sections.xml"):
                (work / name).write_text("identity")
            request["h5m"] = str(work / "model.h5m")
            request["cross_sections"] = str(work / "cross_sections.xml")
            (work / "solver").mkdir()
            (work / "peak").mkdir()
            (work / "solver" / "heating-preflight.json").write_text(json.dumps({
                "status": "READY", "data_identity": {"source": "fixture"},
                "cross_sections_sha256": "fixture-hash", "h5m_sha256": "fixture-hash"}))
            np.savez(work / "peak" / "volumes.npz", volumes_cm3=np.array([[[10000.0, 20000.0]]]),
                     r_edges_cm=np.array([100.0, 105.0, 110.0]), z_edges_cm=np.array([-5.0, 0.0]))
            (work / "peak" / "record.json").write_text(json.dumps({
                "volumes_sha256": "fixture-hash", "solid_ids": ["coil"], "voxel_cm": [5.0, 5.0], "rays": 1000}))
            fake_modules = {"openmc": openmc, "openmc.lib": lib, "openmc_plasma_source": plasma_source,
                            "h5py": types.ModuleType("h5py")}
            with patch.dict("sys.modules", fake_modules), \
                 patch.object(DT, "heating_data_identity", return_value={"source": "fixture"}), \
                 patch.object(DT, "sha256_file", return_value="fixture-hash"), \
                 patch.object(DT, "geometry_copy", side_effect=lambda request, target, random_ray: target), \
                 patch.object(DT, "build_model", return_value=(model, None, None)), \
                 patch.object(DT, "tokamak_arguments", return_value={}), \
                 patch.object(DT, "lost_count", return_value=0):
                raw = DT.phase_heating(request, work)
                stored = json.loads((work / "solver" / "heating-result.json").read_text())
                self.assertEqual(raw, stored)
                self.assertEqual(raw["status"], "CALCULATED")
                self.assertEqual(raw["normalization"], "per_source_neutron")
                self.assertEqual(raw["solids"][0]["total"]["mean_ev_per_source"], 10.0)
                self.assertEqual(raw["solids"][0]["peak"]["volume_cm3"], 10000.0)
                self.assertEqual(raw["solids"][0]["peak"]["closure_residual"]["within_3se"], True)
                self.assertEqual(source.strength, 1.0)
                total = next(t for t in model.tallies if t.name == "heating-total")
                photon = next(t for t in model.tallies if t.name == "heating-photon")
                self.assertNotIn("particle", str(total.filters))
                self.assertIn(("particle", ["photon"]), photon.filters)
                total_bias["value"] = 1.0
                lib.batch = 0
                inconclusive = DT.phase_heating(request, work)
                self.assertEqual(inconclusive["status"], "INCONCLUSIVE")
                self.assertEqual(inconclusive["solids"][0]["closure_residual"]["mean_ev_per_source"], 1.0)
                self.assertFalse(inconclusive["solids"][0]["closure_residual"]["within_3se"])

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
