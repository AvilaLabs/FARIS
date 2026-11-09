# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Synthetic exporter fixtures; these values are not simulation or validation evidence."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest

import numpy as np

try:
    import h5py
except ImportError:
    h5py = None

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import r3_shutdown_dose_export as EX  # noqa: E402
from r3_r2s import sampling_settings  # noqa: E402


def _fixture(root: Path, method: str, basis_hashes=None):
    chain_path = root.parent / "synthetic-chain.xml"
    chain_path.parent.mkdir(parents=True, exist_ok=True)
    chain_path.write_text("<synthetic-chain/>\n", encoding="utf-8")
    chain_hash = hashlib.sha256(chain_path.read_bytes()).hexdigest()
    basis = {}
    for key in ("geometry", "materials", "source", "irradiation", "response", "location"):
        if key == "irradiation":
            data = EX._json_bytes({"timesteps": sc.TIMESTEPS, "source_rates_neutrons_s": sc.SOURCE_RATES,
                                   "chain_path": str(chain_path), "chain_sha256": chain_hash})
        else:
            data = (f"synthetic {key} fixture\n").encode()
        path = root / "shutdown_dose_basis" / f"{key}.bin"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        basis[key] = {"path": str(path.relative_to(root)), "sha256": hashlib.sha256(data).hexdigest()}
    if basis_hashes:
        data = b"synthetic differing geometry fixture\n"
        (root / basis["geometry"]["path"]).write_bytes(data)
        basis["geometry"]["sha256"] = hashlib.sha256(data).hexdigest()
    settings = {"seeds": [123, 124], "timesteps": sc.TIMESTEPS}
    settings["source_rates" if method == "d1s" else "source_rates_n_s"] = sc.SOURCE_RATES
    run = {"kind": "r3-d1s" if method == "d1s" else "r3-r2s", "openmc_version": "0.15.3",
           "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
           "seed": 123, "chain": str(chain_path), "chain_sha256": chain_hash, "settings": settings,
           "mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM), "upper_right_cm": list(sc.MESH_UPPER_CM),
                    "dimension": list(sc.MESH_DIMENSION)},
           "shutdown_dose_basis": basis, "shutdown_dose": {}}
    model_paths = [root / "run/model.xml"] if method == "d1s" else [
        root / "photon_transport" / f"time_{idx}" / "model.xml" for idx in sc.COOLING_LABELS]
    for model_path in model_paths:
        model_path.parent.mkdir(parents=True, exist_ok=True)
        model_path.write_text('<model><tallies><tally name="synthetic"/></tallies></model>\n', encoding="utf-8")
    software_root = root / "synthetic_software"
    software_root.mkdir(parents=True, exist_ok=True)
    lib_path = software_root / "libopenmc.so"
    cli_path = software_root / "openmc"
    lib_path.write_text("synthetic C API library\n", encoding="utf-8")
    cli_path.write_text("synthetic OpenMC CLI\n", encoding="utf-8")
    library_identity = {"path": str(lib_path.resolve()), "sha256": hashlib.sha256(lib_path.read_bytes()).hexdigest()}
    cli_identity = {"path": str(cli_path.resolve()), "sha256": hashlib.sha256(cli_path.read_bytes()).hexdigest()}
    if method == "d1s":
        model_path = model_paths[0]
        settings["max_batches"] = 2
        settings["born_filter"] = None
        settings["photon_particles"] = 10
        settings["dose_scope"] = "all transported photon births and descendants; score only in the response mesh"
        run["particles_per_batch"] = 10
        run["run"] = {"batches": 2, "histories": 20, "stop_reason": "batch_limit", "solver_status": 1,
          "integrity": {
            "solver_model": {"path": str(model_path.resolve()), "sha256": hashlib.sha256(model_path.read_bytes()).hexdigest()},
            "c_api_library": library_identity,
            "lost_particle_assessment": {"status": "PASS", "lost_particles": 0, "histories": 20,
                                          "fraction_limit": 1e-6},
        }}
    else:
        run["runtime_identity"] = {"openmc_version": "0.15.3", "c_api_library": library_identity,
                                   "cli_executable": cli_identity}
        neutron_model = root / "neutron_transport" / "model.xml"
        neutron_model.parent.mkdir(parents=True, exist_ok=True)
        neutron_model.write_text("synthetic neutron manager model\n", encoding="utf-8")
        run["neutron_transport_integrity"] = {
            "status": "PASS", "solver_model": {"path": str(neutron_model.resolve()),
                                                   "sha256": hashlib.sha256(neutron_model.read_bytes()).hexdigest()},
            "c_api_library": library_identity,
            "histories": 20,
            "lost_particle_assessment": {"status": "PASS", "lost_particles": 0, "histories": 20,
                                          "fraction_limit": 1e-6},
        }
        run["settings"] = sampling_settings(SimpleNamespace(
            neutron_particles=10, neutron_batches=2,
            photon_particles=10, photon_max_batches=2, photon_budget=30.0,
            seed=123, mat_vol_samples=10, activation_cell_cm=100.0,
        ))
        run["photon"] = {}
        h5m_path = root / "synthetic_rm_m.h5m"
        with h5py.File(h5m_path, "w") as h5m:
            nodes = h5m.create_group("tstt").create_group("nodes")
            nodes.create_dataset("coordinates", data=np.array([
                [-10.0, -10.0, -10.0], [10.0, 10.0, 10.0],
                [-10.0, 10.0, 10.0], [10.0, -10.0, -10.0],
            ], dtype=np.float64))
        h5m_hash = hashlib.sha256(h5m_path.read_bytes()).hexdigest()
        card_path = root / "synthetic_rm_m_model_card.json"
        card_path.write_text(json.dumps({"schema": "faris-rm-m-model-card/v1",
            "bounding_sphere_radius": {"value": 1000.0}, "outputs": {"h5m_sha256": h5m_hash}}), encoding="utf-8")
        card_hash = hashlib.sha256(card_path.read_bytes()).hexdigest()
        grid = sc.activation_grid_spec([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0], 100.0)
        mesh_xml = (f'<mesh><dimension>{" ".join(map(str, grid["dimension"]))}</dimension>'
                    f'<lower_left>{" ".join(map(str, grid["lower_left_cm"]))}</lower_left>'
                    f'<upper_right>{" ".join(map(str, grid["upper_right_cm"]))}</upper_right></mesh>')
        domain = {"schema": "faris-r3-activation-domain/v1", "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
            "photon_source_scope": "synthetic all material-bearing domains",
            "emission_transport_scope": "synthetic descendants into response mesh",
            "source_rate_normalization": "synthetic photons/s",
            "geometry": {"h5m_path": str(h5m_path.resolve()), "h5m_sha256": h5m_hash,
                         "model_card_path": str(card_path.resolve()), "model_card_sha256": card_hash,
                         "model_card_bounding_sphere_radius_cm": 1000.0,
                         "h5m_vertex_lower_left_cm": [-10.0, -10.0, -10.0],
                         "h5m_vertex_upper_right_cm": [10.0, 10.0, 10.0],
                         "h5m_max_vertex_radius_cm": float(np.sqrt(300.0))},
            "activation_mesh": {**grid, "mesh_xml": mesh_xml},
            "response_mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM), "upper_right_cm": list(sc.MESH_UPPER_CM),
                               "dimension": list(sc.MESH_DIMENSION), "voxel_cm": sc.MESH_VOXEL_CM},
            "discretization_limit": "synthetic fixture only; no adequacy claim"}
        domain_path = root / "activation_domain.json"
        domain_path.write_text(json.dumps(domain), encoding="utf-8")
        run["activation_domain"] = {"path": domain_path.name,
            "sha256": hashlib.sha256(domain_path.read_bytes()).hexdigest()}
    for idx, label in sc.COOLING_LABELS.items():
        values = np.array([1.0, 3.0] if method == "d1s" else [2.0, 2.0], dtype=float)
        batch_path = root / ("synthetic_d1s_batches.npz" if method == "d1s" else f"synthetic_r2s_batches_{idx}.npz")
        if method == "d1s":
            arrays = {f"time_{i}": values for i in sc.COOLING_LABELS}
            np.savez_compressed(batch_path, **arrays)
        else:
            np.savez_compressed(batch_path, values=values)
        raw = batch_path.read_bytes()
        run["shutdown_dose"][label] = {
            "mean": float(values.mean()), "standard_error": float(values.std(ddof=1) / len(values) ** 0.5),
            "batches": len(values), "time_index": idx, "unit": "pSv.cm3/s summed over the scored region",
            "uncertainty_scope": ("Monte Carlo transport batch sampling only; excludes nuclear-data and model uncertainty"
                                  if method == "d1s" else
                                  "conditional Monte Carlo photon batch sampling for this generated decay-photon source; neutron transport and activation-source uncertainty are not propagated"),
            "batch_values_file": batch_path.name, "batch_values_sha256": hashlib.sha256(raw).hexdigest(),
        }
        if method == "r2s":
            model_path = root / "photon_transport" / f"time_{idx}" / "model.xml"
            run["photon"][label] = {"batches": 2, "histories": 20,
              "run": {"batches": 2, "histories": 20, "stop_reason": "batch_limit", "solver_status": 1,
                "integrity": {"solver_model": {"path": str(model_path.resolve()),
                                                  "sha256": hashlib.sha256(model_path.read_bytes()).hexdigest()},
                              "c_api_library": library_identity,
                              "lost_particle_assessment": {"status": "PASS", "lost_particles": 0,
                                                            "histories": 20, "fraction_limit": 1e-6}}}}
        source_relative = (Path("run") / f"d1s-source-{idx}.json" if method == "d1s" else
                           Path("photon_transport") / f"time_{idx}" / "decay-source.xml")
        source_path = root / source_relative
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text(f"synthetic {method} photon source {idx}\n", encoding="utf-8")
        run["shutdown_dose"][label]["method_specific_source"] = {
            "path": str(source_path.relative_to(root)), "sha256": hashlib.sha256(source_path.read_bytes()).hexdigest(),
        }
    record_path = root / ("d1s_record.json" if method == "d1s" else "r2s_record.json")
    record_path.write_text(json.dumps(run), encoding="utf-8")


