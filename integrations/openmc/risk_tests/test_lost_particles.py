"""Lost-particle counting through the real OpenMC library (GEO-025).

A void sphere inside a void shell, with the shell missing on one side of a plane through the centre,
is a deliberate undefined gap: every neutron the point source sends into that half is lost where it
leaves the inner sphere, the other half pass. `design_transport.lost_count` must read the count OpenMC
holds, the count must agree with the geometry (half the histories, within 5 binomial standard
errors; all of them when the shell is missing everywhere; none in the clean model), and
`design_transport.lost_too_many` must fire on it. No nuclear data is read: the cells are void.

The tests skip, with the reason, without OpenMC; with FARIS_REQUIRE_OPENMC=1 (the CI job
`openmc-tests`) a missing OpenMC fails them. Run with the OpenMC 0.15.3 python:

    python3 -m unittest test_lost_particles
"""

import math
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import design_transport as DT  # noqa: E402
import openmc_ci  # noqa: E402

BATCHES, PER_BATCH = 10, 100_000      # 1,000,000 histories: long enough for lost_too_many to judge
HISTORIES = BATCHES * PER_BATCH


def run_model(gap: str) -> tuple[int | None, list[list[float]]]:
    """Run HISTORIES neutrons from a point at the centre of the nested void spheres and return the
    lost count read through lost_count and the positions of the first lost neutrons.
    `gap` is "half" (shell missing for x > 0), "all" (shell nowhere in reach) or "none" (complete shell)."""
    import openmc
    import openmc.lib

    inner = openmc.Sphere(r=5.0)
    outer = openmc.Sphere(r=10.0, boundary_type="vacuum")
    plane = openmc.XPlane(x0=0.0)
    cells = [openmc.Cell(region=-inner)]
    if gap == "half":
        cells.append(openmc.Cell(region=+inner & -outer & -plane))
    elif gap == "none":
        cells.append(openmc.Cell(region=+inner & -outer))
    else:
        # the shell exists only beyond x = 100 cm, outside the outer sphere: no neutron is ever in it
        cells.append(openmc.Cell(region=+inner & -outer & +openmc.XPlane(x0=100.0)))
    model = openmc.Model()
    model.geometry = openmc.Geometry(cells)
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches = BATCHES
    settings.particles = PER_BATCH
    settings.seed = 7
    settings.source = openmc.IndependentSource(
        space=openmc.stats.Point((0.0, 0.0, 0.0)), energy=openmc.stats.Discrete([14.1e6], [1.0]))
    settings.verbosity = 1
    settings.output = {"tallies": False, "summary": False}
    settings.statepoint = {"batches": []}
    # As design_transport.phase_run: lost neutrons are counted here, not aborted on.
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.5
    settings.max_write_lost_particles = DT.LOST_LOCATIONS
    here = os.getcwd()
    with tempfile.TemporaryDirectory() as scratch:
        # OpenMC wants a cross-section file with one entry even when no cell holds material.
        stub = Path(scratch) / "cross_sections.xml"
        stub.write_text('<?xml version="1.0"?>\n<cross_sections>\n  <library materials="H1" path="stub.h5" type="neutron"/>\n</cross_sections>\n')
        os.environ["OPENMC_CROSS_SECTIONS"] = str(stub)
        model.export_to_model_xml(scratch)
        os.chdir(scratch)
        try:
            # OpenMC prints one warning per lost neutron, half a million lines here: send them to a file.
            sys.stdout.flush()
            sys.stderr.flush()
            saved, saved_err = os.dup(1), os.dup(2)
            with open(Path(scratch) / "openmc.log", "w") as log:
                os.dup2(log.fileno(), 1)
                os.dup2(log.fileno(), 2)
                try:
                    openmc.lib.init(["--threads", "2"])
                    try:
                        openmc.lib.run()
                        lost = DT.lost_count(openmc.lib._dll)
                    finally:
                        openmc.lib.finalize()
                finally:
                    os.dup2(saved, 1)
                    os.dup2(saved_err, 2)
                    os.close(saved)
                    os.close(saved_err)
            positions = DT.lost_positions(Path(scratch))
        finally:
            os.chdir(here)
    return lost, positions


class LostParticlesThroughOpenMC(unittest.TestCase):
    def setUp(self):
        openmc_ci.require_modules("numpy", "h5py", "openmc", "openmc.lib")

    # Verifies: GEO-025
    def test_a_half_open_gap_loses_half_the_histories_and_fails_the_run(self):
        lost, positions = run_model("half")
        sigma = math.sqrt(HISTORIES * 0.25)
        print(f"\n  half gap: {lost} of {HISTORIES} lost, expected {HISTORIES // 2} +/- {sigma:.0f}")
        self.assertIsNotNone(lost, "this OpenMC build does not expose n_lost_particles")
        self.assertLess(abs(lost - HISTORIES / 2), 5.0 * sigma)
        # a share far above 1e-6 of a million histories: the run is failed
        self.assertTrue(DT.lost_too_many(lost, HISTORIES))
        # the first LOST_LOCATIONS losses are listed; OpenMC records where each lost neutron was born,
        # which is the point source
        self.assertEqual(positions, [[0.0, 0.0, 0.0]] * DT.LOST_LOCATIONS)

    # Verifies: GEO-025
    def test_a_missing_shell_loses_every_history(self):
        lost, _ = run_model("all")
        self.assertEqual(lost, HISTORIES)
        self.assertTrue(DT.lost_too_many(lost, HISTORIES))

    # Verifies: GEO-025
    def test_a_clean_model_loses_none_and_is_not_failed(self):
        lost, positions = run_model("none")
        self.assertEqual(lost, 0)
        self.assertEqual(positions, [])
        self.assertFalse(DT.lost_too_many(lost, HISTORIES))


if __name__ == "__main__":
    unittest.main()
