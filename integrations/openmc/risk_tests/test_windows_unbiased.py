"""Do weight windows change the answer? A paired windowed-against-analogue run on a small slab (NUC-051).

The case is a 14.1 MeV neutron beam through 30 cm of iron (Fe56) and 30 cm of hydrogen (H1) into an empty
detector cell, reflecting sides so the slab is one-dimensional. The reported response is the fast
(> 0.1 MeV) flux in the detector cell. One run is analogue. The other applies weight windows made by
OpenMC 0.15.3's own MAGIC generator on a mesh along the slab, in a separate run with the reserved
window seed (NUC-053: the generating histories feed no tally and share no seed with a reported run), and
applied through `design_transport.apply_windows` after the same gap-filling step as the real windows.
All three runs have fixed seeds (the generation run on one thread), so the comparison does not change
from run to run on a given OpenMC build. The two means must agree within 3 combined standard errors, and
both standard errors must be under 5 % of their means, or the test could not tell a biased window from a
correct one.

Nuclear data is the FENDL-3.2 neutron files of H1 and Fe56 in the folder named by FARIS_CI_NEUTRON_DATA
(see openmc_ci.py). The test skips, with the reason, where OpenMC or that data is absent, and fails
there when FARIS_REQUIRE_OPENMC=1 (the CI job `openmc-tests`). Run with the OpenMC 0.15.3 python:

    FARIS_CI_NEUTRON_DATA=<folder> python3 -m unittest test_windows_unbiased
"""

import math
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import design_transport as DT  # noqa: E402
import openmc_ci  # noqa: E402

THREADS = 2
ANALOG_SEED, WINDOWED_SEED = 101, 202
GENERATION_SEED = DT.WINDOW_SEED
ENERGY_BOUNDS = [0.0, 1.0e5, 2.0e7]       # eV: the response group (> 0.1 MeV) is the upper one
IRON_CM, HYDROGEN_CM, DETECTOR_CM, HALF_WIDTH_CM = 30.0, 30.0, 5.0, 50.0
ANALOG_RUN = {"batches": 100, "particles": 10000}
WINDOWED_RUN = {"batches": 40, "particles": 5000}
GENERATION_RUN = {"batches": 30, "particles": 2000}
MAX_RELATIVE_ERROR = 0.05
Z_LIMIT = 3.0


def build_model(openmc, cross_sections: Path, run: dict, seed: int):
    iron = openmc.Material(material_id=1, name="iron")
    iron.add_nuclide("Fe56", 1.0, percent_type="ao")
    iron.set_density("g/cm3", 7.874)
    hydrogen = openmc.Material(material_id=2, name="hydrogen")
    hydrogen.add_nuclide("H1", 1.0, percent_type="ao")
    hydrogen.set_density("g/cm3", 0.3)
    materials = openmc.Materials([iron, hydrogen])
    materials.cross_sections = str(cross_sections)
    x = [0.0, IRON_CM, IRON_CM + HYDROGEN_CM, IRON_CM + HYDROGEN_CM + DETECTOR_CM]
    planes = [openmc.XPlane(x0=value) for value in x]
    planes[0].boundary_type = planes[-1].boundary_type = "vacuum"
    sides = [openmc.YPlane(y0=-HALF_WIDTH_CM), openmc.YPlane(y0=HALF_WIDTH_CM),
             openmc.ZPlane(z0=-HALF_WIDTH_CM), openmc.ZPlane(z0=HALF_WIDTH_CM)]
    for side in sides:
        side.boundary_type = "reflective"
    lateral = +sides[0] & -sides[1] & +sides[2] & -sides[3]
    cells = [openmc.Cell(cell_id=10 + n, fill=fill, region=+planes[n] & -planes[n + 1] & lateral)
             for n, fill in enumerate([iron, hydrogen, None])]
    model = openmc.Model(geometry=openmc.Geometry(cells), materials=materials)
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches, settings.particles, settings.seed = run["batches"], run["particles"], seed
    settings.source = openmc.IndependentSource(
        space=openmc.stats.Point((1.0e-3, 0.0, 0.0)), angle=openmc.stats.Monodirectional((1.0, 0.0, 0.0)),
        energy=openmc.stats.Discrete([14.1e6], [1.0]), particle="neutron")
    settings.output = {"summary": False}
    tally = openmc.Tally(tally_id=1, name="fast-flux-behind-the-slab")
    tally.filters = [openmc.CellFilter([cells[2]]), openmc.EnergyFilter([1.0e5, 2.0e7])]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])
    return model


