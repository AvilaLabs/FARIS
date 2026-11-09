#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure request-boundary tests; these do not import OpenMC or launch a solver."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

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

    def test_activation_chain_coverage_tracks_reachable_products_only(self):
        xs = self.root / "coverage.xml"
        xs.write_text('<cross_sections><library type="neutron" materials="Fe56 Fe57 Co58" /></cross_sections>')
        reaction = lambda target: SimpleNamespace(target=target)
        decay = lambda target: SimpleNamespace(target=target)
        chain = SimpleNamespace(
            nuclides=[SimpleNamespace(name="Fe56", reactions=[reaction("Fe57")], decay_modes=[]),
                      SimpleNamespace(name="Fe57", reactions=[reaction("Co58")], decay_modes=[]),
                      SimpleNamespace(name="Co58", reactions=[], decay_modes=[decay("Ni58")]),
                      SimpleNamespace(name="Ni58", reactions=[], decay_modes=[]),
                      SimpleNamespace(name="Unrelated", reactions=[reaction("Missing")], decay_modes=[])],
            get_default_fission_yields=lambda: {})
        coverage = worker.chain_cross_section_coverage(chain, xs, {"Fe56"})
        self.assertEqual(coverage["reachable_chain_nuclides"], 4)
        self.assertEqual(coverage["reachable_reaction_nuclides"], 2)
        self.assertEqual(coverage["unreachable_reaction_nuclides_excluded"], 1)
        self.assertIn("all listed decay targets", coverage["reachability_basis"])

    def test_activation_chain_reachable_reaction_missing_neutron_data_fails_closed(self):
        xs = self.root / "coverage-missing.xml"
        xs.write_text('<cross_sections><library type="neutron" materials="Fe56" /></cross_sections>')
        reaction = lambda target: SimpleNamespace(target=target)
        chain = SimpleNamespace(
            nuclides=[SimpleNamespace(name="Fe56", reactions=[reaction("Co60")], decay_modes=[]),
                      SimpleNamespace(name="Co60", reactions=[reaction("Ni60")], decay_modes=[]),
                      SimpleNamespace(name="Ni60", reactions=[], decay_modes=[])],
            get_default_fission_yields=lambda: {})
        with self.assertRaisesRegex(RuntimeError, "reachable activation-chain nuclides"):
            worker.chain_cross_section_coverage(chain, xs, {"Fe56"})

    def test_main_builds_chain_coverage_record_before_mocked_neutron_run(self):
        xs = Path(self.request["cross_sections"]["path"])
        xs.write_text('<cross_sections><library type="neutron" materials="Fe56" /></cross_sections>')
        self.request["cross_sections"]["sha256"] = digest(xs)
        chain_path = Path(self.request["chain"]["path"])
        chain_path.write_text("mock chain")
        self.request["chain"]["sha256"] = digest(chain_path)
        request_path = self.root / "request.json"
        request_path.write_text(json.dumps(self.request))
        out = self.root / "worker-out"
        library_path = self.root / "libopenmc.so"
        library_path.write_bytes(b"fake library identity")

        class Chain:
            @classmethod
            def from_xml(cls, _path):
                return SimpleNamespace(
                    nuclides=[SimpleNamespace(name="Fe56", reactions=[], decay_modes=[])],
                    get_default_fission_yields=lambda: {})

        class FakeTally:
            id = 7

            def __init__(self, name):
                self.name = name
                self.filters = []
                self.scores = []

        class FakeModel:
            def __init__(self):
                self.settings = SimpleNamespace()
                self.materials = [SimpleNamespace(get_nuclides=lambda: ["Fe56"])]

        class FakeManager:
            def __init__(self, *_args):
                self.results = {}

            def step1_neutron_transport(self, *_args, **_kwargs):
                raise RuntimeError("mock solver stop after method record assembly")

        lib = SimpleNamespace(core=SimpleNamespace(_dll=SimpleNamespace(_name=str(library_path))),
                              init=lambda *a, **k: None, run=lambda *a, **k: None)
        fake_openmc = SimpleNamespace(
            __version__="0.15.3", __file__=str(self.root / "openmc.py"), config={}, lib=lib,
            Model=SimpleNamespace(from_model_xml=lambda _p: FakeModel()),
            RegularMesh=lambda **_kw: SimpleNamespace(),
            data=SimpleNamespace(dose_coefficients=lambda *_a: ([0.0, 1.0], [0.0, 1.0])),
            Tally=FakeTally, Tallies=lambda values: list(values),
            MeshFilter=lambda mesh: ("mesh", mesh), ParticleFilter=lambda values: ("particle", values),
            EnergyFunctionFilter=lambda *a, **k: ("energy", a, k))
        fake_openmc.__path__ = []
        fake_deplete = SimpleNamespace(R2SManager=FakeManager, Chain=Chain)
        with patch.dict(sys.modules, {"openmc": fake_openmc, "openmc.deplete": fake_deplete}), \
             patch("sys.argv", ["maintenance_event_r2s.py", "--request", str(request_path), "--out", str(out)]):
            code = worker.main()
        self.assertEqual(code, 1)
        receipt = json.loads((out / "r2s_event_receipt.json").read_text())
        self.assertEqual(receipt["method"]["activation_chain_coverage"]["material_root_nuclides"], ["Fe56"])
        self.assertIn("mock solver stop", receipt["error"])

    def _microxs_evidence_fixture(self, root: Path, init_kwargs=None, mutate_during_init=False,
                                  mutate_during_run=False):
        runtime_dir = root / "runtime"
        runtime_dir.mkdir()
        runtime_model = runtime_dir / "model.xml"
        runtime_model.write_text('<model><tally name="MicroXS flux"/></model>')
        persisted_model = root / "neutron_transport" / "model.xml"
        persisted_model.parent.mkdir()
        persisted_model.write_bytes(runtime_model.read_bytes())
        library_path = root / "libopenmc.so"
        library_path.write_bytes(b"fake openmc library")

        def original_init(*_args, **_kwargs):
            if mutate_during_init:
                runtime_model.write_text('<model><tally name="changed"/></model>')

        def original_run(*_args, **_kwargs):
            if mutate_during_run:
                runtime_model.write_text('<model><tally name="changed during run"/></model>')

        lib = SimpleNamespace(
            init=original_init, run=original_run,
            settings=SimpleNamespace(batches=2, particles=10),
            _dll=object(),
            core=SimpleNamespace(_dll=SimpleNamespace(_name=str(library_path))))
        fake_openmc = SimpleNamespace(lib=lib)
        evidence = worker.MicroXsRunEvidence(fake_openmc, persisted_model)
        old_cwd = Path.cwd()
        with patch.object(worker.ctypes.c_int, "in_dll", lambda *_a, **_k: SimpleNamespace(value=0)):
            with evidence:
                os.chdir(runtime_dir)
                try:
                    lib.init(**(init_kwargs or {"intracomm": object()}))
                    lib.run()
                finally:
                    os.chdir(old_cwd)
                return evidence.result()

    def test_microxs_evidence_binds_temp_init_model_to_persisted_model_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            result = self._microxs_evidence_fixture(Path(temp))
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(Path(result["model_xml"]["path"]).name, "model.xml")
        self.assertEqual(Path(result["observed_c_api_init_model_path"]).name, "model.xml")
        self.assertEqual(result["histories"], 20)

    def test_microxs_evidence_rejects_explicit_alternate_init_arguments(self):
        with tempfile.TemporaryDirectory() as temp, self.assertRaisesRegex(RuntimeError, "alternate arguments"):
            self._microxs_evidence_fixture(Path(temp), init_kwargs={"args": ["alternate.xml"]})

    def test_microxs_evidence_rejects_model_changed_during_capi_init(self):
        with tempfile.TemporaryDirectory() as temp, self.assertRaisesRegex(RuntimeError, "changed during initialization"):
            self._microxs_evidence_fixture(Path(temp), mutate_during_init=True)

    def test_microxs_evidence_rejects_model_changed_during_capi_run(self):
        with tempfile.TemporaryDirectory() as temp, self.assertRaisesRegex(RuntimeError, "changed during C API run"):
            self._microxs_evidence_fixture(Path(temp), mutate_during_run=True)


if __name__ == "__main__":
    unittest.main()
