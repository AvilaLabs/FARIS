# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Synthetic tests for R3 batch aggregation; no solver output is used."""
from __future__ import annotations

import sys
import os
import tempfile
from pathlib import Path
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_dose_statistics as DS  # noqa: E402


class R3DoseStatisticsTest(unittest.TestCase):
    def test_d1s_aggregate_applies_parent_factors_before_summing_bins(self):
        # Synthetic two-voxel/two-parent values. Each batch contains correlated parent and
        # spatial contributions; aggregation must happen before the batch SE is calculated.
        previous = [0.0, 0.0, 0.0, 0.0]
        factors = [2.0, 0.5]
        batches = [[1.0, 4.0, 2.0, 2.0], [3.0, 8.0, 4.0, 4.0]]
        totals = []
        for cumulative in batches:
            previous, total = DS.d1s_batch_total(cumulative, previous, 2, 2, factors)
            totals.append(total)
        self.assertEqual(totals, [9.0, 11.0])
        summary = DS.batch_summary(totals)
        self.assertEqual(summary["mean"], 10.0)
        self.assertAlmostEqual(summary["standard_error"], 1.0)
        self.assertEqual(summary["uncertainty_scope"], "Monte Carlo batch sampling only")

    def test_fully_correlated_bins_use_batch_aggregate_standard_error(self):
        previous = [0.0, 0.0, 0.0, 0.0]
        factors = [2.0, 0.5]
        totals = []
        for cumulative in ([1.0, 2.0, 3.0, 4.0], [4.0, 6.0, 8.0, 10.0]):
            previous, total = DS.d1s_batch_total(cumulative, previous, 2, 2, factors)
            totals.append(total)
        self.assertEqual(totals, [11.0, 21.0])
        summary = DS.batch_summary(totals)
        self.assertEqual(summary["mean"], 16.0)
        self.assertEqual(summary["standard_error"], 5.0)
        # Independent marginal quadrature discards the deliberately shared batch variation.
        self.assertAlmostEqual((8.5 ** 0.5), 2.9154759474226504)
        self.assertNotAlmostEqual(summary["standard_error"], 8.5 ** 0.5)

    def test_r2s_scalar_batch_differences_recover_per_batch_values(self):
        previous = 0.0
        values = []
        for cumulative in [3.0, 9.0, 12.0]:
            previous, batch_value = DS.scalar_batch_total(cumulative, previous)
            values.append(batch_value)
        self.assertEqual(values, [3.0, 6.0, 3.0])
        self.assertEqual(DS.batch_summary(values)["mean"], 4.0)

    def test_malformed_or_nonfinite_d1s_arrays_are_refused(self):
        with self.assertRaisesRegex(ValueError, "shape"):
            DS.d1s_batch_total([1.0, 2.0], [0.0, 0.0], 2, 2, [1.0, 1.0])
        with self.assertRaisesRegex(ValueError, "finite"):
            DS.d1s_batch_total([float("nan")], [0.0], 1, 1, [1.0])

    def test_uncertainty_requires_two_batches(self):
        with self.assertRaisesRegex(ValueError, "two complete batches"):
            DS.batch_summary([1.0])

    def test_run_live_tally_records_prehash_histories_and_lost_particle_rejection(self):
        class FakeTally:
            id = 7
            num_realizations = 0
            results = None
            std_dev = None

        class FakeLib:
            _dll = object()
            tallies = {7: FakeTally()}

            def init(self, _args):
                self.tallies[7].results = np.zeros((1, 1, 2))
                self.tallies[7].std_dev = np.zeros((1, 1))

            def simulation_init(self):
                pass

            def next_batch(self):
                self.tallies[7].num_realizations += 1
                self.tallies[7].results[0, 0, 1] += 1.0
                return 0

            def simulation_finalize(self):
                pass

            def finalize(self):
                pass

        class FakeOpenMC:
            __version__ = "0.15.3"
            lib = FakeLib()

        with tempfile.TemporaryDirectory() as temp:
            model = Path(temp) / "model.xml"
            model.write_text("synthetic C API model\n", encoding="utf-8")
            expected_model_hash = DS._sha256_file(model)
            original_lost_count = DS._lost_count
            original_identity = DS._library_identity
            DS._lost_count = lambda _dll: 1
            DS._library_identity = lambda _openmc: {"path": "/synthetic/libopenmc.so", "sha256": "f" * 64}
            try:
                result, _, _ = DS.run_live_tally(FakeOpenMC, model, 7, 1, 3, 60.0, lambda *_: None,
                                                 particles_per_batch=500_000)
            finally:
                DS._lost_count = original_lost_count
                DS._library_identity = original_identity
        integrity = result["integrity"]
        self.assertEqual(integrity["solver_model"]["sha256"], expected_model_hash)
        self.assertEqual(integrity["lost_particle_assessment"]["status"], "REJECTED")
        self.assertEqual(integrity["lost_particle_assessment"]["histories"], 500_000)

    def test_neutron_manager_hook_pairs_target_init_run_and_ignores_other_runs(self):
        class FakeSettings:
            batches = 5
            particles = 100

        class FakeLib:
            _dll = object()
            settings = FakeSettings()

            def __init__(self):
                self.calls = []

            def init(self, *args, **kwargs):
                self.calls.append("init")

            def run(self, *args, **kwargs):
                self.calls.append("run")

        class FakeOpenMC:
            lib = FakeLib()

        original_identity = DS._library_identity
        original_lost = DS._lost_count
        DS._library_identity = lambda _openmc: {"path": "/synthetic/libopenmc.so", "sha256": "a" * 64}
        DS._lost_count = lambda _dll: 0
        previous_cwd = Path.cwd()
        try:
            with tempfile.TemporaryDirectory() as temp:
                cwd = Path(temp)
                model = cwd / "model.xml"
                model.write_text('<model><tallies><tally name="unrelated"/></tallies></model>', encoding="utf-8")
                evidence = DS.NeutronManagerRunEvidence(FakeOpenMC, cwd / "manager-model.xml")
                os.chdir(cwd)
                with evidence:
                    FakeOpenMC.lib.init()
                    FakeOpenMC.lib.run()
                    model.write_text('<model><tallies><tally name="MicroXS flux"/></tallies></model>', encoding="utf-8")
                    FakeOpenMC.lib.init()
                    FakeOpenMC.lib.run()
                    (cwd / "manager-model.xml").write_text(model.read_text(), encoding="utf-8")
                result = evidence.result()
                self.assertEqual(FakeOpenMC.lib.calls, ["init", "run", "init", "run"])
                self.assertEqual(result["status"], "PASS")
                self.assertEqual(result["histories"], 500)
                self.assertEqual(result["lost_particle_assessment"]["lost_particles"], 0)
        finally:
            os.chdir(previous_cwd)
            DS._library_identity = original_identity
            DS._lost_count = original_lost

    def test_neutron_manager_hook_fails_closed_without_run_callback_or_when_model_changes(self):
        class FakeSettings:
            batches = 2
            particles = 25

        class FakeLib:
            _dll = object()
            settings = FakeSettings()
            mutate_on_init = False
            model_path = None

            def init(self, *args, **kwargs):
                if self.mutate_on_init:
                    self.model_path.write_text('<model><tally name="mutated by init"/></model>', encoding="utf-8")

            def run(self, *args, **kwargs):
                pass

        class FakeOpenMC:
            lib = FakeLib()

        original_identity = DS._library_identity
        original_lost = DS._lost_count
        DS._library_identity = lambda _openmc: {"path": "/synthetic/libopenmc.so", "sha256": "b" * 64}
        DS._lost_count = lambda _dll: 0
        previous_cwd = Path.cwd()
        try:
            with tempfile.TemporaryDirectory() as temp:
                cwd = Path(temp)
                model = cwd / "model.xml"
                output_model = cwd / "persisted.xml"
                model.write_text('<model><tally name="MicroXS flux"/></model>', encoding="utf-8")
                os.chdir(cwd)
                unpaired = DS.NeutronManagerRunEvidence(FakeOpenMC, output_model)
                with unpaired:
                    FakeOpenMC.lib.init()
                self.assertEqual(unpaired.result()["status"], "NOT_EVALUATED")

                changed = DS.NeutronManagerRunEvidence(FakeOpenMC, output_model)
                with changed:
                    FakeOpenMC.lib.init()
                    model.write_text('<model><tally name="changed after init"/></model>', encoding="utf-8")
                    FakeOpenMC.lib.run()
                self.assertEqual(changed.result()["status"], "NOT_EVALUATED")

                model.write_text('<model><tally name="MicroXS flux"/></model>', encoding="utf-8")
                alternate_args = DS.NeutronManagerRunEvidence(FakeOpenMC, output_model)
                with alternate_args:
                    FakeOpenMC.lib.init(args=["/different/input/model.xml"])
                    FakeOpenMC.lib.run()
                self.assertIn("explicit args", alternate_args.result()["reason"])

                model.write_text('<model><tally name="MicroXS flux"/></model>', encoding="utf-8")
                FakeOpenMC.lib.model_path = model
                FakeOpenMC.lib.mutate_on_init = True
                mutated_during_init = DS.NeutronManagerRunEvidence(FakeOpenMC, output_model)
                with mutated_during_init:
                    FakeOpenMC.lib.init()
                    FakeOpenMC.lib.run()
                FakeOpenMC.lib.mutate_on_init = False
                self.assertIn("changed during C API initialization", mutated_during_init.result()["reason"])
        finally:
            os.chdir(previous_cwd)
            DS._library_identity = original_identity
            DS._lost_count = original_lost

    def test_next_batch_status_one_is_limit_and_status_two_is_trigger(self):
        class FakeTally:
            id = 9
            num_realizations = 0

            def __init__(self):
                self.results = np.zeros((1, 1, 2))
                self.std_dev = np.zeros((1, 1))

        class FakeLib:
            _dll = object()
            settings = None

            def __init__(self, statuses):
                self.tallies = {9: FakeTally()}
                self.statuses = list(statuses)

            def init(self, _args):
                pass

            def simulation_init(self):
                pass

            def next_batch(self):
                tally = self.tallies[9]
                tally.num_realizations += 1
                tally.results[0, 0, 1] += 1.0
                return self.statuses.pop(0)

            def simulation_finalize(self):
                pass

            def finalize(self):
                pass

        original_identity = DS._library_identity
        original_lost = DS._lost_count
        DS._library_identity = lambda _openmc: {"path": "/synthetic/libopenmc.so", "sha256": "c" * 64}
        DS._lost_count = lambda _dll: 0
        previous_cwd = Path.cwd()
        try:
            with tempfile.TemporaryDirectory() as temp:
                model = Path(temp) / "model.xml"
                model.write_text("synthetic model", encoding="utf-8")
                outputs = []
                for statuses in ([0, 1], [2], [1]):
                    class FakeOpenMC:
                        pass
                    FakeOpenMC.lib = FakeLib(statuses)
                    result, _, _ = DS.run_live_tally(FakeOpenMC, model, 9, 1, 2, 60.0,
                                                     lambda *_: None, particles_per_batch=10)
                    outputs.append((result["stop_reason"], result["solver_status"], result["batches"]))
            self.assertEqual(outputs, [("batch_limit", 1, 2), ("tally_trigger", 2, 1),
                                       ("early_maximum_batches", 1, 1)])
        finally:
            os.chdir(previous_cwd)
            DS._library_identity = original_identity
            DS._lost_count = original_lost


if __name__ == "__main__":
    unittest.main()
