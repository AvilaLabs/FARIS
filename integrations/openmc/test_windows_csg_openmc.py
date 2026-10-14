"""Weight windows on a CSG model do not change the answer (PERF-025, NUC-043, NUC-051).

The reactor study's Production tier runs with windows that `reactor_windows.py` makes on its CSG torus. This
checks the same path on a small CSG case: concentric spheres of iron, a 14.1 MeV point source in the core and the
fast (> 0.1 MeV) flux in the outer shell. One run is analog; the other reads weight windows through the worker's
own `windows_path`, `apply_windows` and `add_mesh_to_settings` (reactor_transport.py, OpenMC 0.15.3) and exports
the separate XML files the worker exports. The two means must agree within 3 combined standard errors, and both
errors must be small enough for that to mean something.

Two tests differ in where the windows come from:

- OpenMC's own MAGIC generator in the same OpenMC 0.15.3: this runs in CI.
- The generator the study uses (window_generator.py, random-ray FW-CADIS, OpenMC 0.16 or later) in a second
  interpreter. CI has no OpenMC 0.16, so this test skips there, with the reason, even when FARIS_REQUIRE_OPENMC=1.
  Locally set FARIS_WINDOW_PYTHON to a Python with OpenMC 0.16 (or leave the micromamba `openmc016`
  environment where it is).

Nuclear data is the H1 and Fe56 files of FARIS_CI_NEUTRON_DATA (see openmc_ci.py). Run with the OpenMC 0.15.3
python:

    FARIS_CI_NEUTRON_DATA=<folder> python3 -m unittest test_windows_csg_openmc
"""

import importlib.util
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import openmc_ci

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "risk_tests"))
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)
import window_generator as WG  # noqa: E402

ANALOG_SEED, WINDOWED_SEED = 303, 404
ANALOG_RUN = {"batches": 40, "particles": 10000}
WINDOWED_RUN = {"batches": 40, "particles": 4000}
CORE_CM, IRON_CM, SHELL_CM = 2.0, 22.0, 26.0
ENERGY_BOUNDS = [0.0, 1.0e5, 2.0e7]
TALLY = f"response-{WORKER.STOP_RULE_RESPONSE_ID}"
Z_LIMIT = 3.0
MAX_RELATIVE_ERROR = 0.05
THREADS = 2

# The window step in the OpenMC 0.16 interpreter: the same model, built there, through window_generator.
WINDOW_SCRIPT = r"""
import sys
from pathlib import Path
import openmc

sys.path.insert(0, sys.argv[1])
sys.path.insert(0, str(Path(sys.argv[1]) / "risk_tests"))
import window_generator as WG

xml, work = sys.argv[2], Path(sys.argv[3])
core_cm, iron_cm, shell_cm = (float(v) for v in sys.argv[4:7])


def build():
    iron = openmc.Material(name="iron")
    iron.add_nuclide("Fe56", 1.0, percent_type="ao")
    iron.set_density("g/cm3", 7.874)
    radii = [openmc.Sphere(r=r) for r in (core_cm, iron_cm, shell_cm)]
    radii[-1].boundary_type = "vacuum"
    cells = [openmc.Cell(fill=iron, region=-radii[0]), openmc.Cell(fill=iron, region=+radii[0] & -radii[1]),
             openmc.Cell(fill=iron, region=+radii[1] & -radii[2])]
    model = openmc.Model(geometry=openmc.Geometry(cells), materials=openmc.Materials([iron]))
    model.materials.cross_sections = xml
    model.settings.run_mode = "fixed source"
    return model, cells


model, cells = build()
edges = WG.group_edges(1.0e5)
# light settings: a rough library and few rays make rough windows, which is all this test needs
WG.RANDOM_RAY.update(rays=5000, batches=40, inactive=20)
WG.condense_multigroup(openmc, model, edges, WG.domain_source(openmc, cells[0]), work / "mgxs", 2,
                       dict(WG.MGXS, nparticles=3000, batches=30))
model, cells = build()
box = [-shell_cm - 1.0] * 3, [shell_cm + 1.0] * 3
WG.solve_windows(openmc, model, model.geometry.root_universe, WG.domain_source(openmc, cells[0]), [cells[2]], 1.0e5,
                 WG.window_grid(*box, cell_cm=4.0), work / "mgxs" / "mgxs.h5", work / "windows", 2)
"""


def window_python() -> str:
    """A Python with OpenMC 0.16 for the study's generator, or a skip (never a failure: CI has none)."""
    found = os.environ.get("FARIS_WINDOW_PYTHON") or str(Path.home() / "micromamba/envs/openmc016/bin/python")
    if not Path(found).is_file():
        raise unittest.SkipTest("no Python with OpenMC 0.16: set FARIS_WINDOW_PYTHON (CI has none, so this test skips there)")
    return found


def build_model(openmc, library: Path, run: dict, seed: int):
    iron = openmc.Material(name="iron")
    iron.add_nuclide("Fe56", 1.0, percent_type="ao")
    iron.set_density("g/cm3", 7.874)
    radii = [openmc.Sphere(r=r) for r in (CORE_CM, IRON_CM, SHELL_CM)]
    radii[-1].boundary_type = "vacuum"
    cells = [openmc.Cell(fill=iron, region=-radii[0]), openmc.Cell(fill=iron, region=+radii[0] & -radii[1]),
             openmc.Cell(fill=iron, region=+radii[1] & -radii[2])]
    model = openmc.Model(geometry=openmc.Geometry(cells), materials=openmc.Materials([iron]))
    model.materials.cross_sections = str(library)
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches, settings.particles, settings.seed = run["batches"], run["particles"], seed
    settings.source = openmc.IndependentSource(
        energy=openmc.stats.Discrete([14.1e6], [1.0]), particle="neutron", constraints={"domains": [cells[0]]})
    settings.output = {"summary": False}
    tally = openmc.Tally(name=TALLY)
    tally.filters = [openmc.CellFilter([cells[2]]), openmc.ParticleFilter("neutron"), openmc.EnergyFilter([1.0e5, 2.0e7])]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])
    return model, cells


