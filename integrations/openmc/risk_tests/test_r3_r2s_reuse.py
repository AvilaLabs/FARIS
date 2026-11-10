# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

try:
    import h5py  # noqa: F401
    import numpy  # noqa: F401
except ImportError as exc:
    if os.environ.get("FARIS_REQUIRE_OPENMC") == "1":
        raise
    raise unittest.SkipTest(f"neutron reuse tests require h5py and NumPy: {exc}") from exc

RISK_DIR = Path(__file__).resolve().parents[3] / "integrations/openmc/risk_tests"
sys.path.insert(0, str(RISK_DIR))
SCRIPT = Path(__file__).with_name("r3_r2s.py")
spec = importlib.util.spec_from_file_location("r3_r2s_reuse_prototype", SCRIPT)
r2s = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = r2s
spec.loader.exec_module(r2s)


class NeutronReuseValidationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.run = self.root / "source-run"
        neutron = self.run / "neutron_transport"
        basis_dir = self.run / "shutdown_dose_basis"
        neutron.mkdir(parents=True)
        basis_dir.mkdir()

        self.chain = self.root / "chain.xml"
        self.xs = self.root / "cross_sections.xml"
        self.chain.write_bytes(b"chain")
        self.xs.write_bytes(b"xs")
        (self.run / "activation_domain.json").write_bytes(b"mesh-domain")
        self.artifacts = {}
        for name, content in {
            "model.xml": b"model", "materials.xml": b"materials",
            "mesh_material_volumes.npz": b"volume-map", "fluxes.npy": b"flux-mean",
            "fluxes_std_dev.npy": b"flux-std",
        }.items():
            path = neutron / name
            path.write_bytes(content)
            self.artifacts[name] = {"path": str(path), "sha256": r2s.RM.sha256(path)}
        # This partial HDF is deliberately present and must remain unused.
        (neutron / "bounded-collapse" ).mkdir()
        (neutron / "bounded-collapse/group_mean_xs.h5").write_bytes(b"partial-invalid-microxs")

        basis = {}
        for name in ("geometry", "irradiation", "location", "materials", "response", "source"):
            path = basis_dir / f"{name}.json"
            path.write_text(name)
            basis[name] = {"path": f"shutdown_dose_basis/{name}.json", "sha256": r2s.RM.sha256(path)}
        settings = {"activation_groups": "toy-groups", "seeds": [11, 12], "stop_after_neutron": False,
                    "timesteps": ((1.0, "a"), (1.0, "d"))}
        runtime = {"openmc_version": "0.15.3", "library_sha256": "runtime"}
        resources = {"collapse_activation_output_budget_bytes": 1000, "disk_reserve_bytes": 2000,
                     "working_memory_budget_bytes": 3000, "activation_domain_chunk": 2,
                     "activation_elapsed_time_budget_s": 50}
        preflight = {"status": "PASS_WITHIN_DECLARED_BUDGETS", "output_budget_bytes": 1000,
                     "disk_reserve_bytes": 2000, "memory_budget_bytes": 3000}
        group_edges = [0.0, 1.0, 2.0]
        integrity = {"status": "PASS", "histories": 10, "batches": 2,
                     "elapsed_s": 8.5,
                     "model_xml_sha256": self.artifacts["model.xml"]["sha256"],
                     "solver_model": {"sha256": self.artifacts["model.xml"]["sha256"]},
                     "lost_particle_assessment": {"status": "PASS", "lost_particles": 0, "histories": 10},
                     "flux_mean_sha256": self.artifacts["fluxes.npy"]["sha256"],
                     "flux_std_dev_sha256": self.artifacts["fluxes_std_dev.npy"]["sha256"],
                     "mesh_material_volumes": {"sha256": self.artifacts["mesh_material_volumes.npz"]["sha256"],
                                                "domain_count": 1}}
        protocol = {
            "schema": "faris-r3-bounded-campaign-protocol/v1", "sampling_settings": settings,
            "activation_mesh": {"path": "activation_domain.json", "sha256": r2s.RM.sha256(self.run / "activation_domain.json")},
            "inputs": {
                "model_xml": self.artifacts["model.xml"], "materials_xml": self.artifacts["materials.xml"],
                "mesh_material_volumes": {"path": "neutron_transport/mesh_material_volumes.npz",
                                          "sha256": self.artifacts["mesh_material_volumes.npz"]["sha256"]},
                "chain": {"path": str(self.chain), "sha256": r2s.RM.sha256(self.chain)},
                "cross_sections_xml": {"path": str(self.xs), "sha256": r2s.RM.sha256(self.xs)},
            },
            "fixed_protocol": {"energy_edges_eV": group_edges},
        }
        protocol["protocol_sha256"] = r2s.BM._canonical_sha256(protocol)
        protocol_path = self.run / "neutron_transport/bounded-protocol-plan.json"
        protocol_path.write_text(json.dumps(protocol))
        old_plan = {
            "inputs": {
                "fluxes": self.artifacts["fluxes.npy"],
                "volumes": self.artifacts["mesh_material_volumes.npz"],
                "materials": self.artifacts["materials.xml"],
            },
            "required_nuclide_hdf5": [],
        }
        old_plan["plan_sha256"] = r2s.BM._canonical_sha256(old_plan)
        (self.run / "neutron_transport/bounded-collapse-plan.json").write_text(json.dumps(old_plan))
        self.record = {
            "kind": "r3-r2s", "source_problem_protocol": r2s.sc.FULL_EMISSION_PROTOCOL,
            "settings": settings, "runtime_identity": runtime, "threads": 1,
            "library": {"cross_sections_xml": str(self.xs.resolve()), "sha256": r2s.RM.sha256(self.xs)},
            "chain": str(self.chain.resolve()), "chain_sha256": r2s.RM.sha256(self.chain),
            "steps": {"neutron_transport": {"ok": False, "error": "ValueError: collapsed group means are invalid"}},
            "neutron_transport_integrity": integrity, "shutdown_dose_basis": basis,
            "bounded_protocol_plan": {"path": "neutron_transport/bounded-protocol-plan.json",
                                       "sha256": r2s.RM.sha256(protocol_path),
                                       "protocol_sha256": protocol["protocol_sha256"]},
            "activation_domain": {"path": "activation_domain.json",
                                  "sha256": r2s.RM.sha256(self.run / "activation_domain.json")},
            "neutron_materials_xml": {"path": "neutron_transport/materials.xml",
                                      "sha256": self.artifacts["materials.xml"]["sha256"]},
            "activation_group_basis": {"energy_edges_eV": group_edges},
            "activation_chain_coverage": {"status": "READY_FOR_R2S_TRANSPORT",
                                          "chain_xml_sha256": r2s.RM.sha256(self.chain),
                                          "indexed_data_files": []},
            "bounded_resource_preflight": preflight, "bounded_resources": resources,
        }
        self.record_path = self.run / "r2s_record.json"
        self.record_path.write_text(json.dumps(self.record))
        self.kwargs = dict(
            expected_settings=settings, expected_groups={"energy_edges_eV": group_edges},
            expected_chain=self.chain, expected_cross_sections=self.xs, current_basis=basis,
            current_materials_sha256=self.artifacts["materials.xml"]["sha256"],
            expected_runtime=runtime, expected_threads=1, expected_resource_limits=resources)

    def tearDown(self):
        self.tmp.cleanup()

    def test_accepts_exact_neutron_receipt_and_ignores_partial_microxs(self):
        result = r2s._validate_neutron_reuse_source(self.record_path, **self.kwargs)
        self.assertTrue(result["old_collapse_partial_output_ignored"])
        self.assertEqual(result["integrity"]["histories"], 10)
        self.assertEqual(len(result["coverage_indexed_data_files"]), 0)

    def test_rejects_changed_flux_artifact(self):
        (self.run / "neutron_transport/fluxes.npy").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "flux arrays are missing or changed"):
            r2s._validate_neutron_reuse_source(self.record_path, **self.kwargs)

    def test_elapsed_summary_charges_original_transport_once(self):
        record = {"reused_neutron_original_elapsed_seconds": 8.5,
                  "steps": {"neutron_transport": {"seconds": 2.0},
                            "activation": {"seconds": 3.0}, "photon_transport": {"seconds": 4.0}}}
        r2s._update_reused_elapsed_summary(record)
        self.assertEqual(record["reused_neutron_validation_and_collapse_elapsed_seconds"], 2.0)
        self.assertEqual(record["method_elapsed_seconds_including_reused_neutron"], 17.5)
        r2s._update_reused_elapsed_summary(record)
        self.assertEqual(record["method_elapsed_seconds_including_reused_neutron"], 17.5)
        self.assertIn("original neutron transport wall time exactly once", record["method_elapsed_scope"])

    def test_rejects_changed_settings(self):
        kwargs = dict(self.kwargs)
        kwargs["expected_settings"] = {**self.kwargs["expected_settings"], "seeds": [99, 100]}
        with self.assertRaisesRegex(ValueError, "settings differ"):
            r2s._validate_neutron_reuse_source(self.record_path, **kwargs)


if __name__ == "__main__":
    unittest.main()
