# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("photon_energy_closure.py")
spec = importlib.util.spec_from_file_location("photon_energy_closure", SCRIPT)
control = importlib.util.module_from_spec(spec)
spec.loader.exec_module(control)

class BatchAnalysisTests(unittest.TestCase):
    def test_cumulative_to_batch_values(self):
        self.assertEqual(control.batch_values([3.0, 5.0, 5.5]), [3.0, 2.0, 0.5])

    def test_covariance_includes_same_batch_term(self):
        cols = {"whole": [10.0, 20.0, 30.0], "escape": [999990.0, 999981.0, 999969.0],
                "part_inner": [4.0, 8.0, 12.0], "part_shell": [6.0, 12.0, 18.0]}
        got = control.analyze_batches(cols)
        self.assertGreater(got["closure_variance_from_covariance_eV2"], 0.0)
        self.assertTrue(got["passed"])

    def test_analysis_does_not_accept_marginal_error_substitution(self):
        cols = {"whole": [100.0, 110.0, 90.0, 100.0], "escape": [900000.0, 900010.0, 899990.0, 900000.0],
                "part_inner": [40.0, 44.0, 36.0, 40.0], "part_shell": [60.0, 66.0, 54.0, 60.0]}
        got = control.analyze_batches(cols)
        cov = got["sample_covariance_matrix_batch_realizations"]
        naive = cov["whole"]["whole"] + cov["escape"]["escape"]
        self.assertNotAlmostEqual(naive, got["closure_variance_from_batch_differences_eV2"])

    def test_material_partition_must_close_in_each_batch(self):
        cols = {"whole": [1000000.0, 1000000.0, 1000000.0], "escape": [0.0, 0.0, 0.0],
                "part_inner": [1000000.0, 0.0, 900000.0], "part_shell": [0.0, 800000.0, 0.0]}
        self.assertFalse(control.analyze_batches(cols)["passed"])


@unittest.skipUnless(__import__("importlib").util.find_spec("h5py"), "h5py not installed")
class StatepointReaderTests(unittest.TestCase):
    @staticmethod
    def write_statepoint(path, batch, whole, inner, shell, escape):
        import h5py
        import numpy as np
        with h5py.File(path, "w") as f:
            f.create_dataset("current_batch", data=batch)
            f.create_dataset("n_realizations", data=batch)
            tallies = f.create_group("tallies")
            rows = [
                ("heating-whole-unfiltered", np.array([[[[whole, 0.0]]]])),
                ("heating-by-material", np.array([[[[inner, 0.0]]], [[[shell, 0.0]]]])),
                ("escaping-photon-energy", np.array([[[[escape, 0.0]]]])),
            ]
            for tally_id, (name, results) in enumerate(rows, 1):
                group = tallies.create_group(f"tally {tally_id}")
                group.create_dataset("name", data=np.bytes_(name))
                group.create_dataset("results", data=results)

    def test_padded_batch_filenames_recover_aligned_batch_values(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            # OpenMC formats single digit batch numbers with a leading zero.
            # Each HDF5 results dataset stores cumulative sums, not batch deltas.
            values = [
                (600_000.0, 200_000.0, 400_000.0, 400_000.0),
                (1_300_000.0, 500_000.0, 800_000.0, 700_000.0),
                (1_800_000.0, 600_000.0, 1_200_000.0, 1_200_000.0),
            ]
            for batch, row in enumerate(values, 1):
                self.write_statepoint(out / f"statepoint.{batch:02d}.h5", batch, *row)
            columns = control._extract_batch_tallies(out, 3)
            self.assertEqual(columns["whole"], [600_000.0, 700_000.0, 500_000.0])
            self.assertEqual(columns["part_inner"], [200_000.0, 300_000.0, 100_000.0])
            self.assertEqual(columns["part_shell"], [400_000.0, 400_000.0, 400_000.0])
            self.assertEqual(columns["escape"], [400_000.0, 300_000.0, 500_000.0])
            self.assertTrue(control.analyze_batches(columns)["passed"])

@unittest.skipUnless(__import__("importlib").util.find_spec("openmc"), "OpenMC API not installed")
class OpenMC0153APITests(unittest.TestCase):
    def test_actual_openmc_tally_and_source_api(self):
        import openmc
        if openmc.__version__ != "0.15.3":
            self.skipTest("API pin test targets OpenMC 0.15.3")
        model = control.make_openmc_model(openmc, batches=4, particles=10)
        self.assertEqual(model.settings.electron_treatment, "led")
        self.assertTrue(model.settings.photon_transport)
        # OpenMC 0.15.3 has no Python toggle: true is the solver's fixed default
        # when photon transport is enabled. Actual Fe relaxation tables are
        # checked separately against the pinned HDF5 input.
        self.assertFalse(hasattr(model.settings, "atomic_relaxation"))
        self.assertEqual(len(model.tallies), 3)
        self.assertEqual(model.tallies[0].name, "heating-whole-unfiltered")
        self.assertEqual(model.tallies[0].filters, [])
        self.assertEqual(model.tallies[1].scores, ["heating"])
        self.assertIsInstance(model.tallies[2].filters[-1], openmc.EnergyFunctionFilter)
        self.assertEqual(model.settings.source[0].particle, "photon")
        self.assertEqual(model.settings.statepoint["batches"], [1, 2, 3, 4])

    def test_actual_fe_hdf_has_relaxation_transition_tables(self):
        import openmc
        if openmc.__version__ != "0.15.3":
            self.skipTest("API pin test targets OpenMC 0.15.3")
        xslib = Path(os.environ.get(
            "FARIS_CROSS_SECTIONS",
            control.PROJECT_ROOT / "data/raw/combined-fendl32-endfbvii1/cross_sections.xml",
        ))
        if not xslib.is_file():
            self.skipTest("nuclear data is not in this checkout; set FARIS_CROSS_SECTIONS")
        result = control._verify_fe_relaxation_data(openmc, xslib)
        self.assertGreater(result["transition_count"], 0)

if __name__ == "__main__":
    unittest.main()
