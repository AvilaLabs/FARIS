#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure data-contract tests; no OpenMC or ACTINV solver is imported or launched."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
import maintenance_actinv_backend as backend


class ActinvBackendContracts(unittest.TestCase):
    def response(self, *, volume=500.0):
        return {
            "element_indices": np.array([0]),
            "material_ids": np.array([101]),
            "material_volumes_cm3": np.array([volume]),
            "integrated_track_length_cm_per_source": np.full((1, 709), 10.0),
        }

    def schedules(self):
        return [{"component": "blanket", "installation_s": 0.0,
                 "schedule": [{"duration_s": 4.0, "source_rate_n_s": 2.0},
                              {"duration_s": 6.0, "source_rate_n_s": 2.0},
                              {"duration_s": 5.0, "source_rate_n_s": 0.0}]}]

    def build(self, response=None):
        return backend.build_activation_batches(
            response_arrays=response or self.response(),
            activation_mesh={"lower_left_cm": [0, 0, 0], "upper_right_cm": [10, 10, 10],
                             "dimension": [1, 1, 1]},
            materials_by_component={"blanket": [101]},
            recipes={101: {"density_g_cm3": 8.0, "composition": {"Fe56": 1.0}}},
            source_groups=self.schedules(), shutdown_s=10.0, cooling_s=[1.0, 5.0],
            base_source_rate_n_s=2.0,
            neutron_energy_edges_ev=[float(i) for i in range(710)],
            photon_energy_edges_ev=[0.0, 100.0, 200.0],
        )

    def test_volume_normalization_and_physical_rate_are_applied_once(self):
        batch = self.build()["batches"][0]
        req = batch["activation_request"]
        zone = req["zones"][0]
        self.assertEqual(zone["mass_g"], 4000.0)
        self.assertEqual(zone["volume_m3"], 0.0005)
        self.assertEqual(zone["spectrum"]["flux_per_cm2_s"], [0.04] * 709)
        self.assertEqual(req["steps"], [{"duration_s": 4.0, "flux_scale": 1.0},
                                        {"duration_s": 6.0, "flux_scale": 1.0}])

    def test_material_volume_cannot_exceed_mesh_voxel(self):
        with self.assertRaisesRegex(ValueError, "exceeds voxel"):
            self.build(self.response(volume=1001.0))

    def test_schedule_must_end_exactly_at_shutdown_and_zero_after(self):
        groups = self.schedules()
        groups[0]["schedule"][2]["source_rate_n_s"] = 1.0
        with self.assertRaisesRegex(ValueError, "post-shutdown"):
            backend.build_activation_batches(
                response_arrays=self.response(),
                activation_mesh={"lower_left_cm": [0, 0, 0], "upper_right_cm": [10, 10, 10],
                                 "dimension": [1, 1, 1]},
                materials_by_component={"blanket": [101]},
                recipes={101: {"density_g_cm3": 8.0, "composition": {"Fe56": 1.0}}},
                source_groups=groups, shutdown_s=10.0, cooling_s=[1.0],
                base_source_rate_n_s=2.0,
                neutron_energy_edges_ev=[float(i) for i in range(710)],
                photon_energy_edges_ev=[0.0, 100.0, 200.0],
            )

    def test_activation_gaps_and_unbounded_outside_photons_refuse_spatial_source(self):
        base = {"schema": "faris-activation-result/v0.1", "zones": [{
            "id": "e0_m101", "completeness": "COMPLETE", "gaps": [], "run": {"problem_sha256": "a" * 64},
            "points": [{"cooling_s": 1.0, "photon_source": {
                "total_photons_per_s": 10.0, "photons_outside_groups_per_s": 0.0,
                "groups": [{"centroid_ev": 50.0, "low_ev": 0.0, "high_ev": 100.0,
                            "photons_per_s": 10.0}]}}]}]}
        meta = {"e0_m101": {"mesh_element": 0, "material_id": 101}}
        self.assertEqual(backend.photon_emitters_from_activation(base, meta, 1.0)[0]["photons_per_s"], 10.0)
        base["zones"][0]["gaps"] = [{"category": "decay", "count": 1}]
        with self.assertRaisesRegex(ValueError, "data gaps"):
            backend.photon_emitters_from_activation(base, meta, 1.0)
        base["zones"][0]["gaps"] = []
        base["zones"][0]["points"][0]["photon_source"]["photons_outside_groups_per_s"] = 0.1
        with self.assertRaisesRegex(ValueError, "outside requested group bounds"):
            backend.photon_emitters_from_activation(base, meta, 1.0)

    def test_safe_source_preflight_accepts_normalized_mesh_and_rejects_plugins(self):
        import xml.etree.ElementTree as ET
        root = ET.fromstring("""<model><settings>
          <source type="mesh" mesh="7" strength="1.0">
            <source type="independent" strength="0.4" particle="neutron">
              <space type="box"><parameters>0 0 0 1 1 1</parameters></space>
              <angle type="isotropic"/><energy type="discrete"><parameters>14e6 1</parameters></energy>
            </source>
            <source type="independent" strength="0.6" particle="neutron">
              <space type="box"><parameters>1 0 0 2 1 1</parameters></space>
              <angle type="isotropic"/><energy type="discrete"><parameters>14e6 1</parameters></energy>
            </source>
          </source>
          <mesh id="7"><dimension>2 1 1</dimension><lower_left>0 0 0</lower_left><upper_right>2 1 1</upper_right></mesh>
        </settings></model>""")
        backend.validate_safe_model_sources(root)
        root.find("settings/source/source").set("library", "attacker.py")
        with self.assertRaisesRegex(ValueError, "compiled or external"):
            backend.validate_safe_model_sources(root)


if __name__ == "__main__":
    unittest.main()
