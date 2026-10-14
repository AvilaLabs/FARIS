"""The reactor stop rule's variance guard continues a run from its last statepoint (NUC-043).

A run that reaches its target error while the variance of the variance of a watched response is above the limit
goes on with more batches instead of stopping (reactor_transport.sample_until_settled). This runs that on a small
iron-sphere model with OpenMC 0.15.3, with a limit so low that every stop is unsettled, so the run must be taken
on to its batch budget by restarts, and checks that the restarted run is one run: the batch count, the tally
accumulated by OpenMC across the restarts and the per-batch sums the guard works from.

Nuclear data is the Fe56 file of FARIS_CI_NEUTRON_DATA (see openmc_ci.py). Run with the OpenMC 0.15.3 python:

    FARIS_CI_NEUTRON_DATA=<folder> python3 -m unittest test_guard_restart_openmc
"""

import importlib.util
import math
import os
from pathlib import Path
import sys
import tempfile
import unittest

import openmc_ci

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)

BUDGET = 45
SAMPLING = {"batches": BUDGET, "particles_per_batch": 1000, "seed": 17, "threads": 2, "target_relative_error": 0.5}


class GuardRestartsARun(unittest.TestCase):
    def setUp(self):
        openmc_ci.require_modules("numpy", "h5py", "openmc")
        self.data = openmc_ci.neutron_data_dir()

    def model(self, openmc, library):
        iron = openmc.Material()
        iron.add_nuclide("Fe56", 1.0)
        iron.set_density("g/cm3", 7.874)
        radii = [openmc.Sphere(r=r) for r in (2.0, 22.0, 26.0)]
        radii[-1].boundary_type = "vacuum"
        cells = [openmc.Cell(fill=iron, region=-radii[0]), openmc.Cell(fill=iron, region=+radii[0] & -radii[1]),
                 openmc.Cell(fill=iron, region=+radii[1] & -radii[2])]
        model = openmc.Model(geometry=openmc.Geometry(cells), materials=openmc.Materials([iron]))
        model.materials.cross_sections = str(library)
        settings = model.settings
        settings.run_mode = "fixed source"
        settings.particles, settings.seed = SAMPLING["particles_per_batch"], SAMPLING["seed"]
        settings.batches = BUDGET
        settings.statepoint = {"batches": list(range(1, BUDGET + 1))}
        settings.source = openmc.IndependentSource(
            energy=openmc.stats.Discrete([14.1e6], [1.0]), constraints={"domains": [cells[0]]})
        tally = openmc.Tally(name="response-x")
        tally.filters = [openmc.CellFilter([cells[2]])]
        tally.scores = ["flux"]
        model.tallies = openmc.Tallies([tally])
        rule = WORKER.stop_rule(dict(SAMPLING, watch_responses=["x"]))
        WORKER.apply_stop_rule(openmc, settings, {"response-x": tally}, rule)
        return model, rule

    # Verifies: NUC-043
    def test_an_unsettled_target_takes_the_run_on_to_its_budget_as_one_run(self):
        import warnings

        import openmc

        warnings.simplefilter("ignore", openmc.IDWarning)
        with tempfile.TemporaryDirectory() as scratch:
            out = Path(scratch)
            library = openmc_ci.cross_sections_xml(self.data, out)
            model, rule = self.model(openmc, library)
            model.export_to_xml(directory=str(out))
            hashes = {path.name: WORKER.sha256(path) for path in out.glob("*.xml")}
            env = dict(os.environ, OMP_NUM_THREADS="2", OPENMC_CROSS_SECTIONS=str(library))
            command = [str(Path(sys.executable).parent / "openmc")]
            _, sums, n_batches, restarts = WORKER.sample_until_settled(
                openmc, out, command, env, rule, BUDGET, ["response-x"], hashes, vov_limit=-1.0)
            self.assertEqual(n_batches, BUDGET)
            self.assertGreaterEqual(len(restarts), 1)
            # each restart begins where the run before it ended and adds batches, up to the budget
            self.assertEqual(restarts[0]["from_batch"], rule["min_batches"])
            for before, after in zip(restarts, restarts[1:]):
                self.assertEqual(after["from_batch"], before["to_batch"])
            self.assertEqual(restarts[-1]["to_batch"], BUDGET)
            self.assertEqual(hashes["settings.xml"], WORKER.sha256(out / "settings.xml"))
            # one statepoint is left, and it holds every batch: OpenMC accumulated across the restarts
            left = sorted(out.glob("statepoint.*.h5"))
            self.assertEqual([p.name.split(".")[1] for p in left], [str(BUDGET)])
            with openmc.StatePoint(str(left[0])) as state:
                self.assertEqual(int(state.n_realizations), BUDGET)
                tally = state.get_tally(name="response-x")
                mean, error = float(tally.mean.ravel()[0]), float(tally.std_dev.ravel()[0])
            values = WORKER.batch_values(sums["response-x"])
            self.assertEqual(len(values), BUDGET)
            self.assertAlmostEqual(math.fsum(values) / BUDGET, mean, delta=1.0e-9 * abs(mean))
            stats = WORKER.RunningSum()
            for value in values:
                stats.add(value)
            self.assertAlmostEqual(stats.standard_error, error, delta=1.0e-6 * error)
            # the logs of every part are kept
            self.assertIn("Total time elapsed", (out / "openmc.stdout.log").read_text())
            self.assertGreaterEqual((out / "openmc.stdout.log").read_text().count("Total time elapsed"), 2)

    # Verifies: NUC-043
    def test_a_settled_target_is_not_restarted(self):
        import warnings

        import openmc

        warnings.simplefilter("ignore", openmc.IDWarning)
        with tempfile.TemporaryDirectory() as scratch:
            out = Path(scratch)
            library = openmc_ci.cross_sections_xml(self.data, out)
            model, rule = self.model(openmc, library)
            model.export_to_xml(directory=str(out))
            hashes = {path.name: WORKER.sha256(path) for path in out.glob("*.xml")}
            env = dict(os.environ, OMP_NUM_THREADS="2", OPENMC_CROSS_SECTIONS=str(library))
            command = [str(Path(sys.executable).parent / "openmc")]
            _, _, n_batches, restarts = WORKER.sample_until_settled(
                openmc, out, command, env, rule, BUDGET, ["response-x"], hashes, vov_limit=1.0e9)
            self.assertEqual(restarts, [])
            self.assertEqual(n_batches, rule["min_batches"])


if __name__ == "__main__":
    unittest.main()
