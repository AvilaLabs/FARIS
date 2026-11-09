#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure request-boundary tests; these do not import OpenMC or launch a solver."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import maintenance_event_r2s as worker


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class RequestBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        geometry = self.root / "geometry.h5m"
        geometry.write_bytes(b"opaque test geometry")
        model = self.root / "model.xml"
        model.write_text("<model><geometry><dagmc><filename>geometry.h5m</filename></dagmc></geometry></model>")
        xs = self.root / "cross_sections.xml"
        xs.write_text("<cross_sections/>")
        chain = self.root / "chain.xml"
        chain.write_text("<depletion_chain/>")
        history = b'{"events":[]}'
        history_path = self.root / "history.json"
        history_path.write_bytes(history)
        location = self.root / "location.json"
        location.write_text(json.dumps({"schema": "faris-task-location/v0.1", "id": "port-worker",
                                        "lower_left_cm": [0, 0, 0], "upper_right_cm": [10, 10, 10],
                                        "dimension": [1, 1, 1]}))
        map_path = self.root / "component-map.json"
        map_path.write_text(json.dumps({"schema": "faris-cad-component-material-map/v0.1",
                                        "geometry_sha256": digest(geometry),
                                        "materials": {"component-a": [101], "component-b": [102]}}))
        self.request = {
            "schema": worker.REQUEST_SCHEMA,
            "request_id": "request-1",
            "history_sha256": hashlib.sha256(history).hexdigest(),
            "history_artifact": {"path": str(history_path), "sha256": digest(history_path)},
            "event": {
                "design": "design-a", "class": "blanket", "component": "component-a",
                "replacement_k": 1, "shutdown_s": 100.0,
                "location_id": "port-worker", "location_sha256": digest(location), "cooling_s": [10.0],
                "source_groups": [
                    {"component": "component-a", "history_sha256": hashlib.sha256(history).hexdigest(),
                     "installation_s": 30.0,
                     "schedule": [{"duration_s": 70.0, "source_rate_n_s": 1.0},
                                  {"duration_s": 10.0, "source_rate_n_s": 0.0}]},
                    {"component": "component-b", "history_sha256": hashlib.sha256(history).hexdigest(),
                     "installation_s": 0.0,
                     "schedule": [{"duration_s": 100.0, "source_rate_n_s": 1.0},
                                  {"duration_s": 10.0, "source_rate_n_s": 0.0}]},
                ],
            },
            "model": {"model_xml": str(model), "sha256": digest(model)},
            "geometry": {"path": str(geometry), "sha256": digest(geometry)},
            "cross_sections": {"path": str(xs), "sha256": digest(xs)},
            "chain": {"path": str(chain), "sha256": digest(chain)},
            "component_material_map": {"path": str(map_path), "sha256": digest(map_path)},
            "activation_mesh": {"lower_left_cm": [0, 0, 0], "upper_right_cm": [10, 10, 10], "dimension": [1, 1, 1]},
            "response_mesh": {"artifact": {"path": str(location), "sha256": digest(location)},
                              "lower_left_cm": [0, 0, 0], "upper_right_cm": [10, 10, 10], "dimension": [1, 1, 1]},
            "sampling": {"neutron_particles": 100, "neutron_batches": 2,
                         "photon_particles": 100, "photon_batches": 2, "seed": 7, "threads": 1,
                         "mat_vol_samples": 1000, "activation_groups": "VITAMIN-J-42", "run_budget_s": 120.0},
        }

    def tearDown(self):
        self.temp.cleanup()

    def test_accepts_exact_opaque_history_and_all_source_groups(self):
        model, xs, chain, component_map = worker.validate_request(self.request)
        self.assertEqual(model.name, "model.xml")
        self.assertEqual(xs.name, "cross_sections.xml")
        self.assertEqual(chain.name, "chain.xml")
        self.assertEqual(component_map.name, "component-map.json")

    def test_rejects_history_artifact_that_does_not_match_rust_hash(self):
        history_path = Path(self.request["history_artifact"]["path"])
        history_path.write_bytes(b'{"events":[1]}')
        self.request["history_artifact"]["sha256"] = digest(history_path)
        with self.assertRaisesRegex(ValueError, "history artifact bytes"):
            worker.validate_request(self.request)

    def test_rejects_component_id_reuse_between_solids(self):
        mapping = Path(self.request["component_material_map"]["path"])
        data = json.loads(mapping.read_text())
        data["materials"]["component-b"] = [101]
        mapping.write_text(json.dumps(data))
        self.request["component_material_map"]["sha256"] = digest(mapping)
        with self.assertRaisesRegex(ValueError, "must be unique"):
            worker.validate_request(self.request)

    def test_rejects_schedule_that_does_not_reach_last_cooling_point(self):
        self.request["event"]["source_groups"][0]["schedule"][1]["duration_s"] = 9.0
        with self.assertRaisesRegex(ValueError, "does not span"):
            worker.validate_request(self.request)

    def test_rejects_response_volume_different_from_hashed_location(self):
        self.request["response_mesh"]["upper_right_cm"] = [11, 10, 10]
        with self.assertRaisesRegex(ValueError, "task-location artifact content"):
            worker.validate_request(self.request)

    def test_cad_preparation_requires_exact_nonvoid_solid_mapping(self):
        h5m = self.root / "checked.h5m"
        h5m.write_bytes(b"checked")
        xs = self.root / "xs.xml"
        xs.write_text("<xs/>")
        helper = self.root / "design_transport.py"
        helper.write_text("# trusted helper placeholder")
        request = {"schema": "faris-maintenance-dose-cad-prepare/v0.1", "h5m": str(h5m),
                   "h5m_sha256": digest(h5m), "cross_sections": str(xs),
                   "solids": [{"solid_id": "s1", "material_id": "steel"}],
                   "materials": [], "plasma": {}, "solid_history_components": {},
                   "openmc_helper": str(helper)}
        with self.assertRaisesRegex(ValueError, "cover every nonvoid"):
            worker.prepare_checked_cad_model(request, self.root / "prepared")

    def test_activation_chain_missing_neutron_data_fails_closed(self):
        xs = self.root / "coverage.xml"
        xs.write_text('<cross_sections><library type="neutron" materials="Fe56 U235" /></cross_sections>')
        chain = SimpleNamespace(nuclides=[SimpleNamespace(name="Fe56", reactions=[1]),
                                          SimpleNamespace(name="U235", reactions=[1]),
                                          SimpleNamespace(name="Co60", reactions=[1])])
        with self.assertRaisesRegex(RuntimeError, "silently zero-fill"):
            worker.chain_cross_section_coverage(chain, xs)


if __name__ == "__main__":
    unittest.main()