@unittest.skipUnless(h5py is not None, "exporter tests require h5py to inspect actual H5M coordinates")
class R3ShutdownDoseExportTest(unittest.TestCase):
    def test_synthetic_batch_outputs_generate_all_cooling_point_records(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s, out = base / "d1s", base / "r2s", base / "export"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            request = EX.export_records(d1s, r2s, out)
            self.assertEqual(len(request["points"]), 3)
            self.assertTrue(all(point["covariance"] == {"kind": "unknown"} for point in request["points"]))
            estimate = json.loads((out / "d1s-2-estimate.json").read_text())
            self.assertEqual(estimate["unit"], "uSv/h")
            self.assertEqual(estimate["estimate"]["standard_error"],
                             sc.dose_rate_usv_per_h(1.0, sc.mesh_volume_cm3()))
            run_record = json.loads((out / "d1s-2-run.json").read_text())
            for ref in (run_record["source_run_record"], run_record["method_specific_solver_model"],
                        run_record["activation_decay_chain"], run_record["method_specific_photon_source"],
                        run_record["batch_statistics"]["batch_values"]):
                artifact = out / ref["path"]
                self.assertTrue(artifact.is_file())
                self.assertEqual(hashlib.sha256(artifact.read_bytes()).hexdigest(), ref["sha256"])
            activation = run_record["r2s_full_geometry_activation_domain"]
            for ref in (activation["domain_record"], activation["geometry_h5m"], activation["model_card"]):
                artifact = out / ref["path"]
                self.assertTrue(artifact.is_file())
                self.assertEqual(hashlib.sha256(artifact.read_bytes()).hexdigest(), ref["sha256"])
            self.assertEqual(activation["mesh"]["dimension"], [2, 2, 2])
            self.assertIn("no adequacy claim", activation["discretization_limit"])
            with self.assertRaisesRegex(ValueError, "already exists"):
                EX.export_records(d1s, r2s, out)

    def test_legacy_missing_basis_and_mismatched_basis_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s", basis_hashes="0" * 64)
            with self.assertRaisesRegex(ValueError, "input bytes differ"):
                EX.export_records(d1s, r2s, base / "out")
            run_path = d1s / "d1s_record.json"
            run = json.loads(run_path.read_text())
            run.pop("shutdown_dose_basis")
            run_path.write_text(json.dumps(run))
            with self.assertRaisesRegex(ValueError, "legacy output refused"):
                EX.export_records(d1s, d1s, base / "legacy")

    def test_batch_artifact_tampering_or_summary_substitution_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            (d1s / "synthetic_d1s_batches.npz").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                EX.export_records(d1s, r2s, base / "out")

    def test_model_mutation_after_tally_run_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            with (d1s / "run" / "model.xml").open("a", encoding="utf-8") as stream:
                stream.write("post-run mutation\n")
            with self.assertRaisesRegex(ValueError, "hash does not match the run record"):
                EX.export_records(d1s, r2s, base / "out")

    def test_missing_or_out_of_band_loss_evidence_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            d1s_record = d1s / "d1s_record.json"
            record = json.loads(d1s_record.read_text())
            record["run"]["integrity"]["lost_particle_assessment"]["lost_particles"] = 1
            d1s_record.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "lost-particle count/history evidence"):
                EX.export_records(d1s, r2s, base / "out")

    def test_r2s_unmeasured_manager_neutron_losses_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            r2s_record = r2s / "r2s_record.json"
            record = json.loads(r2s_record.read_text())
            record["neutron_transport_integrity"]["status"] = "NOT_EVALUATED"
            r2s_record.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "R2S neutron transport integrity is NOT_EVALUATED"):
                EX.export_records(d1s, r2s, base / "out")

    def test_legacy_meshborn_d1s_is_not_equivalent_to_full_emission(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            record_path = d1s / "d1s_record.json"
            record = json.loads(record_path.read_text())
            record.pop("source_problem_protocol")
            record_path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "legacy MeshBornFilter diagnostic"):
                EX.export_records(d1s, r2s, base / "legacy")

    def test_full_emission_protocol_rejects_a_meshborn_filter_in_d1s_xml(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            record_path = d1s / "d1s_record.json"
            record = json.loads(record_path.read_text())
            model_path = d1s / "run/model.xml"
            model_path.write_text('<model><tallies><filter type="meshborn"/></tallies></model>\n')
            record["run"]["integrity"]["solver_model"]["sha256"] = hashlib.sha256(model_path.read_bytes()).hexdigest()
            record_path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "contains MeshBornFilter"):
                EX.export_records(d1s, r2s, base / "out")

    def test_r2s_activation_domain_must_cover_h5m_and_remain_separate_from_response_mesh(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            domain_path = r2s / "activation_domain.json"
            domain = json.loads(domain_path.read_text())
            domain["response_mesh"]["dimension"] = [1, 1, 1]
            domain_path.write_text(json.dumps(domain))
            record_path = r2s / "r2s_record.json"
            record = json.loads(record_path.read_text())
            record["activation_domain"]["sha256"] = hashlib.sha256(domain_path.read_bytes()).hexdigest()
            record_path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "confuses its activation and dose-response meshes"):
                EX.export_records(d1s, r2s, base / "out")

    def test_rehashed_false_h5m_bounds_are_compared_with_actual_node_coordinates(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            d1s, r2s = base / "d1s", base / "r2s"
            _fixture(d1s, "d1s")
            _fixture(r2s, "r2s")
            domain_path = r2s / "activation_domain.json"
            domain = json.loads(domain_path.read_text())
            geometry = domain["geometry"]
            geometry["h5m_vertex_lower_left_cm"] = [-11.0, -11.0, -11.0]
            geometry["h5m_vertex_upper_right_cm"] = [11.0, 11.0, 11.0]
            grid = sc.activation_grid_spec(geometry["h5m_vertex_lower_left_cm"],
                                           geometry["h5m_vertex_upper_right_cm"], 100.0)
            mesh_xml = (f'<mesh><dimension>{" ".join(map(str, grid["dimension"]))}</dimension>'
                        f'<lower_left>{" ".join(map(str, grid["lower_left_cm"]))}</lower_left>'
                        f'<upper_right>{" ".join(map(str, grid["upper_right_cm"]))}</upper_right></mesh>')
            domain["activation_mesh"] = {**grid, "mesh_xml": mesh_xml}
            domain_path.write_text(json.dumps(domain))
            record_path = r2s / "r2s_record.json"
            record = json.loads(record_path.read_text())
            record["activation_domain"]["sha256"] = hashlib.sha256(domain_path.read_bytes()).hexdigest()
            record_path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "differ from the actual H5M node coordinates"):
                EX.export_records(d1s, r2s, base / "out")

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFO creation is Unix-specific")
    def test_special_file_inputs_are_rejected_without_blocking(self):
        with tempfile.TemporaryDirectory() as temp:
            fifo = Path(temp) / "batch-values.fifo"
            os.mkfifo(fifo)
            with self.assertRaisesRegex(ValueError, "regular file"):
                EX._sha256_file(fifo)


if __name__ == "__main__":
    unittest.main()
