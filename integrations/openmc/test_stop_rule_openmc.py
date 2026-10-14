"""The reactor worker's stop rule through the real OpenMC 0.15.3 (PERF-025, NUC-043).

A small iron sphere with a neutron source at its centre and a fast-flux tally named as the worker's
`magnets-fast-flux` response. The worker's own `stop_rule`, `apply_stop_rule`, `read_batch_sums` and
`judge_stop` configure and check the run:

- with a loose target the run stops after the 30-batch minimum, before the budget, with one
  statepoint per batch it reached, and OpenMC's relative error is at or below the target;
- with a target the budget cannot reach, the run uses the whole budget and says so.

The tests skip, with the reason, without OpenMC or the nuclear data; with FARIS_REQUIRE_OPENMC=1 (the CI job
`openmc-tests`) that absence fails them. Run with the OpenMC 0.15.3 python:

    FARIS_CI_NEUTRON_DATA=<folder with H1.h5 and Fe56.h5> python3 -m unittest test_stop_rule_openmc
"""

import importlib.util
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

import openmc_ci

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
ADAPTER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ADAPTER)

PER_BATCH = 2000
RESPONSE = f"response-{ADAPTER.STOP_RULE_RESPONSE_ID}"


def run_case(data_dir: Path, scratch: Path, sampling: dict):
    """Run the iron sphere under the stop rule `sampling` asks for; return (realized, achieved, outcome)."""
    import openmc

    openmc.config["cross_sections"] = str(openmc_ci.cross_sections_xml(data_dir, scratch))
    iron = openmc.Material(name="iron")
    iron.add_nuclide("Fe56", 1.0)
    iron.set_density("g/cm3", 7.8)
    sphere = openmc.Sphere(r=15.0, boundary_type="vacuum")
    model = openmc.Model()
    model.materials = openmc.Materials([iron])
    model.geometry = openmc.Geometry([openmc.Cell(fill=iron, region=-sphere)])
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.particles = PER_BATCH
    settings.seed = 11
    settings.source = openmc.IndependentSource(space=openmc.stats.Point((0.0, 0.0, 0.0)), energy=openmc.stats.Discrete([14.1e6], [1.0]))
    settings.batches = int(sampling["batches"])
    settings.statepoint = {"batches": list(range(1, int(sampling["batches"]) + 1))}
    tally = openmc.Tally(name=RESPONSE)
    tally.filters = [openmc.CellFilter(model.geometry.get_all_cells().popitem()[1]), openmc.ParticleFilter("neutron"), openmc.EnergyFilter([1.0e5, 2.0e7])]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])
    rule = ADAPTER.stop_rule(sampling)
    ADAPTER.apply_stop_rule(openmc, settings, {tally.name: tally}, rule)
    model.export_to_model_xml(scratch)
    here = os.getcwd()
    os.chdir(scratch)
    try:
        # the interpreter's own bin folder when the environment is not activated
        openmc.run(output=False, threads=2, openmc_exec=shutil.which("openmc") or str(Path(sys.executable).parent / "openmc"))
    finally:
        os.chdir(here)
    min_batches = rule["min_batches"] if rule else int(sampling["batches"])
    sums, realized = ADAPTER.read_batch_sums(openmc, scratch, min_batches, int(sampling["batches"]), [tally.name])
    (final,) = scratch.glob("statepoint.*.h5")  # the zero-padded name of the last batch
    with openmc.StatePoint(str(final)) as sp:
        result = sp.get_tally(name=tally.name)
        achieved = float(result.std_dev.ravel()[0] / result.mean.ravel()[0])
    ADAPTER.verify_batch_values(RESPONSE, ADAPTER.per_batch_values(sums[tally.name]), float(result.mean.ravel()[0]), float(result.std_dev.ravel()[0]))
    return realized, achieved, ADAPTER.judge_stop(rule, realized, achieved)


class StopRuleThroughOpenMC(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        openmc_ci.require_modules("openmc", "numpy", "h5py")
        cls.data = openmc_ci.neutron_data_dir()

    def run_sampling(self, sampling):
        with tempfile.TemporaryDirectory() as scratch:
            return run_case(self.data, Path(scratch), sampling)

    # Verifies: PERF-025, NUC-043
    def test_loose_target_stops_before_the_budget(self):
        realized, achieved, (code, _) = self.run_sampling({"batches": 150, "target_relative_error": 0.03})
        self.assertGreaterEqual(realized, 30)
        self.assertLess(realized, 150)
        self.assertLessEqual(achieved, 0.03)
        self.assertEqual(code, "target_met")

    # Verifies: NUC-043
    def test_unreachable_target_uses_the_budget_and_reports_it(self):
        realized, achieved, (code, words) = self.run_sampling({"batches": 40, "target_relative_error": 1.0e-6})
        self.assertEqual(realized, 40)
        self.assertGreater(achieved, 1.0e-6)
        self.assertEqual(code, "budget_exhausted")
        self.assertIn("budget", words)

    def test_fixed_run_is_unchanged(self):
        realized, _, (code, _) = self.run_sampling({"batches": 35})
        self.assertEqual((realized, code), (35, "fixed_count"))


if __name__ == "__main__":
    unittest.main()
