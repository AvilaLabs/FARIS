# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""TIARA OpenMC adapter checks: manifest, source, CSG and tallies without transport."""
from __future__ import annotations

import importlib.util
import math
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
ADAPTER_DIR = ROOT / "validation" / "cases" / "tiara-bc-68mev"
UPSTREAM = Path.home() / ".cache/avila-night/validation-data/open-benchmarks"
RUNNER_SPEC = importlib.util.spec_from_file_location("tiara_openmc_runner", ADAPTER_DIR / "run_case.py")
RUNNER = importlib.util.module_from_spec(RUNNER_SPEC)
assert RUNNER_SPEC and RUNNER_SPEC.loader
sys.path.insert(0, str(ROOT))
sys.modules[RUNNER_SPEC.name] = RUNNER
RUNNER_SPEC.loader.exec_module(RUNNER)
BUILDER_SPEC = importlib.util.spec_from_file_location("tiara_manifest_builder", ADAPTER_DIR / "build_manifest.py")
BUILDER = importlib.util.module_from_spec(BUILDER_SPEC)
assert BUILDER_SPEC and BUILDER_SPEC.loader
sys.modules[BUILDER_SPEC.name] = BUILDER
BUILDER_SPEC.loader.exec_module(BUILDER)

try:
    import openmc
except ImportError:
    openmc = None


def deck(case_name: str) -> str:
    path = (UPSTREAM / "jade_open_benchmarks" / "inputs" / "Tiara-BC" / case_name / "mcnp" / f"{case_name}.i")
    return path.read_text(encoding="utf-8")


