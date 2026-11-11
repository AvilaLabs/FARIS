#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure data-contract tests; no OpenMC or ACTINV solver is imported or launched."""
from __future__ import annotations

import math
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
import maintenance_actinv_backend as backend


class ActinvBackendContracts(unittest.TestCase):
    def test_geometry_binding_accepts_modern_dagmc_universe_and_rejects_mismatch(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            geometry = root / "dose.h5m"
            geometry.write_bytes(b"hash-checked fixture")
            model = root / "model.xml"
            model.write_text('<model><geometry><dagmc_universe filename="dose.h5m"/>'
                             '</geometry></model>', encoding="utf-8")
            backend.validate_model_geometry_binding(model, geometry)
            model.write_text('<model><geometry><dagmc><filename>other.h5m</filename>'
                             '</dagmc></geometry></model>', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "hash-bound exact DAGMC geometry"):
                backend.validate_model_geometry_binding(model, geometry)

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

    def build(self, response=None, *, track_light_charged_products=False):
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
            track_light_charged_products=track_light_charged_products,
        )

    def test_volume_normalization_and_physical_rate_are_applied_once(self):
        batch = self.build()["batches"][0]
        req = batch["activation_request"]
        zone = req["zones"][0]
        self.assertTrue(math.isclose(zone["mass_g"], 4000.0, rel_tol=1e-12, abs_tol=1e-15))
        self.assertTrue(math.isclose(zone["volume_m3"], 0.0005, rel_tol=1e-12, abs_tol=1e-15))
        self.assertEqual(zone["spectrum"]["flux_per_cm2_s"], [0.04] * 709)
        self.assertEqual(req["steps"], [{"duration_s": 4.0, "flux_scale": 1.0},
                                        {"duration_s": 6.0, "flux_scale": 1.0}])

    def test_light_charged_product_tracking_is_opt_in_in_the_event_batch(self):
        off = self.build()["batches"][0]["activation_request"]
        on = self.build(track_light_charged_products=True)["batches"][0]["activation_request"]
        self.assertNotIn("track_light_charged_products", off)
        self.assertIs(on["track_light_charged_products"], True)

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
        self.assertTrue(math.isclose(
            backend.photon_emitters_from_activation(base, meta, 1.0)[0]["photons_per_s"],
            10.0, rel_tol=1e-12, abs_tol=1e-15))
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
            <source type="independent" strength="0.0" particle="neutron">
              <space type="box"><parameters>0 0 0 1 1 1</parameters></space>
              <angle type="isotropic"/><energy type="discrete"><parameters>14e6 1</parameters></energy>
            </source>
            <source type="independent" strength="1.0" particle="neutron">
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

    def test_safe_source_preflight_accepts_bounded_recursive_energy_mixtures(self):
        import xml.etree.ElementTree as ET
        root = ET.fromstring("""<model><settings><source type="independent" strength="1" particle="neutron">
          <angle type="isotropic"/><energy type="mixture">
            <pair probability="0.25"><dist type="normal" parameters="2450000 16000"/></pair>
            <pair probability="0.75"><dist type="mixture">
              <pair probability="0.5"><dist type="normal" parameters="14000000 35000"/></pair>
              <pair probability="0.5"><dist type="tabular" interpolation="linear-linear">
                <parameters>1 2 0.25 0.75</parameters>
              </dist></pair>
            </dist></pair>
          </energy></source></settings></model>""")
        backend.validate_safe_model_sources(root)

    def test_safe_source_preflight_rejects_bad_mixture_weights_and_depth(self):
        import xml.etree.ElementTree as ET
        negative = ET.fromstring("""<model><settings><source type="independent" strength="1">
          <energy type="mixture"><pair probability="-0.1"><dist type="normal" parameters="1 1"/></pair>
          <pair probability="1.1"><dist type="normal" parameters="2 1"/></pair></energy>
        </source></settings></model>""")
        with self.assertRaisesRegex(ValueError, "mixture probability"):
            backend.validate_safe_model_sources(negative)

        distribution = '<dist type="normal" parameters="1 1"/>'
        for _ in range(5):
            distribution = ('<dist type="mixture"><pair probability="1">' + distribution + '</pair></dist>')
        nested = ET.fromstring('<model><settings><source type="independent" strength="1"><energy type="mixture">'
                               '<pair probability="1">' + distribution + '</pair>'
                               '</energy></source></settings></model>')
        with self.assertRaisesRegex(ValueError, "depth bound"):
            backend.validate_safe_model_sources(nested)

        bounded = ET.fromstring("""<model><settings><source type="independent" strength="1">
          <energy type="mixture"><pair probability="1"><dist type="normal" parameters="1 1"/></pair>
        </energy></source></settings></model>""")
        with patch.object(backend, "MAX_SOURCE_DISTRIBUTION_NODES", 1):
            with self.assertRaisesRegex(ValueError, "node-count bound"):
                backend.validate_safe_model_sources(bounded)


if __name__ == "__main__":
    unittest.main()