def run_in(openmc, directory: Path, model, windows: Path | None = None) -> tuple[float, float]:
    """Export the way the worker does (separate XML files, the window mesh put in the settings), run, and read the
    fast flux in the outer shell: (mean, standard error) per source neutron."""
    directory.mkdir(parents=True, exist_ok=True)
    if windows is not None:
        solver = directory / "solver"
        solver.mkdir()
        mesh = WORKER.apply_windows(openmc, model, WORKER.windows_path(windows_plan(windows), solver), solver)
        model.export_to_xml(directory=str(solver))
        WORKER.add_mesh_to_settings(solver / "settings.xml", mesh)
        directory = solver
    else:
        model.export_to_xml(directory=str(directory))
    openmc.run(cwd=str(directory), threads=THREADS, output=False,
               openmc_exec=str(Path(sys.executable).parent / "openmc"))
    with openmc.StatePoint(str(directory / f"statepoint.{model.settings.batches}.h5")) as state:
        tally = state.get_tally(name=TALLY)
        return float(tally.mean.ravel()[0]), float(tally.std_dev.ravel()[0])


def windows_plan(windows: Path) -> dict:
    """The input's plan for the file WINDOWS, which must lie beside the solver directory."""
    return {"kind": "weight_windows", "response_id": WORKER.STOP_RULE_RESPONSE_ID,
            "windows_file": windows.name, "windows_sha256": WORKER.sha256(windows)}


def agree(testcase, analog, windowed):
    (a, a_error), (w, w_error) = analog, windowed
    z = (w - a) / math.hypot(a_error, w_error)
    print(f"\n  analog {a:.4e} +/- {a_error:.2e} ({a_error / a:.1%}), windowed {w:.4e} +/- {w_error:.2e} "
          f"({w_error / w:.1%}), z = {z:+.2f}")
    testcase.assertGreater(a, 0.0)
    testcase.assertGreater(w, 0.0)
    testcase.assertLess(a_error / a, MAX_RELATIVE_ERROR)
    testcase.assertLess(w_error / w, MAX_RELATIVE_ERROR)
    testcase.assertLess(abs(z), Z_LIMIT)


class WindowsOnCsgDoNotBiasTheAnswer(unittest.TestCase):
    def setUp(self):
        openmc_ci.require_modules("numpy", "h5py", "openmc")
        self.data = openmc_ci.neutron_data_dir()

    def analog_and_windowed(self, work: Path, library: Path, windows: Path):
        import warnings

        import openmc

        warnings.simplefilter("ignore", openmc.IDWarning)
        model, _ = build_model(openmc, library, ANALOG_RUN, ANALOG_SEED)
        analog = run_in(openmc, work / "analog", model)
        # the file lies in the run directory, beside the solver directory, as the engine places it
        run = work / "windowed"
        run.mkdir()
        placed = run / "windows.h5"
        placed.write_bytes(windows.read_bytes())
        model, _ = build_model(openmc, library, WINDOWED_RUN, WINDOWED_SEED)
        windowed = run_in(openmc, run, model, placed)
        agree(self, analog, windowed)

    # Verifies: NUC-051, PERF-025
    def test_windows_from_openmc_magic_agree_with_analog(self):
        import openmc

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(self.data, work)
            model, cells = build_model(openmc, library, {"batches": 30, "particles": 2000}, WG.WINDOW_SEED)
            model.tallies = openmc.Tallies()
            mesh = openmc.RegularMesh(mesh_id=7, name="windows")
            mesh.lower_left, mesh.upper_right, mesh.dimension = [-27.0] * 3, [27.0] * 3, [14, 14, 14]
            model.settings.weight_window_generators = [openmc.WeightWindowGenerator(
                mesh=mesh, energy_bounds=ENERGY_BOUNDS, particle_type="neutron", method="magic", max_realizations=30,
                update_interval=1, on_the_fly=True)]
            (work / "generate").mkdir()
            model.export_to_xml(directory=str(work / "generate"))
            openmc.run(cwd=str(work / "generate"), threads=1, output=False,
                       openmc_exec=str(Path(sys.executable).parent / "openmc"))
            made = sorted((work / "generate").glob("weight_windows*.h5"))
            self.assertTrue(made, "OpenMC's MAGIC generator wrote no weight windows")
            import s0f_common

            filled = work / "filled.h5"
            s0f_common.fill_file(made[0], filled)
            WG.label_windows(filled)
            self.analog_and_windowed(work, library, filled)

    # Verifies: NUC-051, PERF-025
    def test_windows_from_the_studys_generator_agree_with_analog(self):
        python = window_python()
        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(self.data, work)
            made = subprocess.run(
                [python, "-c", WINDOW_SCRIPT, str(HERE), str(library), str(work / "made"), str(CORE_CM),
                 str(IRON_CM), str(SHELL_CM)],
                capture_output=True, text=True, env=dict(os.environ, OMP_NUM_THREADS="2", OPENMC_CROSS_SECTIONS=str(library)))
            self.assertEqual(made.returncode, 0, made.stderr[-2000:])
            filled = work / "made" / "windows" / "filled.h5"
            self.assertTrue(filled.is_file())
            self.analog_and_windowed(work, library, filled)


if __name__ == "__main__":
    unittest.main()