class TiaraSourceAndTally(unittest.TestCase):
    @unittest.skipUnless(UPSTREAM.is_dir(), "pinned local IAEA/JADE checkout not present")
    def test_both_decks_retain_full_energy_source_and_all_e14_bins(self):
        for name in ("Tiara-BC_fe-68-40-00", "Tiara-BC_cc-68-100-00"):
            with self.subTest(case=name):
                parsed = RUNNER.parse_deck(deck(name))
                self.assertEqual(len(parsed["energy_edges_mev"]), 69)
                self.assertEqual(parsed["energy_edges_mev"][-1], 72.5)
                self.assertEqual(len(parsed["energy_weights"]), 68)
                self.assertAlmostEqual(parsed["energy_weight_sum"], 2.636514, places=6)
                self.assertEqual(parsed["tally_cell"], 109)
                self.assertEqual(len(parsed["tally_edges_mev"]), 52)
                self.assertEqual(parsed["tally_edges_mev"][0], 6.0)
                self.assertEqual(parsed["tally_edges_mev"][-1], 70.0)
                self.assertEqual(parsed["fm14"], 1.31126e11)
                self.assertEqual(parsed["sb_weights"], [0.0, 0.0, 1.0])

    @unittest.skipUnless(UPSTREAM.is_dir(), "pinned local IAEA/JADE checkout not present")
    def test_physical_angular_distribution_is_isotropic_and_sb_is_separate(self):
        parsed = RUNNER.parse_deck(deck("Tiara-BC_fe-68-40-00"))
        self.assertEqual(parsed["mu_edges"], [-1.0, 0.99990531, 1.0])
        self.assertAlmostEqual(sum(parsed["mu_weights"]), 1.0)
        self.assertAlmostEqual(parsed["mu_weights"][0], (0.99990531 + 1.0) / 2.0, places=7)
        self.assertAlmostEqual(parsed["mu_weights"][1], (1.0 - 0.99990531) / 2.0, places=7)

    @unittest.skipUnless(UPSTREAM.is_dir(), "pinned local IAEA/JADE checkout not present")
    def test_each_published_nonblank_spectrum_point_is_retained_on_the_deck_energy_grid(self):
        from validation.manifest import load_manifest
        for case_id, data in BUILDER.CASES.items():
            with self.subTest(case=case_id):
                manifest = load_manifest(ROOT / "validation" / "cases" / case_id / "manifest.json")
                name = data[0]
                table_path = UPSTREAM / "jade_open_benchmarks" / "exp_results" / "Tiara-BC" / f"{name} Neutron yield.csv"
                rows = BUILDER.parse_table(table_path.read_bytes())
                self.assertEqual([d["controlling_variable"]["value"] for d in manifest["detectors"]],
                                 [row["energy_mev"] for row in rows])
                for detector, row in zip(manifest["detectors"], rows):
                    if row["value"] is not None and not (row["value"] == 0.0 and row["absolute_u"] == 0.0):
                        self.assertEqual(detector["reference"], {"value": row["value"], "u": row["absolute_u"]})
                    else:
                        self.assertNotIn("reference", detector)

                deck_text = deck(name)
                parsed = RUNNER.parse_deck(deck_text)
                self.assertEqual([d["controlling_variable"]["value"] for d in manifest["detectors"]],
                                 parsed["tally_edges_mev"])

    @unittest.skipUnless(UPSTREAM.is_dir(), "pinned local IAEA/JADE checkout not present")
    @unittest.skipUnless(openmc is not None, "OpenMC Python API not importable")
    def test_actual_openmc_models_translate_points_materials_source_and_tally_without_solver(self):
        import numpy as np
        cases = (("Tiara-BC_fe-68-40-00", 447.35, 7.87, 7.08),
                 ("Tiara-BC_cc-68-100-00", 507.35, 2.31, 7.08))
        for name, detector_x, shield_density, filler_density in cases:
            with self.subTest(case=name):
                model, parsed, points = RUNNER.build_model(openmc, deck(name))
                self.assertEqual(points, {"source_cell": 600, "detector_cell": 109})
                cells = model.geometry.get_all_cells()
                self.assertEqual(cells[600].id, 600)
                self.assertEqual(cells[109].id, 109)
                self.assertEqual(cells[110].fill.density, shield_density)
                self.assertEqual(cells[102].fill.density, filler_density)
                self.assertEqual(cells[101].fill.density, 7.87)
                self.assertNotIn("C0", [name for name, _, _ in cells[109].fill.nuclides])
                self.assertIn("C12", [name for name, _, _ in cells[109].fill.nuclides])
                self.assertIn("C13", [name for name, _, _ in cells[109].fill.nuclides])
                self.assertEqual(model.geometry.find((0.0, 0.0, 0.0))[-1].id, 600)
                self.assertEqual(model.geometry.find((200.0, 15.0, 9.08))[-1].id, 103)
                self.assertEqual(model.geometry.find((0.0, 200.0, 100.0))[-1].id, 100)
                self.assertEqual(model.geometry.find((420.0, 0.0, 0.0))[-1].id, 110)
                self.assertEqual(model.geometry.find((detector_x, 0.0, 0.0))[-1].id, 109)
                self.assertEqual(model.geometry.find((700.0, 0.0, 0.0))[-1].id, 700)
                self.assertEqual(model.geometry.find((900.0, 0.0, 0.0))[-1].id, 601)

                source = model.settings.source[0]
                self.assertEqual(source.strength, 1.31126e11)
                self.assertEqual(source.particle, "neutron")
                self.assertEqual(tuple(source.space.xyz), (0.0, 0.0, 0.0))
                self.assertEqual(source.energy.interpolation, "histogram")
                self.assertEqual(len(source.energy.x), 69)
                integral = math.fsum(p * (hi - lo) for lo, hi, p in
                                     zip(source.energy.x, source.energy.x[1:], source.energy.p))
                self.assertAlmostEqual(integral, 1.0)
                np.testing.assert_allclose(source.angle.reference_uvw, (1.0, 0.0, 0.0))
                self.assertEqual(source.angle.mu.a, -1.0)
                self.assertEqual(source.angle.mu.b, 1.0)

                tally = model.tallies[0]
                self.assertEqual(tally.scores, ["flux"])
                self.assertEqual(len(tally.filters), 2)
                self.assertEqual([int(x) for x in tally.filters[0].bins], [109])
                self.assertEqual(len(tally.filters[1].values), 53)
                self.assertEqual(tally.filters[1].values[-1], 70e6)
                self.assertEqual(len(parsed["tally_edges_mev"]), 52)
                with tempfile.TemporaryDirectory() as tmp:
                    model.export_to_xml(directory=tmp)
                    self.assertTrue((Path(tmp) / "geometry.xml").is_file())
                    self.assertTrue((Path(tmp) / "materials.xml").is_file())
                    self.assertTrue((Path(tmp) / "settings.xml").is_file())
                    self.assertTrue((Path(tmp) / "tallies.xml").is_file())

    def test_lethargy_mapping_preserves_rows_and_blocks_zero_lower_bound(self):
        class Flattenable(list):
            def ravel(self):
                return self

        parsed = {"tally_edges_mev": [6.0, 7.0, 8.0]}
        tally = SimpleNamespace(mean=Flattenable([12.0, 5.0, 6.0]), std_dev=Flattenable([1.2, 0.5, 0.6]))
        rows = RUNNER.score_from_tally(tally, parsed)
        self.assertEqual(len(rows), 3)
        self.assertEqual([row["detector_id"] for row in rows], ["n-6", "n-7", "n-8"])
        self.assertIsNone(rows[0]["value"])
        self.assertIsNone(rows[0]["u_mc"])
        self.assertAlmostEqual(rows[1]["value"], 5.0 / math.log(7.0 / 6.0))
        self.assertAlmostEqual(rows[2]["u_mc"], 0.6 / math.log(8.0 / 7.0))

    @unittest.skipUnless(openmc is not None, "OpenMC Python API not importable")
    def test_energy_range_preflight_does_not_extrapolate_short_data(self):
        fake_library = SimpleNamespace(libraries=[{"type": "neutron", "materials": "Fe54", "path": "Fe54.h5"}])
        import h5py
        import numpy as np

        with tempfile.TemporaryDirectory() as tmp:
            okay_h5 = Path(tmp) / "Fe54-ok.h5"
            short_h5 = Path(tmp) / "Fe54-short.h5"
            for path, maximum in ((okay_h5, 72.5e6), (short_h5, 20e6)):
                with h5py.File(path, "w") as h5:
                    energy = h5.create_group("Fe54").create_group("energy")
                    energy.create_dataset("293K", data=np.array([0.0, maximum]))
            fake_library = SimpleNamespace(libraries=[{"type": "neutron", "materials": ["Fe54"], "path": str(okay_h5)}])
            with patch.object(openmc.data.DataLibrary, "from_xml", return_value=fake_library):
                result = RUNNER.check_xs_coverage(openmc, Path("cross_sections.xml"), ["Fe54"])
                self.assertTrue(result["complete"])
            fake_library.libraries[0]["path"] = str(short_h5)
            with patch.object(openmc.data.DataLibrary, "from_xml", return_value=fake_library):
                result = RUNNER.check_xs_coverage(openmc, Path("cross_sections.xml"), ["Fe54"])
                self.assertFalse(result["complete"])
                self.assertEqual(result["short_energy_tables"][0]["nuclide"], "Fe54")


if __name__ == "__main__":
    unittest.main()
