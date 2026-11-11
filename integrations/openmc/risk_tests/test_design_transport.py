# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for ../design_transport.py: the pure helpers. Nothing here runs OpenMC."""

import copy
import json
import math
import os
from pathlib import Path
import subprocess
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
    def test_tbr_target_is_split_but_mapped_flux_keeps_aggregate_target(self):
        import numpy as np

        class Moments:
            count = 30
            relative_errors = np.array([0.009, 0.009, 0.04, 0.049])

        split = {**SAMPLING, "h3_target_relative_error": 0.01}
        self.assertTrue(DT.history_targets_reached(Moments(), split))
        Moments.relative_errors = np.array([0.011, 0.009, 0.01, 0.01])
        self.assertFalse(DT.history_targets_reached(Moments(), split))
        Moments.relative_errors = np.array([0.009, 0.009, 0.051, 0.01])
        self.assertFalse(DT.history_targets_reached(Moments(), split))
        # Old requests apply their one shared target to H3 and component flux alike.
        legacy = {**SAMPLING, "target_relative_error": 0.05}
        Moments.relative_errors = np.array([0.04, 0.04, 0.04, 0.04])
        self.assertTrue(DT.history_targets_reached(Moments(), legacy))

    def test_h3_shortfall_does_not_mask_history_or_time_budget(self):
        settled = stats_of([1.0] * SAMPLING["min_batches"])
        self.assertIsNone(DT.stop_reason(settled, SAMPLING, 1.0, history_target_met=False))
        history_bounded = {**SAMPLING, "max_batches": SAMPLING["min_batches"]}
        self.assertEqual(
            DT.stop_reason(settled, history_bounded, 1.0, history_target_met=False),
            "history_limit",
        )
        self.assertEqual(
            DT.stop_reason(settled, SAMPLING, SAMPLING["max_seconds"] + 1,
                           history_target_met=False),
            "time_limit",
        )

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

    # Verifies: NUC-056
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

    def test_heating_preflight_imports_openmc_lib_in_fresh_interpreter(self):
        python = os.environ.get("FARIS_OPENMC_PYTHON", sys.executable)
        script = '''
import pathlib, sys
sys.path.insert(0, sys.argv[1])
try:
    import openmc
except ModuleNotFoundError as exc:
    if exc.name == "openmc":
        raise SystemExit(77)
    raise
import design_transport as worker
root = pathlib.Path(sys.argv[2])
(root / "model.h5m").write_bytes(b"geometry")
(root / "cross_sections.xml").write_text("<cross_sections />")
def stop_after_import(_request):
    assert hasattr(openmc, "lib"), "phase preflight did not initialize openmc.lib"
    raise ValueError("regression sentinel")
worker.validate_heating_request = stop_after_import
record = worker.phase_heating_preflight({"h5m": str(root / "model.h5m"),
                                         "cross_sections": str(root / "cross_sections.xml")}, root)
assert record["status"] == "NOT_EVALUATED", record
assert record["why"] == "regression sentinel", record
'''
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run([python, "-c", script,
                                     str(Path(DT.__file__).resolve().parent), tmp],
                                    check=False, capture_output=True, text=True, timeout=10)
            if result.returncode == 77:
                self.skipTest("selected Python does not have OpenMC installed")
            self.assertEqual(result.returncode, 0, result.stderr)

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
        # phase_run records the loaded C API library path and bytes; this pure fake
        # points at an existing file only to exercise result serialization.
        lib._dll = SimpleNamespace(_name=str(Path(DT.__file__).resolve()))
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
    def test_openmc_material_volume_capacity_uses_all_geometry_materials(self):
        elements = 100_000
        selected_count = 18
        geometry_material_count = 41  # 40 stand-ins plus the OpenMC void entry
        selected_bytes, _ = DT.volume_material_allocation(elements, selected_count)
        geometry_bytes, slots = DT.volume_material_allocation(elements, geometry_material_count)
        self.assertEqual(slots, 2 * geometry_material_count)
        self.assertEqual(geometry_bytes, elements * slots * 12)
        self.assertGreater(geometry_bytes, selected_bytes)
        self.assertIn("estimated", DT.volume_material_allocation_not_evaluated(
            1_000_000, geometry_material_count))
        self.assertIn("elements", DT.volume_material_allocation_not_evaluated(
            DT.MAX_VOLUME_MATERIAL_CELLS + 1, 1))

    def test_build_model_sphere_contains_mixed_sign_extents(self):
        captured = {}

        class FakeSphere:
            def __neg__(self):
                return self

        class FakeMaterials(list):
            pass

        class FakeMaterial:
            def __init__(self, material_id, name):
                self.id, self.name = material_id, name
            def add_nuclide(self, *args, **kwargs):
                pass
            def set_density(self, *args):
                pass

        class FakeModel:
            def __init__(self, geometry, materials):
                self.geometry, self.materials = geometry, materials
                self.settings = SimpleNamespace()

        openmc = SimpleNamespace(
            Material=FakeMaterial,
            Materials=FakeMaterials,
            DAGMCUniverse=lambda _path: SimpleNamespace(bounding_box=SimpleNamespace(
                lower_left=(-10.0, -1.0, -1.0), upper_right=(1.0, 9.0, 1.0))),
            Sphere=lambda **kwargs: (captured.update(kwargs) or FakeSphere()),
            Cell=lambda **kwargs: kwargs,
            Geometry=lambda cells: cells,
            Model=FakeModel,
        )
        _, _, _ = DT.build_model(openmc, {"materials": [], "cross_sections": "xs.xml"}, Path("model.h5m"))
        self.assertAlmostEqual(captured["r"], 1.05 * math.sqrt(10.0**2 + 9.0**2 + 1.0**2))

    def test_volume_api_counts_all_standins_and_keeps_shared_material_solids_separate(self):
        import numpy as np

        created_materials = []
        call = {}
        standin_count = 4

        class FakeMaterial:
            def __init__(self, material_id, name):
                self.id, self.name = material_id, name
                created_materials.append((material_id, name))
            def add_nuclide(self, *args, **kwargs):
                pass
            def set_density(self, *args):
                pass

        class FakeSurface:
            boundary_type = None
            def __pos__(self):
                return self
            def __neg__(self):
                return self
            def __and__(self, other):
                return self

        class FakeMesh:
            def __init__(self, **kwargs):
                self.kwargs = kwargs
            def material_volumes(self, model, *, n_samples, max_materials, output, args):
                call.update(n_samples=n_samples, max_materials=max_materials, output=output, args=args)
                # The real API retries with a larger hash table if its initial estimate is low.
                return {number: np.full(8, float(number)) for number in range(1, standin_count + 1)}

        class FakeMaterials(list):
            pass

        class FakeModel:
            def __init__(self, geometry, materials):
                self.settings = SimpleNamespace()
                self.materials = materials

        openmc = SimpleNamespace(
            __version__="0.16.0",
            Material=FakeMaterial,
            Materials=FakeMaterials,
            DAGMCUniverse=lambda _path: SimpleNamespace(bounding_box=SimpleNamespace(
                lower_left=(-1.0, -1.0, -2.0), upper_right=(1.0, 1.0, 2.0))),
            XPlane=lambda *args, **kwargs: FakeSurface(),
            YPlane=lambda *args, **kwargs: FakeSurface(),
            ZPlane=lambda *args, **kwargs: FakeSurface(),
            Geometry=lambda cells: cells,
            Cell=lambda **kwargs: kwargs,
            Model=FakeModel,
            IndependentSource=lambda: object(),
            CylindricalMesh=FakeMesh,
        )
        solids = [
            {"solid_id": "coil-a", "material_id": "copper"},
            {"solid_id": "coil-b", "material_id": "copper"},
            {"solid_id": "case", "material_id": "steel"},
            {"solid_id": "shield", "material_id": "steel"},
        ]
        request = {
            "heating_mode": True,
            "h5m": "model.h5m",
            "cross_sections": "xs.xml",
            "solids": solids,
            "response": {"solid_ids": ["coil-a", "coil-b"], "peak_voxel_cm": [1.0, 1.0]},
            "sampling": {"threads": 1},
        }
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            h5m = work / "model.h5m"
            h5m.write_bytes(b"stable geometry")
            request["h5m"] = str(h5m)
            with patch.dict("sys.modules", {"openmc": openmc}), \
                 patch.object(DT, "checked_heating_h5m_snapshot",
                              return_value=(h5m, DT.sha256_file(h5m))):
                record = DT.phase_volumes(request, work)
            self.assertEqual(call["max_materials"], len(solids) + 1)
            self.assertEqual([name for _, name in created_materials], [s["solid_id"] for s in solids])
            self.assertEqual(record["solid_ids"], ["coil-a", "coil-b"])
            self.assertEqual(record["geometry_standin_material_count"], len(solids))
            self.assertEqual(record["material_volume_max_materials"], len(solids) + 1)
            self.assertIn("all per-solid stand-ins", record["material_volume_scope"].lower())
            self.assertEqual(record["h5m_sha256"], DT.sha256_file(h5m))
            with np.load(work / "peak" / "volumes.npz") as saved:
                self.assertEqual(saved["volumes_cm3"].shape[0], 2)
                self.assertTrue(np.all(saved["volumes_cm3"][0] == 1.0))
                self.assertTrue(np.all(saved["volumes_cm3"][1] == 2.0))

    def test_scalar_online_moments_match_direct_vov_formula_without_retaining_batches(self):
        import math

        samples = [1.0, 2.0, 4.0, 5.0, 10.0]
        moments = DT.ScalarMoments()
        for value in samples:
            moments.add(value)
        mean = sum(samples) / len(samples)
        centered = [value - mean for value in samples]
        second = sum(value * value for value in centered)
        fourth = sum(value ** 4 for value in centered)
        self.assertAlmostEqual(moments.mean, mean)
        self.assertAlmostEqual(moments.standard_error, math.sqrt(second / (len(samples) * (len(samples) - 1))))
        self.assertAlmostEqual(moments.variance_of_variance, fourth / (second * second) - 1.0 / len(samples))
        self.assertFalse(hasattr(moments, "values"))

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

    def test_history_energy_covariance_serializes_flat_row_major_with_off_diagonals(self):
        import numpy as np

        matrix = np.arange(16, dtype=float).reshape(4, 4)
        encoded = DT.covariance_row_major(matrix)
        self.assertEqual(encoded, list(map(float, range(16))))
        self.assertEqual(encoded[1], matrix[0, 1])
        self.assertEqual(encoded[4], matrix[1, 0])
        with self.assertRaisesRegex(ValueError, "square matrix"):
            DT.covariance_row_major(np.zeros((3, 4)))

    def test_peak_quality_rejects_zero_and_relative_error_over_ten_percent(self):
        self.assertEqual(DT.heating_peak_quality(0.0, 0.0),
                         (False, "The sampled peak response is zero; relative heating uncertainty is unavailable."))
        self.assertEqual(DT.heating_peak_quality(1.0, None)[0], False)
        self.assertEqual(DT.heating_peak_quality(1.0, 0.1000001)[0], False)
        self.assertEqual(DT.heating_peak_quality(1.0, 0.1), (True, None))

    def test_heating_mesh_bound_is_checked_before_ray_sampling(self):
        bounds = SimpleNamespace(lower_left=(-1.0, -1.0, -2.0), upper_right=(1.0, 1.0, 2.0))
        # ceil(sqrt(2)) gives two radial bins at 1 cm and four z bins: 8 cells per solid.
        cells = DT.heating_mesh_cell_count(bounds, [1.0, 1.0], 4)
        self.assertEqual(cells, 32)
        self.assertIsNone(DT.heating_mesh_not_evaluated(cells))
        self.assertIn("1,000,001", DT.heating_mesh_not_evaluated(DT.MAX_HEATING_VOLUME_CELLS + 1))
        huge = SimpleNamespace(lower_left=(-1.0e12, -1.0e12, -1.0e12),
                               upper_right=(1.0e12, 1.0e12, 1.0e12))
        self.assertIsNotNone(DT.heating_mesh_not_evaluated(DT.heating_mesh_cell_count(huge, [1.0, 1.0], 1)))

    def test_heating_preflight_refuses_oversized_grid_before_volume_phase(self):
        import types

        class Settings:
            photon_transport = False
            atomic_relaxation = False
            electron_treatment = ""

        class OpenMC(types.ModuleType):
            __version__ = "0.16.0"

            @staticmethod
            def DAGMCUniverse(_path):
                bounds = SimpleNamespace(lower_left=(-1000.0, -1000.0, -1000.0),
                                         upper_right=(1000.0, 1000.0, 1000.0))
                return SimpleNamespace(bounding_box=bounds)

        OpenMC.Settings = Settings

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            h5m, cross_sections = root / "model.h5m", root / "cross_sections.xml"
            h5m.write_bytes(b"geometry")
            cross_sections.write_bytes(b"data")
            request = {
                "schema": DT.REQUEST_SCHEMA, "heating_mode": True,
                "h5m": str(h5m), "cross_sections": str(cross_sections),
                "solids": [{"solid_id": "coil-a", "classification": "winding_pack", "material_id": "steel"},
                           {"solid_id": "coil-b", "classification": "winding_pack", "material_id": "steel"}],
                "response": {"solid_ids": ["coil-a", "coil-b"], "energy_min_ev": 0.0,
                             "peak_voxel_cm": [1.0, 1.0]},
                "sampling": {"particles_per_batch": 1000, "min_batches": 30, "max_batches": 30,
                             "max_seconds": 60.0, "seed": 1, "threads": 1, "target_relative_error": 0.05},
            }
            fake_openmc = OpenMC("openmc")
            fake_openmc.__path__ = []
            fake_modules = {"openmc": fake_openmc, "openmc.lib": ModuleType("openmc.lib"),
                            "h5py": types.ModuleType("h5py")}
            with patch.dict("sys.modules", fake_modules):
                record = DT.phase_heating_preflight(request, root)
            self.assertEqual(record["status"], "NOT_EVALUATED")
            self.assertIn("above the bounded limit", record["why"])
            self.assertIn("voxel size", record["next_step"])

    def test_heating_preflight_bounds_material_volume_arrays_for_all_standins(self):
        class Settings:
            photon_transport = False
            atomic_relaxation = False
            electron_treatment = ""

        class OpenMC(ModuleType):
            __version__ = "0.16.0"

            @staticmethod
            def DAGMCUniverse(_path):
                bounds = SimpleNamespace(lower_left=(-750.0, 0.0, -500.0),
                                         upper_right=(750.0, 0.0, 500.0))
                return SimpleNamespace(bounding_box=bounds)

        OpenMC.Settings = Settings

        solids = [{"solid_id": f"solid-{index}", "classification": "winding_pack",
                   "material_id": f"material-{index}"} for index in range(30)]
        request = {
            "schema": DT.REQUEST_SCHEMA, "heating_mode": True,
            "solids": solids, "materials": [], "plasma": {}, "chamber_solid_id": "solid-0",
            "h5m": "model.h5m", "cross_sections": "cross_sections.xml",
            "response": {"solid_ids": ["solid-0"], "energy_min_ev": 0.0,
                         "peak_voxel_cm": [1.0, 1.0]},
            "sampling": {"target_relative_error": 0.05, "particles_per_batch": 1000,
                         "min_batches": 30, "max_batches": 30, "max_seconds": 60.0,
                         "seed": 1, "threads": 1},
        }
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            h5m, cross_sections = work / "model.h5m", work / "cross_sections.xml"
            h5m.write_bytes(b"geometry")
            cross_sections.write_bytes(b"data")
            request["h5m"], request["cross_sections"] = str(h5m), str(cross_sections)
            fake_openmc = OpenMC("openmc")
            fake_openmc.__path__ = []
            with patch.dict("sys.modules", {"openmc": fake_openmc, "openmc.lib": ModuleType("openmc.lib"),
                                            "h5py": ModuleType("h5py")}), \
                 patch.object(DT, "heating_runtime_identity", return_value={}), \
                 patch.object(DT, "heating_data_identity", side_effect=AssertionError("data audit ran too early")):
                record = DT.phase_heating_preflight(request, work)
        self.assertEqual(record["status"], "NOT_EVALUATED")
        self.assertIn("estimated", record["why"])
        self.assertIn("558,000,000 bytes", record["why"])

    def test_heating_history_and_time_bounds_match_native_controls(self):
        request = {"schema": DT.REQUEST_SCHEMA, "sampling": {
            "target_relative_error": 0.05, "particles_per_batch": 50_000, "min_batches": 30, "max_batches": 50_000,
            "max_seconds": 100.0, "threads": 1, "seed": 1},
            "response": {"energy_min_ev": 0.0, "peak_voxel_cm": [5.0, 5.0], "solid_ids": ["coil"]},
            "solids": [{"solid_id": "coil", "classification": "winding_pack", "material_id": "steel"}]}
        with self.assertRaisesRegex(ValueError, "planned source histories"):
            DT.validate_heating_request(request)
        request["sampling"].update(particles_per_batch=1, max_batches=DT.MAX_HEATING_BATCHES + 1,
                                   max_seconds=100.0)
        with self.assertRaisesRegex(ValueError, "batches"):
            DT.validate_heating_request(request)
        request["sampling"].update(max_batches=30, max_seconds=DT.MAX_HEATING_SECONDS + 1)
        with self.assertRaisesRegex(ValueError, "time limits"):
            DT.validate_heating_request(request)

    def test_history_energy_map_must_partition_material_bearing_solids_exactly(self):
        request = {"schema": DT.REQUEST_SCHEMA, "sampling": {
            "target_relative_error": 0.05, "particles_per_batch": 1000, "min_batches": 30,
            "max_batches": 30, "max_seconds": 100.0, "threads": 1, "seed": 1},
            "response": {"energy_min_ev": 0.0, "peak_voxel_cm": [5.0, 5.0], "solid_ids": ["coil"]},
            "solids": [{"solid_id": "coil", "classification": "winding_pack", "material_id": "steel"},
                       {"solid_id": "shield", "classification": "shield", "material_id": "steel"},
                       {"solid_id": "void", "material_id": None}],
            "history_energy_response": {"component_map": {"coils": ["coil"], "shield": ["shield"]},
                                        "source_definition_sha256": "a" * 64,
                                        "source_kernel_path": "/source.py", "source_kernel_sha256": "b" * 64}}
        DT.validate_heating_request(request)
        request["history_energy_response"]["component_map"] = {"coils": ["coil"]}
        with self.assertRaisesRegex(ValueError, "partition every material-bearing"):
            DT.validate_heating_request(request)
        request["history_energy_response"]["component_map"] = {"coils": ["coil", "coil"], "shield": ["shield"]}
        with self.assertRaisesRegex(ValueError, "repeats or names"):
            DT.validate_heating_request(request)

    def test_heating_target_error_must_be_finite_and_positive(self):
        request = {"schema": DT.REQUEST_SCHEMA, "sampling": {
            "target_relative_error": 0.05, "particles_per_batch": 1000, "min_batches": 30,
            "max_batches": 30, "max_seconds": 100.0, "threads": 1, "seed": 1},
            "response": {"energy_min_ev": 0.0, "peak_voxel_cm": [5.0, 5.0], "solid_ids": ["coil"]},
            "solids": [{"solid_id": "coil", "classification": "winding_pack", "material_id": "steel"}]}
        for target in (0.0, -0.01, 1.0, float("nan"), float("inf")):
            request["sampling"]["target_relative_error"] = target
            with self.assertRaisesRegex(ValueError, "target_relative_error"):
                DT.validate_heating_request(request)

    def test_checked_h5m_snapshot_rejects_transient_mutation_even_when_source_is_restored(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            source = work / "model.h5m"
            original, transient = b"checked geometry", b"transient altered geometry"
            source.write_bytes(original)
            (work / "solver").mkdir()
            (work / "solver" / "heating-preflight.json").write_text(json.dumps({
                "status": "READY", "h5m_sha256": DT.sha256_file(source)}))
            real_copy = DT.shutil.copyfile

            def mutate_during_copy(src, dst):
                Path(src).write_bytes(transient)
                result = real_copy(src, dst)
                Path(src).write_bytes(original)
                return result

            with patch.object(DT.shutil, "copyfile", side_effect=mutate_during_copy):
                with self.assertRaisesRegex(ValueError, "stable worker snapshot"):
                    DT.checked_heating_h5m_snapshot({"h5m": str(source)}, work, "volume")
            self.assertEqual(source.read_bytes(), original)

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
        batch, voxel_batch, closure, voxel_closure, _, _, count, stop, status = DT.collect_heating_batches(
            lib, tally_refs, voxel_refs, 1, 2, 30, 1e9, 0.0)
        self.assertEqual((count, stop, status), (30, "history_limit", 1))
        self.assertEqual(batch["total"].count, 30)
        self.assertEqual(batch["total"].mean.tolist(), [10.0])
        self.assertEqual(batch["total"].standard_error.tolist(), [0.0])
        self.assertEqual(voxel_batch["total"].mean[:, 0].tolist(), [7.5, 2.5])
        self.assertEqual(closure.mean.tolist(), [0.0])
        self.assertEqual(closure.standard_error.tolist(), [0.0])
        self.assertEqual(voxel_closure.mean[:, 0].tolist(), [0.0, 0.0])
        # Source normalization is a unit-strength convention; the worker never rescales tally results afterward.
        source_scores = [batch[name].mean[0] for name in components]
        self.assertEqual(source_scores[-1], 10.0)
        self.assertEqual(DT.heating_closure_quality(0.0, 0.0, sum(source_scores)), {"within_3se": True,
                         "roundoff_tolerance_ev_per_source": DT.heating_closure_quality(0.0, 0.0, sum(source_scores))["roundoff_tolerance_ev_per_source"]})

    def test_history_heating_uses_unfiltered_and_partitioned_same_batch_covariance(self):
        import numpy as np
        from types import SimpleNamespace

        class FakeLib:
            def __init__(self):
                self.batch = 0
                self.tallies = {}

            def next_batch(self):
                sign = 1.0 if self.batch % 2 == 0 else -1.0
                self.batch += 1
                for tally in self.tallies.values():
                    tally.results[:, 0, 1] += tally.pattern(sign)
                return 1 if self.batch == 30 else 0

        lib = FakeLib()
        components = ("neutron", "photon", "electron", "positron", "total")
        tally_refs, voxel_refs = {}, {}

        def add(pattern):
            tid = len(lib.tallies) + 1
            lib.tallies[tid] = SimpleNamespace(results=np.zeros((1, 1, 2)), pattern=pattern)
            return SimpleNamespace(id=tid)

        for name in components:
            weight = 1.0 if name in ("photon", "electron") else 0.0
            if name == "neutron":
                tally_pattern = lambda sign: np.array([8.0 + sign])
                voxel_pattern = lambda sign: np.array([6.75 + 0.75 * sign, 2.25 + 0.25 * sign])
            elif name == "total":
                tally_pattern = lambda sign: np.array([10.0 + sign])
                voxel_pattern = lambda sign: np.array([7.5 + 0.75 * sign, 2.5 + 0.25 * sign])
            else:
                tally_pattern = lambda sign, w=weight: np.array([w])
                voxel_pattern = lambda sign, w=weight: np.array([0.75 * w, 0.25 * w])
            tally_refs[name] = add(tally_pattern)
            vid = len(lib.tallies) + 1
            lib.tallies[vid] = SimpleNamespace(results=np.zeros((2, 1, 2)), pattern=voxel_pattern)
            voxel_refs[name] = SimpleNamespace(id=vid)

        whole = add(lambda sign: np.array([10.0 + sign]))
        blanket = add(lambda sign: np.array([4.0 + 0.4 * sign]))
        magnet = add(lambda sign: np.array([6.0 + 0.6 * sign]))
        history_refs = {"whole_model": whole, "components": {"blanket": blanket, "magnet": magnet},
                        "component_map": {"blanket": ["b"], "magnet": ["m"]},
                        "source_definition_sha256": "a" * 64,
                        "source_kernel_path": "/source.py", "source_kernel_sha256": "b" * 64}
        result = DT.collect_heating_batches(
            lib, tally_refs, voxel_refs, 1, 2, 30, 1e9, 0.0, 0.05,
            np.ones((1, 2)), np.ones((1, 2), dtype=bool), None, history_refs)
        self.assertEqual(result[6:9], (30, "target_reached", 1))
        energy = result[9]
        self.assertEqual(energy["status"], "CALCULATED")
        self.assertEqual(set(energy), set(json.loads(
            (Path(__file__).resolve().parents[3] / "crates" / "faris-engine" / "tests" / "fixtures"
             / "heating-history-response.json").read_text())))
        self.assertIn("closure_residual", energy)
        self.assertNotIn("closure_residual_ev_per_source", energy)
        observed = np.array([[10.0 + (1.0 if i % 2 == 0 else -1.0),
                              4.0 + (0.4 if i % 2 == 0 else -0.4),
                              6.0 + (0.6 if i % 2 == 0 else -0.6)] for i in range(30)])
        expected_covariance = np.cov(observed, rowvar=False, ddof=1) / len(observed)
        np.testing.assert_allclose(energy["covariance_of_mean_ev2"], expected_covariance.reshape(-1),
                                   rtol=1e-12, atol=1e-14)
        self.assertAlmostEqual(energy["closure_residual"]["mean"], 0.0)
        self.assertTrue(energy["closure_residual"]["within_3se"])
        self.assertEqual(energy["cross_run_covariance"], "unknown_to_flux_and_breeder_H3")
        self.assertEqual(len(energy["covariance_of_mean_ev2"]), 9)

    def test_heating_target_stops_early_only_with_reliable_mean_peak_and_vov(self):
        import numpy as np

        names = ("neutron", "photon", "electron", "positron", "total")

        class BatchLib:
            def __init__(self, values, planned_batches):
                self.batch = 0
                self.values = values
                self.planned_batches = planned_batches
                self.tallies = {i: SimpleNamespace(results=np.zeros((1, 1, 2))) for i in range(1, 6)}
                self.tallies.update({i + 10: SimpleNamespace(results=np.zeros((2, 1, 2))) for i in range(1, 6)})

            def next_batch(self):
                value = self.values[self.batch % len(self.values)]
                for index, name in enumerate(names, 1):
                    component_value = value if name == "total" else (value - 2.0 if name == "neutron" else (1.0 if name in ("photon", "electron") else 0.0))
                    self.tallies[index].results[0, 0, 1] += component_value
                    self.tallies[index + 10].results[:, 0, 1] += [component_value * 0.75, component_value * 0.25]
                self.batch += 1
                return 1 if self.batch == self.planned_batches else 0

        refs = {name: SimpleNamespace(id=i) for i, name in enumerate(names, 1)}
        voxel_refs = {name: SimpleNamespace(id=i + 10) for i, name in enumerate(names, 1)}
        volumes, ranked = np.ones((1, 2)), np.ones((1, 2), dtype=bool)
        result = DT.collect_heating_batches(BatchLib([9.0, 11.0], 100), refs, voxel_refs, 1, 2,
                                            100, 1e9, 0.0, 0.05, volumes, ranked)
        _, _, _, _, aggregate, peak_error, batches, reason, _ = result
        self.assertEqual((batches, reason), (30, "target_reached"))
        self.assertLessEqual(aggregate.relative_error, 0.05)
        self.assertLessEqual(peak_error, 0.10)
        self.assertLessEqual(aggregate.variance_of_variance, DT.HEATING_VOV_LIMIT)

        means = np.array([[10.0], [2.0]])
        errors = np.array([[0.5], [0.1]])
        selected_peak = DT.peak_of_moments(means[:, 0], errors[:, 0], volumes[0], ranked[0])
        expected = selected_peak["standard_error"] / selected_peak["mean"]
        self.assertAlmostEqual(DT.heating_peak_relative_error(means, errors, volumes, ranked), expected)

        rare = DT.collect_heating_batches(BatchLib([1.0] * 29 + [100.0], 30), refs, voxel_refs, 1, 2,
                                          30, 1e9, 0.0, 0.05, volumes, ranked)
        _, _, _, _, aggregate, peak_error, batches, reason, _ = rare
        self.assertEqual((batches, reason), (30, "history_limit"))
        self.assertGreater(aggregate.variance_of_variance, DT.HEATING_VOV_LIMIT)
        self.assertTrue(peak_error is None or peak_error > 0.10)

        zero = DT.collect_heating_batches(BatchLib([0.0], 30), refs, voxel_refs, 1, 2,
                                          30, 1e9, 0.0, 0.05, volumes, ranked)
        _, _, _, _, aggregate, peak_error, batches, reason, _ = zero
        self.assertEqual((batches, reason), (30, "history_limit"))
        self.assertIsNone(aggregate.relative_error)
        self.assertIsNone(aggregate.variance_of_variance)
        self.assertIsNone(peak_error)

    def test_streaming_batch_moments_match_independent_sequence_and_covariant_residual(self):
        import math
        import numpy as np

        components = ("neutron", "photon", "electron", "positron", "total")
        sequences = [
            {"neutron": [8.0], "photon": [1.0], "electron": [1.0], "positron": [0.0], "total": [11.0]},
            {"neutron": [6.0], "photon": [1.0], "electron": [1.0], "positron": [0.0], "total": [9.0]},
            {"neutron": [10.0], "photon": [1.0], "electron": [1.0], "positron": [0.0], "total": [13.0]},
            {"neutron": [4.0], "photon": [1.0], "electron": [1.0], "positron": [0.0], "total": [7.0]},
        ]
        voxel_sequences = [
            {"neutron": [8.0, 4.0], "photon": [1.0, 1.0], "electron": [1.0, 1.0],
             "positron": [0.0, 0.0], "total": [11.0, 7.0]},
            {"neutron": [6.0, 2.0], "photon": [1.0, 1.0], "electron": [1.0, 1.0],
             "positron": [0.0, 0.0], "total": [9.0, 5.0]},
            {"neutron": [10.0, 6.0], "photon": [1.0, 1.0], "electron": [1.0, 1.0],
             "positron": [0.0, 0.0], "total": [13.0, 9.0]},
            {"neutron": [4.0, 0.0], "photon": [1.0, 1.0], "electron": [1.0, 1.0],
             "positron": [0.0, 0.0], "total": [7.0, 3.0]},
        ]
        tally_arrays, voxel_arrays, tally_refs, voxel_refs = {}, {}, {}, {}
        for index, name in enumerate(components):
            tally_id, voxel_id = index + 1, index + 11
            tally_arrays[tally_id] = SimpleNamespace(results=np.zeros((1, 1, 2)))
            voxel_arrays[voxel_id] = SimpleNamespace(results=np.zeros((2, 1, 2)))
            tally_refs[name] = SimpleNamespace(id=tally_id)
            voxel_refs[name] = SimpleNamespace(id=voxel_id)

        class CombinedLib:
            def __init__(self):
                self.batch = 0
                self.tallies = {**tally_arrays, **voxel_arrays}

            def next_batch(self):
                for name, values in sequences[self.batch].items():
                    self.tallies[tally_refs[name].id].results[:, 0, 1] += values
                for name, values in voxel_sequences[self.batch].items():
                    self.tallies[voxel_refs[name].id].results[:, 0, 1] += values
                self.batch += 1
                return 1 if self.batch == len(sequences) else 0

        summary = DT.collect_heating_batches(
            CombinedLib(), tally_refs, voxel_refs, 1, 2, 4, 1e9, 0.0)
        batch_moments, voxel_moments, closure, voxel_closure, _, _, count, _, _ = summary
        self.assertEqual(count, 4)
        # Direct textbook calculations over the named batch sequences are independent of Welford.
        self.assertEqual(batch_moments["neutron"].mean[0], 7.0)
        self.assertAlmostEqual(batch_moments["neutron"].standard_error[0], math.sqrt(5.0 / 3.0))
        self.assertEqual(batch_moments["total"].mean[0], 10.0)
        self.assertAlmostEqual(batch_moments["total"].standard_error[0], math.sqrt(5.0 / 3.0))
        self.assertEqual(closure.mean.tolist(), [1.0])
        self.assertEqual(closure.standard_error.tolist(), [0.0])
        self.assertEqual(voxel_moments["neutron"].mean[:, 0].tolist(), [7.0, 3.0])
        self.assertAlmostEqual(voxel_moments["neutron"].standard_error[0, 0], math.sqrt(5.0 / 3.0))
        self.assertEqual(voxel_closure.mean[:, 0].tolist(), [1.0, 1.0])
        self.assertEqual(voxel_closure.standard_error[:, 0].tolist(), [0.0, 0.0])
        self.assertEqual(batch_moments["neutron"].mean.shape, (1,))
        self.assertFalse(hasattr(batch_moments["neutron"], "samples"))

    def test_streaming_state_size_is_independent_of_batch_count(self):
        import numpy as np

        class LongLib:
            def __init__(self, refs):
                self.batch = 0
                self.tallies = {ref.id: SimpleNamespace(results=np.zeros((1, 1, 2)))
                                for ref in [*refs.values(), *(SimpleNamespace(id=n + 11) for n in range(5))]}

            def next_batch(self):
                self.batch += 1
                for tally in self.tallies.values():
                    tally.results[:, 0, 1] += 1.0
                return 1 if self.batch == 20_000 else 0

        names = ("neutron", "photon", "electron", "positron", "total")
        refs = {name: SimpleNamespace(id=index + 1) for index, name in enumerate(names)}
        voxel_refs = {name: SimpleNamespace(id=index + 11) for index, name in enumerate(names)}
        summary = DT.collect_heating_batches(LongLib(refs), refs, voxel_refs, 1, 1, 20_000, 1e9, 0.0)
        tally_moments, voxel_moments, _, _, _, _, count, _, _ = summary
        self.assertEqual(count, 20_000)
        self.assertEqual(tally_moments["total"].mean.shape, (1,))
        self.assertEqual(voxel_moments["total"].mean.shape, (1, 1))
        self.assertEqual(tally_moments["total"].m2.shape, (1,))

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
                self._dll = types.SimpleNamespace(_name=None)
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

        total_bias, zero_scores = {"value": 0.0}, {"value": False}
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
                    if zero_scores["value"]:
                        values = [0.0] * len(values)
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
                         "max_seconds": 1000.0, "seed": 5, "threads": 1, "target_relative_error": 0.05}}

        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            for name in ("model.h5m", "cross_sections.xml"):
                (work / name).write_text("identity")
            request["h5m"] = str(work / "model.h5m")
            request["cross_sections"] = str(work / "cross_sections.xml")
            lib_path = work / "libopenmc-fixture.so"
            lib_path.write_bytes(b"fake OpenMC C API library")
            lib._dll._name = str(lib_path)
            (work / "solver").mkdir()
            (work / "peak").mkdir()
            runtime_identity = DT.heating_runtime_identity(openmc)
            (work / "solver" / "heating-preflight.json").write_text(json.dumps({
                "status": "READY", "data_identity": {"source": "fixture"},
                "runtime_identity": runtime_identity,
                "cross_sections_sha256": "fixture-hash", "h5m_sha256": DT.sha256_file(request["h5m"]),
                "mesh_voxel_solid_count": 2,
                "volume_mesh_element_count": 2,
                "volume_max_materials": 2,
                "volume_material_slot_count": 4,
                "volume_material_estimated_bytes": 96}))
            np.savez(work / "peak" / "volumes.npz", volumes_cm3=np.array([[[10000.0, 20000.0]]]),
                     r_edges_cm=np.array([100.0, 105.0, 110.0]), z_edges_cm=np.array([-5.0, 0.0]))
            (work / "peak" / "record.json").write_text(json.dumps({
                "volumes_sha256": "fixture-hash", "h5m_sha256": DT.sha256_file(request["h5m"]),
                "solid_ids": ["coil"], "voxel_cm": [5.0, 5.0], "rays": 1000}))
            fake_modules = {"openmc": openmc, "openmc.lib": lib, "openmc_plasma_source": plasma_source,
                            "h5py": types.ModuleType("h5py")}
            real_sha256_file = DT.sha256_file
            runtime_paths = {Path(runtime_identity[key]["path"]).resolve()
                             for key in ("python", "openmc_c_api_library", "worker_helper")}
            def fixture_sha256_file(path):
                canonical = Path(path).resolve()
                exact_h5ms = {Path(request["h5m"]).resolve(),
                              (work / "geometry" / "model-heating-source.h5m").resolve()}
                return real_sha256_file(canonical) if canonical in runtime_paths | exact_h5ms else "fixture-hash"
            def fake_geometry_copy(request, target, random_ray):
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(b"retagged fixture H5M")
                return target
            with patch.dict("sys.modules", fake_modules), \
                 patch.object(DT, "heating_data_identity", return_value={"source": "fixture"}), \
                 patch.object(DT, "sha256_file", side_effect=fixture_sha256_file), \
                 patch.object(DT, "geometry_copy", side_effect=fake_geometry_copy), \
                 patch.object(DT, "build_model", return_value=(model, None, None)), \
                 patch.object(DT, "tokamak_arguments", return_value={}), \
                 patch.object(DT, "lost_count", return_value=0):
                preflight_path = work / "solver" / "heating-preflight.json"
                ready_preflight = json.loads(preflight_path.read_text())
                drifted_preflight = copy.deepcopy(ready_preflight)
                drifted_preflight["runtime_identity"]["openmc_version"] = "0.16.1"
                preflight_path.write_text(json.dumps(drifted_preflight))
                refused = DT.phase_heating(request, work)
                self.assertEqual(refused["status"], "NOT_EVALUATED")
                self.assertEqual(lib.batch, 0)
                preflight_path.write_text(json.dumps(ready_preflight))
                volume_record_path = work / "peak" / "record.json"
                volume_record = json.loads(volume_record_path.read_text())
                volume_record["h5m_sha256"] = "wrong-hash"
                volume_record_path.write_text(json.dumps(volume_record))
                stale_volume = DT.phase_heating(request, work)
                self.assertEqual(stale_volume["status"], "NOT_EVALUATED")
                self.assertEqual(lib.batch, 0)
                volume_record["h5m_sha256"] = ready_preflight["h5m_sha256"]
                volume_record_path.write_text(json.dumps(volume_record))
                raw = DT.phase_heating(request, work)
                stored = json.loads((work / "solver" / "heating-result.json").read_text())
                self.assertEqual(raw, stored)
                self.assertEqual(raw["status"], "INCONCLUSIVE")
                self.assertFalse(raw["target_reached"])
                self.assertIsNone(raw["variance_of_variance"])
                self.assertIn("variance-of-variance is unavailable", raw["why"])
                self.assertEqual(raw["normalization"], "per_source_neutron")
                self.assertEqual(raw["runtime_identity"], runtime_identity)
                self.assertEqual(
                    raw["execution_identity"]["run_h5m"]["sha256"],
                    real_sha256_file(work / "geometry" / "model-heating.h5m"),
                )
                self.assertEqual(
                    raw["execution_identity"]["model_xml"]["sha256"],
                    real_sha256_file(work / "solver" / "heating-model.xml"),
                )
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
                total_bias["value"] = 0.0
                zero_scores["value"] = True
                lib.batch = 0
                zero = DT.phase_heating(request, work)
                self.assertEqual(zero["status"], "INCONCLUSIVE")
                self.assertIn("relative heating uncertainty is unavailable", zero["why"])
                self.assertEqual(zero["solids"][0]["peak"]["mean"], 0.0)
                self.assertIsNone(zero["solids"][0]["peak"]["standard_error"])

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
