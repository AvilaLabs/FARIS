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

try:
    import openmc as OPENMC_PY
except ImportError:
    OPENMC_PY = None

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

    def test_unit_conversion_divides_by_the_width_only(self):
        value, err = RC.to_manifest_units(2.0e-3, 1.0e-4, 0.04)
        self.assertAlmostEqual(value, 0.05)
        self.assertAlmostEqual(err, 2.5e-3)
        value, err = RC.to_manifest_units(2.0e-3, 1.0e-4, 0.5)  # photon bin of 0.5 MeV
        self.assertAlmostEqual(value, 4.0e-3)
        self.assertAlmostEqual(err, 2.0e-4)
        self.assertFalse(hasattr(RC, "shell_dilution"))

    def test_partial_and_net_agreement_check(self):
        ok = RC.partial_net_agreement([1.0, 2.0, 0.0], [0.1, 0.1, 0.0], [1.1, 1.9, 0.0], [0.1, 0.1, 0.0])
        self.assertTrue(ok["agree"])
        self.assertEqual(ok["bins_exceeding"], [])
        # 0.5 apart with combined sd 0.1414 is 3.54 sigma: beyond the limit, reported by bin
        bad = RC.partial_net_agreement([1.0, 2.0], [0.1, 0.1], [1.0, 2.5], [0.1, 0.1])
        self.assertFalse(bad["agree"])
        self.assertEqual(bad["bins_exceeding"], [1])
        self.assertAlmostEqual(bad["worst_sigma"], 0.5 / math.hypot(0.1, 0.1))
        self.assertIn("outside", bad["note"])
        # exactly 3 sigma still agrees; a difference with no uncertainty never does
        edge = RC.partial_net_agreement([0.0], [3.0], [15.0], [4.0])
        self.assertTrue(edge["agree"])
        self.assertFalse(RC.partial_net_agreement([1.0], [0.0], [1.5], [0.0])["agree"])
        with self.assertRaises(SystemExit):
            RC.partial_net_agreement([1.0], [0.1], [1.0, 2.0], [0.1, 0.1])

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
    @unittest.skipUnless(OPENMC_PY, "OpenMC not importable")
    def test_model_tallies_the_current_on_surface_6_with_no_extra_shell(self):
        openmc = OPENMC_PY
        parsed = RC.parse_mcnp((UPSTREAM / RC.INPUTS / "mcnp" / "Oktavian_Al.i").read_text(encoding="utf-8"))
        args = argparse.Namespace(particles=1000, batches=10, seed=1)
        model, inside = RC.build_model(openmc, UPSTREAM, parsed, args)
        self.assertEqual(sorted(c.id for c in model.geometry.get_all_cells().values()), [1, 2, 3, 4, 5, 6])  # no shell cell
        self.assertEqual(sorted(c.id for c in inside), [1, 2, 4])  # the cells that reach surface 6 from inside
        self.assertEqual(sorted(s for s in model.geometry.get_all_surfaces()), [1, 2, 3, 4, 5, 6, 7, 8])  # no extra sphere
        self.assertEqual(model.geometry.get_all_cells()[5].region.get_surfaces().keys(), {6, 7})  # void keeps its published region
        tallies = {t.name: t for t in model.tallies}
        self.assertEqual(sorted(tallies), ["neutron_net_current", "neutron_partial_current", "photon_net_current", "photon_partial_current"])
        for name, t in tallies.items():
            self.assertEqual(t.scores, ["current"])
            kinds = [type(f).__name__ for f in t.filters]
            self.assertEqual(kinds.count("SurfaceFilter"), 1)
            surf = next(f for f in t.filters if type(f).__name__ == "SurfaceFilter")
            self.assertEqual([int(b) for b in surf.bins], [6])
            self.assertEqual("CellFromFilter" in kinds, name.endswith("partial_current"))
        edges = next(f for f in tallies["neutron_partial_current"].filters if type(f).__name__ == "EnergyFilter").values
        self.assertEqual(len(edges), 135)  # 134 e21 edges plus 0
        photon_edges = next(f for f in tallies["photon_net_current"].filters if type(f).__name__ == "EnergyFilter").values
        self.assertEqual(len(photon_edges), 58)

    @unittest.skipUnless((UPSTREAM / ".git").exists(), "upstream checkout not present")
    def test_csv_integrals_support_the_per_source_neutron_current_basis(self):
        import csv
        exp = UPSTREAM / "jade_open_benchmarks/exp_results/Oktavian"
        def table(name):
            return [(float(r["Energy"]), float(r["Value"])) for r in csv.DictReader((exp / name).read_text(encoding="utf-8").splitlines())]
        n = table("Oktavian_Al Neutron flux.csv")
        edges = [0.097122] + [e for e, _ in n[:-1]]
        neutrons = sum(v * math.log(hi / lo) for lo, (hi, v) in zip(edges, n))
        self.assertAlmostEqual(neutrons, 0.942, places=3)
        g = table("Oktavian_Al Gamma flux.csv")
        upper = sum(g[i][1] * (g[i][0] - g[i - 1][0]) for i in range(1, len(g)))  # CSV energy as the upper edge, first bin dropped
        lower = sum(g[i][1] * ((g[i + 1][0] if i + 1 < len(g) else g[i][0] + 0.5) - g[i][0]) for i in range(len(g)))
        self.assertAlmostEqual(upper, 0.372, places=3)
        self.assertAlmostEqual(lower, 0.399, places=3)

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
