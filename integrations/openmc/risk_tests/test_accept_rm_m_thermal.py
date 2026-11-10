#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs

import unittest
from types import SimpleNamespace
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import accept_rm_m as acceptance


class FakeTally:
    def __init__(self, name):
        self.name = name
        self.filters = []
        self.scores = []


class FakeFilter:
    def __init__(self, bins):
        self.bins = bins


class FakeEnergyFilter(FakeFilter):
    pass


class FakeStatePoint:
    tallies = {}

    def __init__(self, path):
        self.batch = int(Path(path).name.split(".")[1])

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def get_tally(self, name):
        return self.tallies[(self.batch, name)]


class ThermalDiagnosticTests(unittest.TestCase):
    def test_plan_builds_bounded_material_flux_and_nuclide_capture_tallies(self):
        fake_openmc = SimpleNamespace(Tally=FakeTally, MaterialFilter=FakeFilter,
                                      NuclideFilter=FakeFilter, EnergyFilter=FakeEnergyFilter)
        materials = [SimpleNamespace(id=2, name="steel", get_nuclides=lambda: ["Fe56", "Mn55"]),
                     SimpleNamespace(id=1, name="copper", get_nuclides=lambda: ["Cu63"])]
        tallies, plan = acceptance.thermal_diagnostic_plan(fake_openmc, SimpleNamespace(materials=materials))

        self.assertEqual([t.name for t in tallies], ["thermal_split_material_flux", "thermal_capture_1", "thermal_capture_2"])
        self.assertEqual(tallies[0].scores, ["flux"])
        self.assertEqual(tallies[1].scores, ["102"])
        self.assertEqual(plan["energy_edges_eV"], [0.0, 1000.0, 50000000.0])
        self.assertEqual(plan["reaction_mt"], 102)
        self.assertEqual(plan["tally_bin_count_estimate"], 10)
        self.assertEqual([m["name"] for m in plan["materials"]], ["copper", "steel"])
        self.assertEqual(plan["materials"][1]["nuclides"], ["Fe56", "Mn55"])
        self.assertEqual(len(plan["configuration_sha256"]), 64)

    def test_batch_fraction_uses_aligned_batch_variation_for_se(self):
        summary = acceptance._batch_fraction_summary([1.0, 3.0], [9.0, 7.0])
        self.assertEqual(summary["thermal_fraction"], 0.2)
        self.assertAlmostEqual(summary["standard_error"], 0.1)
        self.assertEqual(summary["status"], "AVAILABLE")
        self.assertIn("covariance retained", summary["uncertainty"])

    def test_batch_fraction_refuses_unavailable_or_zero_total_data(self):
        self.assertIsNone(acceptance._batch_fraction_summary([1.0], [3.0])["standard_error"])
        self.assertEqual(acceptance._batch_fraction_summary([0.0, 0.0], [0.0, 0.0])["status"], "UNAVAILABLE")

    def test_statepoint_reader_uses_energy_then_nuclide_axis_and_numeric_filenames(self):
        try:
            import numpy as np
            import openmc
        except ImportError as exc:
            self.skipTest(f"NumPy and OpenMC 0.15.3 API unavailable: {exc}")
        openmc.reset_auto_ids()

        plan = {"materials": [{"name": "steel", "id": 7, "nuclides": ["Fe56", "Mn55"]}]}
        material = openmc.Material(material_id=7, name="steel")
        material.add_nuclide("Fe56", 0.5, percent_type="ao")
        material.add_nuclide("Mn55", 0.5, percent_type="ao")
        material.set_density("g/cm3", 7.8)

        def actual_tally(name, filters, scores, cumulative, nuclides=None):
            tally = openmc.Tally(name=name)
            tally.filters = filters
            tally.scores = scores
            if nuclides is not None:
                tally.nuclides = nuclides
            tally._sum = np.asarray(cumulative, dtype=float).reshape(tally.shape)
            tally._sp_filename = Path("synthetic-statepoint.h5")
            tally._results_read = True
            return tally

        energy_edges = [0.0, 1000.0, 50000000.0]
        FakeStatePoint.tallies = {
            (1, "thermal_split_material_flux"): actual_tally("thermal_split_material_flux",
                [openmc.MaterialFilter([material]), openmc.EnergyFilter(energy_edges)], ["flux"], [1.0, 9.0], ["total"]),
            (2, "thermal_split_material_flux"): actual_tally("thermal_split_material_flux",
                [openmc.MaterialFilter([material]), openmc.EnergyFilter(energy_edges)], ["flux"], [3.0, 16.0], ["total"]),
            # Raw OpenMC result order is filter bins, then nuclide axis, then score.
            (1, "thermal_capture_7"): actual_tally("thermal_capture_7",
                [openmc.MaterialFilter([material]), openmc.EnergyFilter(energy_edges)], ["102"],
                [[1.0, 2.0], [9.0, 18.0]], ["Fe56", "Mn55"]),
            (2, "thermal_capture_7"): actual_tally("thermal_capture_7",
                [openmc.MaterialFilter([material]), openmc.EnergyFilter(energy_edges)], ["102"],
                [[3.0, 6.0], [16.0, 36.0]], ["Fe56", "Mn55"]),
        }
        fake_openmc = SimpleNamespace(StatePoint=FakeStatePoint)
        with TemporaryDirectory() as temp:
            Path(temp, "statepoint.01.h5").touch()
            Path(temp, "statepoint.2.h5").touch()
            result = acceptance.read_thermal_diagnostic(fake_openmc, Path(temp), 2, 10, plan)

        flux = result["materials"][0]
        self.assertAlmostEqual(flux["flux_thermal_fraction"], 3.0 / 19.0)
        captures = flux["captures_mt102"]
        self.assertEqual([row["nuclide"] for row in captures], ["Fe56", "Mn55"])
        self.assertAlmostEqual(captures[0]["thermal_fraction"], 3.0 / 19.0)
        self.assertAlmostEqual(captures[1]["thermal_fraction"], 6.0 / 42.0)

    def test_openmc_0153_model_and_tally_plan_export_without_transport(self):
        try:
            import openmc
            import rm_m_openmc as RM
        except ImportError as exc:
            self.skipTest(f"OpenMC 0.15.3 API unavailable: {exc}")
        self.assertEqual(openmc.__version__, "0.15.3")
        openmc.reset_auto_ids()
        material = openmc.Material(name="steel")
        material.add_nuclide("Fe56", 0.9, percent_type="ao")
        material.add_nuclide("Mn55", 0.1, percent_type="ao")
        material.set_density("g/cm3", 7.8)
        sphere = openmc.Sphere(r=10.0)
        cell = openmc.Cell(cell_id=1, fill=material, region=-sphere)
        model = openmc.Model(geometry=openmc.Geometry([cell]), materials=openmc.Materials([material]))
        base_tally = RM.tf_fast_flux_tallies(openmc, [cell.id])
        diag_tallies, plan = acceptance.thermal_diagnostic_plan(openmc, model)
        base_id = base_tally.id
        tallies = openmc.Tallies([base_tally, *diag_tallies])
        model.tallies = tallies
        self.assertEqual(base_tally.id, base_id)
        self.assertEqual(len({int(t.id) for t in tallies}), len(tallies))
        self.assertTrue(all(hasattr(t, "nuclides") for t in diag_tallies))
        self.assertFalse(any(type(f).__name__ == "NuclideFilter" for t in diag_tallies for f in t.filters))
        model.settings.seed = 1201
        with TemporaryDirectory() as temp:
            root = Path(temp)
            model_dir, run_dir = root / "model", root / "run"
            model_dir.mkdir()
            run_dir.mkdir()
            (model_dir / "rm_m.h5m").write_bytes(b"test geometry identity")
            (model_dir / "rm_m_roles.json").write_text("{}\n", encoding="utf-8")
            cross_sections = root / "cross_sections.xml"
            cross_sections.write_text("<cross_sections/>\n", encoding="utf-8")
            with patch.object(RM, "FARIS_XS", cross_sections):
                protocol = acceptance.prepare_thermal_protocol(
                    model, model_dir, run_dir, "rm_m.h5m", 14.0, 20, 2, 1, plan)
            model_xml = run_dir / "model.xml"
            materials_xml = run_dir / "thermal-materials.xml"
            self.assertTrue(model_xml.is_file())
            self.assertTrue(materials_xml.is_file())
            self.assertTrue((run_dir / "thermal-diagnostic-protocol.json").is_file())
            self.assertEqual(protocol["sampling"]["histories"], 20)
            self.assertEqual(protocol["tally_configuration"]["tallies"][0]["id"], base_id)
            self.assertGreater(len(plan["configuration_sha256"]), 32)


if __name__ == "__main__":
    unittest.main()
