#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Focused tests for the bounded OpenMC 709-group and ACTINV photon adapter."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

import numpy as np

import maintenance_709_response as worker


def write_ref(path: Path, data: bytes) -> dict:
    path.write_bytes(data)
    return {"path": str(path.resolve()), "sha256": hashlib.sha256(data).hexdigest()}


class ResponseMathTests(unittest.TestCase):
    def test_material_volume_rows_are_element_major_and_component_filtered(self):
        volumes = type("MeshMaterialVolumes", (), {
            "_materials": np.array([[7, 8, -1], [8, 7, -1]], dtype=np.int32),
            "_volumes": np.array([[1.5, 2.5, 0], [3.5, 4.5, 0]], dtype=np.float64),
        })()
        elements, ids, cm3 = worker.material_response_rows(volumes, {7})
        np.testing.assert_array_equal(elements, [0, 1])
        np.testing.assert_array_equal(ids, [7, 7])
        np.testing.assert_array_equal(cm3, [1.5, 4.5])

    def test_capi_integrated_flux_uses_expected_709_group_order(self):
        values = np.zeros((2 * 709, 1, 3), dtype=np.float64)
        values[:, 0, 1] = np.arange(1, 2 * 709 + 1, dtype=np.float64)
        result = worker.integrated_track_length_mean(values, 2, 2)
        self.assertEqual(result.shape, (2, 709))
        np.testing.assert_array_equal(result[0, :3], [0.5, 1.0, 1.5])
        np.testing.assert_array_equal(result[1, :3], [355.0, 355.5, 356.0])

    def test_mesh_index_is_openmc_x_fastest(self):
        bounds = {"lower_left_cm": [0.0, 0.0, 0.0], "upper_right_cm": [4.0, 6.0, 8.0],
                  "dimension": [2, 3, 4]}
        self.assertEqual(worker.voxel_bounds(bounds, 0), ([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]))
        self.assertEqual(worker.voxel_bounds(bounds, 1), ([2.0, 0.0, 0.0], [4.0, 2.0, 2.0]))
        self.assertEqual(worker.voxel_bounds(bounds, 2), ([0.0, 2.0, 0.0], [2.0, 4.0, 2.0]))

    def test_full_emitter_fixture_binds_activation_result_bins_and_strength(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            refs = {key: write_ref(root / f"{key}.bin", key.encode())
                    for key in ("model", "geometry", "xs", "location", "library")}
            map_value = {"schema": "faris-cad-component-material-map/v0.1",
                         "geometry_sha256": refs["geometry"]["sha256"], "materials": {"coil": [42]}}
            map_ref = write_ref(root / "map.json", json.dumps(map_value).encode())
            npz_path = root / "response.npz"
            np.savez_compressed(npz_path, element_indices=np.array([0], dtype=np.int32),
                material_ids=np.array([42], dtype=np.int32),
                material_volumes_cm3=np.array([1.0], dtype=np.float64),
                integrated_track_length_cm_per_source=np.zeros((1, 709), dtype=np.float64))
            response_ref = {"path": str(npz_path.resolve()), "sha256": worker.sha256_file(npz_path)}
            mesh = {"lower_left_cm": [0.0, 0.0, 0.0], "upper_right_cm": [1.0, 1.0, 1.0],
                    "dimension": [1, 1, 1]}
            event = {"design": "synthetic-fixture", "class": "test", "component": "coil",
                     "replacement_k": 1, "shutdown_s": 10.0, "location_id": "ap-1",
                     "location_sha256": refs["location"]["sha256"], "cooling_s": [20.0]}
            receipt = {"schema": worker.NEUTRON_RECEIPT_SCHEMA, "request_id": "fixture", "status": "PASS",
                "model": refs["model"], "geometry": refs["geometry"], "cross_sections": refs["xs"],
                "component_material_map": map_ref, "activation_mesh": mesh,
                "event": event, "source_groups": [{"component": "coil"}],
                "source": {"unit_strength": 1.0, "base_source_rate_n_s": 1e18},
                "activation_library_npz": refs["library"], "response_npz": response_ref}
            receipt_ref = write_ref(root / "neutron-receipt.json", json.dumps(receipt).encode())
            result = {"schema": worker.ACTIVATION_RESULT_SCHEMA, "cooling_s": [20.0],
                "data": {"library": {"sha256": refs["library"]["sha256"]}},
                "zones": [{"id": "e0_m42", "completeness": "COMPLETE", "gaps": [],
                    "input": {"volume_m3": 1e-6}, "points": [{"cooling_s": 20.0,
                    "photon_source": {"total_photons_per_s": 5.0,
                        "photons_outside_groups_per_s": 0.0,
                        "groups": [{"centroid_ev": 1e6, "photons_per_s": 5.0}]}}]}]}
            result_ref = write_ref(root / "activation.json", json.dumps(result).encode())
            emitter = {"schema": "faris-maintenance-actinv-photon-emitters/v0.1",
                "emitters_by_cooling": [{"cooling_s": 20.0, "source_strength_photons_s": 5.0,
                    "emitters": [{"component": "coil", "zone_id": "e0_m42", "mesh_element": 0,
                        "material_id": 42, "lower_left_cm": [0.0, 0.0, 0.0],
                        "upper_right_cm": [1.0, 1.0, 1.0], "photons_per_s": 5.0,
                        "energy_groups": [{"energy_ev": 1e6, "photons_per_s": 5.0}]}]}]}
            emitter_ref = write_ref(root / "emitters.json", json.dumps(emitter).encode())
            request = {"schema": worker.PHOTON_REQUEST_SCHEMA, "request_id": "fixture", "stage": "photon",
                "model": refs["model"], "geometry": refs["geometry"], "cross_sections": refs["xs"],
                "component_material_map": map_ref, "activation_mesh": mesh, "event": event,
                "sampling": {"neutron_particles": 100, "neutron_batches": 2,
                    "photon_particles": 100, "photon_batches": 2, "seed": 1, "threads": 1,
                    "mat_vol_samples": 1000, "activation_groups": "CCFE-709", "run_budget_s": 60.0},
                "neutron_response_receipt": receipt_ref,
                "activation_results": [{"component": "coil", **result_ref}],
                "response_mesh": {"artifact": refs["location"], "lower_left_cm": [0, 0, 0],
                    "upper_right_cm": [1, 1, 1], "dimension": [1, 1, 1]},
                "emitters_by_cooling": emitter_ref}
            self.assertEqual(worker._validate_stage_request(request, worker.PHOTON_REQUEST_SCHEMA), request)
            calculated = worker._emitters_from_results(request)
            self.assertEqual(calculated[20.0][0]["photons_per_s"], 5.0)
            wrong_event = {**request, "event": {**event, "location_id": "other"}}
            with self.assertRaisesRegex(ValueError, "location_id"):
                worker._emitters_from_results(wrong_event)
            bad = json.loads(json.dumps(emitter))
            bad["emitters_by_cooling"][0]["source_strength_photons_s"] = 9.0
            bad_ref = write_ref(root / "bad-emitters.json", json.dumps(bad).encode())
            tampered = {**request, "emitters_by_cooling": bad_ref}
            with self.assertRaisesRegex(ValueError, "source strength"):
                worker._emitters_from_results(tampered)

    def test_exact_openmc0153_filter_sources_and_model_export_without_solver(self):
        try:
            import openmc
        except ImportError:
            self.skipTest("OpenMC is not installed in this Python environment")
        self.assertEqual(openmc.__version__, "0.15.3")
        mesh = openmc.RegularMesh(name="synthetic-709")
        mesh.lower_left = (-1.0, -1.0, -1.0)
        mesh.upper_right = (1.0, 1.0, 1.0)
        mesh.dimension = (1, 1, 1)
        tally = worker.make_neutron_response_tally(
            openmc, mesh, np.array([0], dtype=np.int32), np.array([42], dtype=np.int32),
            [float(edge + 1) for edge in range(710)])
        self.assertEqual([type(item).__name__ for item in tally.filters],
                         ["MeshMaterialFilter", "EnergyFilter"])
        material = openmc.Material(material_id=42, name="synthetic-material")
        material.add_nuclide("H1", 1.0)
        material.set_density("g/cm3", 1.0)
        cell = openmc.Cell(fill=material, region=-openmc.Sphere(r=1.0, boundary_type="vacuum"))
        settings = openmc.Settings()
        settings.run_mode = "fixed source"
        settings.photon_transport = True
        constraint_info = worker.configure_material_source_sampling(settings)
        self.assertEqual(settings.source_rejection_fraction, 1e-6)
        self.assertEqual(constraint_info["strategy"], "resample")
        sources, total = worker._photon_sources(openmc, [{"material_id": 42,
            "photons_per_s": 5.0, "energy_groups": [
                {"energy_ev": 1e6, "photons_per_s": 2.0},
                {"energy_ev": 2e6, "photons_per_s": 3.0}],
            "lower_left_cm": [-0.2, -0.2, -0.2], "upper_right_cm": [0.2, 0.2, 0.2]}], {42: material})
        self.assertEqual(len(sources), 1)
        self.assertEqual(sources[0].strength, 5.0)
        self.assertEqual(total, 5.0)
        settings.source = sources
        model = openmc.Model(openmc.Geometry([cell]), materials=[material], settings=settings)
        _, dose_tally = worker._dose_tally(openmc, {"lower_left_cm": [-0.5, -0.5, -0.5],
            "upper_right_cm": [0.5, 0.5, 0.5], "dimension": [1, 1, 1]})
        model.tallies = openmc.Tallies([dose_tally])
        with tempfile.TemporaryDirectory() as temp:
            model.export_to_model_xml(temp)
            self.assertTrue((Path(temp) / "model.xml").is_file())
            root = ET.parse(Path(temp) / "model.xml").getroot()
            self.assertEqual(float(root.findtext("./settings/source_rejection_fraction")), 1e-6)
            neutron_settings = openmc.Settings()
            neutron_settings.run_mode = "fixed source"
            source_mesh = openmc.RegularMesh(mesh_id=21)
            source_mesh.lower_left = (-1.0, -1.0, -1.0)
            source_mesh.upper_right = (1.0, 1.0, 1.0)
            source_mesh.dimension = (1, 1, 1)
            source_bin = openmc.IndependentSource(
                energy=openmc.stats.Discrete([14.0e6], [1.0]), particle="neutron", strength=1.0)
            neutron_settings.source = openmc.MeshSource(source_mesh, [source_bin])
            neutron_model = openmc.Model(openmc.Geometry([cell]), materials=[material], settings=neutron_settings)
            neutron_dir = Path(temp) / "unit-source"
            neutron_dir.mkdir()
            neutron_model.export_to_model_xml(neutron_dir)
            identity = worker.source_kernel_identity(neutron_dir / "model.xml")
            self.assertEqual(identity["unit_strength"], 1.0)
            self.assertEqual(identity["source_types"], ["mesh"])
            self.assertEqual(len(identity["kernel_sha256"]), 64)

    def test_cli_dispatch_passes_hash_of_exact_request_bytes(self):
        import maintenance_event_r2s
        request_bytes = b'{"schema":"faris-maintenance-709-photon-request/v0.1","request_id":"fixture"}\n'
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "request.json"
            path.write_bytes(request_bytes)
            with mock.patch.object(sys, "argv", ["maintenance_event_r2s.py", "--request", str(path),
                                                  "--out", str(Path(temp) / "out")]), \
                    mock.patch.object(worker, "run_photon_stage") as run_photon:
                self.assertEqual(maintenance_event_r2s.main(), 0)
            run_photon.assert_called_once()
            self.assertEqual(run_photon.call_args.kwargs["request_sha256"],
                             hashlib.sha256(request_bytes).hexdigest())


if __name__ == "__main__":
    unittest.main()