def window_mesh(openmc):
    mesh = openmc.RegularMesh(mesh_id=7, name="windows")
    mesh.lower_left = (0.0, -HALF_WIDTH_CM, -HALF_WIDTH_CM)
    mesh.upper_right = (IRON_CM + HYDROGEN_CM + DETECTOR_CM, HALF_WIDTH_CM, HALF_WIDTH_CM)
    mesh.dimension = (65, 1, 1)
    return mesh


def run_in(directory: Path, model, log: str, threads: int = THREADS):
    directory.mkdir(parents=True, exist_ok=True)
    model.export_to_model_xml(str(directory))
    DT.run_cli(directory, threads, log)


def read_tally(openmc, directory: Path, batches: int) -> tuple[float, float]:
    """(mean, standard error) of the detector flux per source neutron."""
    with openmc.StatePoint(str(directory / f"statepoint.{batches}.h5")) as state:
        tally = state.get_tally(id=1)
        return float(tally.mean.ravel()[0]), float(tally.std_dev.ravel()[0])


class WindowsDoNotBiasTheAnswer(unittest.TestCase):
    def setUp(self):
        openmc_ci.require_modules("numpy", "h5py", "openmc")
        self.data = openmc_ci.neutron_data_dir()

    # Verifies: NUC-051
    # Verifies: NUC-053
    def test_windowed_and_analogue_fast_flux_agree_within_three_sigma(self):
        import warnings

        import openmc
        import s0f_common

        # the models are rebuilt with the same explicit ids in one process
        warnings.simplefilter("ignore", openmc.IDWarning)
        self.assertNotIn(ANALOG_SEED, (DT.MGXS_SEED, DT.WINDOW_SEED))
        self.assertNotIn(WINDOWED_SEED, (DT.MGXS_SEED, DT.WINDOW_SEED))
        self.assertNotIn(GENERATION_SEED, (ANALOG_SEED, WINDOWED_SEED))
        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(self.data, work)
            started = time.monotonic()

            # 1. windows from a run of their own, with the reserved window seed
            generation = build_model(openmc, library, GENERATION_RUN, GENERATION_SEED)
            generation.tallies = openmc.Tallies()
            generation.settings.weight_window_generators = [openmc.WeightWindowGenerator(
                mesh=window_mesh(openmc), energy_bounds=ENERGY_BOUNDS, particle_type="neutron", method="magic",
                max_realizations=GENERATION_RUN["batches"], update_interval=1, on_the_fly=True)]
            # One thread: the windows come out bit-for-bit the same each time (a different order of summing the
            # generator's tallies would move the windows slightly and change every later random choice).
            run_in(work / "generate", generation, "openmc.log", threads=1)
            made = sorted((work / "generate").glob("weight_windows*.h5"))
            self.assertTrue(made, "OpenMC's MAGIC generator wrote no weight windows")
            filled = work / "filled.h5"
            s0f_common.fill_file(made[0], filled)
            generated = time.monotonic() - started

            # 2. the analogue run
            analog = build_model(openmc, library, ANALOG_RUN, ANALOG_SEED)
            started = time.monotonic()
            run_in(work / "analog", analog, "openmc.log")
            analog_seconds = time.monotonic() - started
            a, a_error = read_tally(openmc, work / "analog", ANALOG_RUN["batches"])

            # 3. the windowed run, same model and source, other seed
            windowed = build_model(openmc, library, WINDOWED_RUN, WINDOWED_SEED)
            DT.apply_windows(openmc, windowed, filled)
            started = time.monotonic()
            run_in(work / "windowed", windowed, "openmc.log")
            windowed_seconds = time.monotonic() - started
            w, w_error = read_tally(openmc, work / "windowed", WINDOWED_RUN["batches"])

        z = (w - a) / math.hypot(a_error, w_error)
        print(f"\n  analogue {a:.4e} +/- {a_error:.2e} ({a_error / a:.1%}), windowed {w:.4e} +/- {w_error:.2e} "
              f"({w_error / w:.1%}), z = {z:+.2f}\n  seconds: generation {generated:.1f}, analogue "
              f"{analog_seconds:.1f}, windowed {windowed_seconds:.1f}")
        self.assertGreater(a, 0.0)
        self.assertGreater(w, 0.0)
        # the test has power: the errors are small against the 3-sigma band
        self.assertLess(a_error / a, MAX_RELATIVE_ERROR)
        self.assertLess(w_error / w, MAX_RELATIVE_ERROR)
        self.assertLess(abs(z), Z_LIMIT)


if __name__ == "__main__":
    unittest.main()
