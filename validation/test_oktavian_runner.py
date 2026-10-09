# SPDX-License-Identifier: AGPL-3.0-only
import argparse
import importlib.util
import math
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
spec = importlib.util.spec_from_file_location("oktavian_run_case", ROOT / "validation" / "cases" / "oktavian-al" / "run_case.py")
RC = importlib.util.module_from_spec(spec)
spec.loader.exec_module(RC)

UPSTREAM = Path.home() / ".cache/avila-night/validation-data/open-benchmarks"
SAMPLE = """Leakage test
mode   n p
sdef   pos=0 0 0  cel=1  erg=d1
c   Composition 1
m1     13027.41c   0.9975488
\t 14028.41c   0.1329808E-02
c
m2     24050.41c   -0.4
\t 26056.41c   -0.6
c
1  0               (  -3 -8):(8   -1 -6)  imp:n=1
3  1       -1.223  (4 -5 -8):(8 2    -5)  imp:n=1
2  2       -7.824  (3 -4 -8):(8 1 -2 -6)  imp:n=1
f21:n   6
e21       0.1 0.2 0.4
e41       0.5 0.6
c
#           si1             sp1
         1.0  0.0
         2.0  0.25
         4.0  0.75
c
"""


class RunnerHelpers(unittest.TestCase):
    def test_zaid_names(self):
        self.assertEqual(RC.zaid_to_nuclide("13027.41c"), "Al27")
        self.assertEqual(RC.zaid_to_nuclide("24050"), "Cr50")
        self.assertEqual(RC.zaid_to_nuclide("1001.80c"), "H1")
        with self.assertRaises(SystemExit):
            RC.zaid_to_nuclide("x")

    def test_parse_sample_materials_and_source(self):
        p = RC.parse_mcnp(SAMPLE)
        self.assertEqual(p["materials"][1]["basis"], "ao")
        self.assertEqual(p["materials"][2]["basis"], "wo")
        self.assertEqual(p["materials"][2]["nuclides"], [("Cr50", 0.4), ("Fe56", 0.6)])
        self.assertEqual(p["mass_density_g_cm3"], {1: 1.223, 2: 7.824})
        self.assertEqual(p["source_bounds_mev"], [1.0, 2.0, 4.0])
        self.assertEqual(p["e21_mev"], [0.1, 0.2, 0.4])

    def test_histogram_conversion_keeps_bin_probabilities(self):
        x, density = RC.histogram_for_openmc([1.0, 2.0, 4.0], [0.0, 0.25, 0.75])
        self.assertEqual(x, [1.0e6, 2.0e6, 4.0e6])
        self.assertAlmostEqual(density[0] * (x[1] - x[0]), 0.25)
        self.assertAlmostEqual(density[1] * (x[2] - x[1]), 0.75)
        self.assertEqual(density[-1], 0.0)
        with self.assertRaises(SystemExit):
            RC.histogram_for_openmc([1.0, 2.0], [0.5, 0.5])

    def test_shell_dilution_and_unit_conversion(self):
        self.assertAlmostEqual(RC.shell_dilution(10.0, 1e-9), 1.0, places=6)
        r1, t = 19.95, 0.2
        self.assertAlmostEqual(RC.shell_dilution(r1, t), 3 * r1**2 * t / ((r1 + t) ** 3 - r1**3))
        volume = 4 / 3 * math.pi * ((r1 + t) ** 3 - r1**3)
        # A flux phi at r1 spreading as 1/r^2: track length in the shell is phi * mean-factor * volume.
        phi = 2.0e-3
        track = phi * RC.shell_dilution(r1, t) * volume
        value, err = RC.to_manifest_units(track, 0.1 * track, volume, RC.shell_dilution(r1, t), 4 * math.pi * 19.5**2, 0.04)
        self.assertAlmostEqual(value, phi * 4 * math.pi * 19.5**2 / 0.04)
        self.assertAlmostEqual(err, 0.1 * value)

    def test_run_refuses_without_the_cpu_flag(self):
        args = argparse.Namespace(i_have_the_cpu=False)
        with self.assertRaises(SystemExit) as ctx:
            RC.command_run(args)
        self.assertIn("--i-have-the-cpu", str(ctx.exception))

    def test_parser_requires_the_flag_only_for_run_and_defaults_are_bounded(self):
        args = RC.parser().parse_args(["run", "--upstream", "u", "--work-dir", "w"])
        self.assertFalse(args.i_have_the_cpu)
        self.assertEqual((args.particles, args.batches, args.seed), (RC.DEFAULT_PARTICLES, RC.DEFAULT_BATCHES, RC.DEFAULT_SEED))
        self.assertFalse(hasattr(RC.parser().parse_args(["plan", "--upstream", "u"]), "i_have_the_cpu"))

    # Verifies: VAL-036
    def test_run_record_is_sealed_as_a_faris_run(self):
        from validation.scoring import check_run_record
        rec = RC.build_run_record("r1", {"library_sha256": "a" * 64, "code_version": "openmc-0.15.3", "adapter_sha256": "b" * 64}, [], {})
        self.assertEqual(rec["kind"], "faris-run")
        self.assertEqual(check_run_record(rec, "oktavian-al").code_version, "openmc-0.15.3")

    @unittest.skipUnless((UPSTREAM / ".git").exists(), "upstream checkout not present")
    def test_parses_the_published_mcnp_input(self):
        p = RC.parse_mcnp((UPSTREAM / RC.INPUTS / "mcnp" / "Oktavian_Al.i").read_text(encoding="utf-8"))
        self.assertEqual(len(p["materials"][1]["nuclides"]), 10)
        self.assertEqual(len(p["materials"][2]["nuclides"]), 13)
        self.assertEqual(p["materials"][2]["basis"], "wo")
        self.assertEqual((len(p["e21_mev"]), len(p["e41_mev"])), (134, 57))
        self.assertEqual(len(p["source_bounds_mev"]), 52)
        self.assertAlmostEqual(sum(p["source_sp"]), 1.0, delta=0.01)  # MCNP renormalises
        self.assertEqual(p["mass_density_g_cm3"], {1: 1.223, 2: 7.824})


if __name__ == "__main__":
    unittest.main()
