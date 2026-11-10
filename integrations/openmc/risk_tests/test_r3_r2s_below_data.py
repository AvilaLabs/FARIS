#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Focused tests for the optional same-solve below-data-edge R2S diagnostic."""
from __future__ import annotations

import sys
import unittest
import tempfile
import os
from pathlib import Path
from types import SimpleNamespace
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
try:
    import numpy  # noqa: F401
    import h5py  # noqa: F401
    import openmc  # noqa: F401
    import openmc.deplete  # noqa: F401
    import r3_r2s as R2S  # noqa: E402
    import r3_scenario as SC  # noqa: E402
    import r3_bounded_microxs as BM  # noqa: E402
except ImportError as exc:
    if os.environ.get("FARIS_REQUIRE_OPENMC") == "1":
        raise
    R2S = SC = BM = None


class BelowDataEdgeDiagnosticTests(unittest.TestCase):
    @unittest.skipIf(R2S is None, "R2S helper requires NumPy and pinned OpenMC environment")
    def test_actual_openmc_tally_plan_uses_material_energy_and_mt102_axes(self):
        try:
            import openmc
            import openmc.deplete
        except ImportError:
            self.skipTest("OpenMC API check requires pinned OpenMC environment")
        openmc.reset_auto_ids()
        materials = []
        for material_id, name, nuclides in ((9101, "diag_fe", ("Fe56", "Fe54")), (9102, "diag_li", ("Li6",))):
            material = openmc.Material(material_id=material_id, name=name)
            for nuclide in nuclides:
                material.add_nuclide(nuclide, 1.0 / len(nuclides))
            materials.append(material)
        model = SimpleNamespace(materials=openmc.Materials(materials), settings=openmc.Settings())
        import h5py
        with tempfile.TemporaryDirectory() as data_temp:
            data_root = Path(data_temp)
            neutron_dir = data_root / "neutron"
            neutron_dir.mkdir()
            entries = []
            for nuclide in ("Fe56", "Fe54", "Li6"):
                data_path = neutron_dir / f"{nuclide}.h5"
                with h5py.File(data_path, "w") as handle:
                    root = handle.create_group(nuclide)
                    kts = root.create_group("kTs")
                    temp_name = "294K"
                    kts.create_dataset(temp_name, data=8.617333262145e-5 * R2S.RM.DATA_TEMPERATURE_K)
                    energy = root.create_group("energy")
                    energy.create_dataset(temp_name, data=[1.0e-5, 1.0])
                    reaction = root.create_group("reactions").create_group("reaction_102")
                    reaction.attrs["mt"] = 102
                    reaction_temp = reaction.create_group(temp_name)
                    xs = reaction_temp.create_dataset("xs", data=[1.0, 0.5])
                    xs.attrs["threshold_idx"] = 0
                entries.append({"materials": nuclide, "path": f"neutron/{nuclide}.h5"})
            root = ET.Element("cross_sections")
            ET.SubElement(root, "directory").text = "."
            for attrs in entries:
                ET.SubElement(root, "library", {"type": "neutron", **attrs})
            xs_xml = data_root / "cross_sections.xml"
            ET.ElementTree(root).write(xs_xml, encoding="utf-8", xml_declaration=True)
            tallies, plan = R2S.below_data_edge_diagnostic_plan(openmc, model, xs_xml)
        self.assertEqual([t.name for t in tallies], ["r3_below_data_material_flux",
                                                     "r3_below_data_capture_9101",
                                                     "r3_below_data_capture_9102"])
        self.assertEqual(plan["energy_edges_eV"], [0.0, 1.0e-5, 5.0e7])
        self.assertEqual(plan["transport_cutoff_eV"], 0.0)
        self.assertIn("unresolved", plan["activation_response_below_data_edge"])
        self.assertEqual(tallies[0].nuclides, ["total"])
        self.assertEqual(tallies[0].scores, ["flux"])
        for tally, nuclides in zip(tallies[1:], (["Fe54", "Fe56"], ["Li6"])):
            self.assertEqual(tally.nuclides, nuclides)
            self.assertEqual(tally.scores, ["102"])
            self.assertEqual(len(tally.filters), 2)
            self.assertIsInstance(tally.filters[1], openmc.EnergyFilter)
        self.assertEqual(tallies[1].shape, (2, 2, 1))
        for basis in SC.ACTIVATION_GROUP_OPTIONS[1:]:
            edges = SC.activation_group_edges(openmc, basis, BM.VITAMIN_J_42_EDGES_E_V)
            micro = openmc.deplete.MicroXS(
                __import__("numpy").zeros((1, 1, len(edges) - 1)), ["Fe56"], ["(n,gamma)"])
            self.assertEqual(micro.data.shape, (1, 1, len(edges) - 1))
        mesh_tally = openmc.Tally(name="r3_activation_energy_basis_smoke")
        mesh_tally.filters = [openmc.MaterialFilter(materials), openmc.EnergyFilter(edges)]
        mesh_tally.scores = ["flux"]
        exported = openmc.Model(
            geometry=openmc.Geometry([openmc.Cell(fill=materials[0], region=-openmc.Sphere(r=1.0))]),
            materials=model.materials, settings=model.settings,
            tallies=openmc.Tallies([*tallies, mesh_tally]))
        with tempfile.TemporaryDirectory() as temp:
            model_xml = Path(temp) / "model.xml"
            materials_xml = Path(temp) / "materials.xml"
            exported.materials.export_to_xml(path=materials_xml)
            material_xml_bytes = materials_xml.read_bytes()
            self.assertTrue(materials_xml.is_file())
            exported.export_to_model_xml(str(model_xml))
            self.assertTrue(model_xml.is_file())
            self.assertTrue(materials_xml.is_file())
            self.assertEqual(materials_xml.read_bytes(), material_xml_bytes)

    @unittest.skipIf(R2S is None, "R2S helper requires NumPy and pinned OpenMC environment")
    def test_neutron_only_completion_does_not_claim_activation_or_dose(self):
        record = R2S.mark_neutron_only_complete({"steps": {"neutron_transport": {"ok": True}}})
        self.assertEqual(record["workflow_status"], "NEUTRON_STAGE_COMPLETE")
        self.assertEqual(record["scientific_interpretation_status"], "NOT_EVALUATED")
        self.assertEqual(record["activation_status"], "NOT_RUN")
        self.assertEqual(record["microxs_status"], "NOT_RUN")
        self.assertEqual(record["photon_transport_status"], "NOT_RUN")
        self.assertEqual(record["shutdown_dose_status"], "NOT_RUN")
        self.assertIn("no activation", record["qualification_verdict"])

    @unittest.skipIf(R2S is None, "R2S helper requires NumPy and pinned OpenMC environment")
    def test_summary_retains_batches_and_uses_paired_fraction_statistics(self):
        plan = {"materials": [{"name": "m", "id": 4, "nuclides": ["Fe56", "Li6"]}],
                "flux_tally": "flux", "tally_ids": {"flux": 10, "r3_below_data_capture_4": 11}}
        cumulative = {"10": [[2.0, 8.0], [5.0, 19.0]],
                      "11": [[3.0, 4.0, 7.0, 8.0], [8.0, 9.0, 22.0, 23.0]]}
        result = R2S.summarize_below_data_edge_batches(plan, cumulative, 2, 50)
        row = result["materials"][0]
        self.assertEqual(result["raw_batch_cumulative_by_tally"], cumulative)
        self.assertEqual(row["raw_batch_flux_increments_below_and_above"],
                         {"below": [2.0, 3.0], "above": [8.0, 11.0]})
        self.assertAlmostEqual(row["flux_below_vs_tallied_fraction"]["fraction"], 5.0 / 24.0)
        self.assertEqual(row["flux_below_vs_tallied_fraction"]["status"], "AVAILABLE")
        self.assertAlmostEqual(row["flux_below_vs_tallied_fraction"]["standard_error"], 1.0 / 144.0)
        capture = row["captures_mt102"][0]
        self.assertEqual(capture["raw_batch_increments_below_and_above"],
                         {"below": [3.0, 5.0], "above": [7.0, 15.0]})
        self.assertAlmostEqual(capture["below_vs_tallied_capture_fraction"]["fraction"], 8.0 / 30.0)
        second = row["captures_mt102"][1]
        self.assertEqual(second["raw_batch_increments_below_and_above"],
                         {"below": [4.0, 5.0], "above": [8.0, 15.0]})

    @unittest.skipIf(R2S is None, "R2S helper requires NumPy and pinned OpenMC environment")
    def test_empty_energy_bin_keeps_observed_fraction_but_marks_uncertainty_unresolved(self):
        low_empty = R2S._paired_energy_fraction([0.0, 0.0, 0.0], [2.0, 3.0, 1.0])
        self.assertEqual(low_empty["fraction"], 0.0)
        self.assertIsNone(low_empty["standard_error"])
        self.assertEqual(low_empty["status"], "UNRESOLVED_ZERO_OBSERVED_LOW_BIN")
        self.assertIn("does not bound", low_empty["reason"])

        high_empty = R2S._paired_energy_fraction([2.0, 0.0, 3.0], [0.0, 0.0, 0.0])
        self.assertEqual(high_empty["fraction"], 1.0)
        self.assertIsNone(high_empty["standard_error"])
        self.assertEqual(high_empty["status"], "UNRESOLVED_ZERO_OBSERVED_HIGH_BIN")
        self.assertIn("unresolved", high_empty["reason"])

        no_events = R2S._paired_energy_fraction([0.0, 0.0], [0.0, 0.0])
        self.assertIsNone(no_events["fraction"])
        self.assertEqual(no_events["status"], "UNAVAILABLE")

    @unittest.skipIf(R2S is None, "R2S helper requires NumPy and pinned OpenMC environment")
    def test_invalid_negative_batch_increments_fail(self):
        plan = {"materials": [{"name": "m", "id": 4, "nuclides": []}],
                "flux_tally": "flux", "tally_ids": {"flux": 10}}
        with self.assertRaisesRegex(ValueError, "invalid batch increments"):
            R2S.summarize_below_data_edge_batches(plan, {"10": [[4.0, 1.0], [3.0, 2.0]]}, 2, 50)


if __name__ == "__main__":
    unittest.main()
