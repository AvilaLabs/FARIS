# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""OpenMC 0.15.3 API-only source serializer checks; no transport is initialized."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_shutdown_dose_export as DX  # noqa: E402
import r3_dose_statistics as DS  # noqa: E402
import r3_scenario as sc  # noqa: E402
import r3_d1s as D1S  # noqa: E402
import r3_r2s as R2S  # noqa: E402
import rm_m_openmc as RM  # noqa: E402

if importlib.util.find_spec("openmc") is None:
    openmc = None
else:
    import openmc  # noqa: E402


def _actual_basis_smoke(method: str, model_dir: Path, out: Path) -> dict:
    """Build one producer's actual OpenMC basis in an isolated interpreter process."""
    import openmc
    from openmc.deplete import d1s
    import r3_scenario as sc
    import r3_r2s as R2S
    import r3_shutdown_dose_export as DX
    import rm_m_openmc as RM

    openmc.reset_auto_ids()
    response = R2S.scoring_mesh(openmc)
    if method == "r2s":
        extra = openmc.RegularMesh(name="synthetic-r2s-activation-id-offset")
        extra.lower_left = (-500.0, -500.0, -500.0)
        extra.upper_right = (500.0, 500.0, 500.0)
        extra.dimension = (10, 10, 10)
    roles = RM.load_roles(model_dir)
    model, _ = RM.build_model(openmc, model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, 1, rate=1.0, mesh_resolution=(4, 4))
    source_mesh_id = source.mesh.id
    tally = R2S.dose_tally(openmc, response)
    if method == "d1s":
        model.tallies = openmc.Tallies([tally])
        d1s.prepare_tallies(model, nuclides=["H1"])
    refs = DX.write_basis_artifacts(
        out, model, source, response,
        {"timesteps": [[1.0, "a"]], "source_rates_neutrons_s": [1.0]},
        [1.0, 2.0], [3.0, 4.0], sc.rm_m_geometry_dependencies(model_dir),
        {"cross_sections_sha256": "1" * 64},
    )
    return {"hashes": {name: ref["sha256"] for name, ref in refs.items()},
            "source_mesh_id": source_mesh_id}


@unittest.skipIf(openmc is None, "OpenMC is not installed in this Python")
class R3SourceSerializationTest(unittest.TestCase):
    def _mesh(self, mesh_id: int, upper_right=(2.0, 2.0, 2.0)):
        mesh = openmc.RegularMesh(mesh_id=mesh_id)
        mesh.lower_left = (0.0, 0.0, 0.0)
        mesh.upper_right = upper_right
        mesh.dimension = (2, 2, 2)
        return mesh

    def _mesh_source(self, mesh_id: int, upper_right=(2.0, 2.0, 2.0), particle="neutron"):
        mesh = self._mesh(mesh_id, upper_right)
        source_bins = [openmc.IndependentSource(
            energy=openmc.stats.Discrete([14.0e6], [1.0]), particle=particle, strength=float(i + 1))
            for i in range(8)]
        return openmc.MeshSource(mesh, source_bins)

    def _model(self):
        material = openmc.Material(material_id=51)
        material.add_nuclide("H1", 1.0)
        material.set_density("atom/b-cm", 0.01)
        region = openmc.Cell(cell_id=61, fill=material)
        model = openmc.Model(geometry=openmc.Geometry([region]), materials=openmc.Materials([material]))
        return model

    def test_primary_mesh_source_binds_mesh_geometry_and_normalizes_only_ids(self):
        model = self._model()
        location = self._mesh(3)
        args = ({"irradiation": True}, [1.0, 2.0], [3.0, 4.0], {}, {})
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            first = DX.write_basis_artifacts(base / "first", model, self._mesh_source(17), location, *args)
            same_physics = DX.write_basis_artifacts(base / "second", model, self._mesh_source(117), location, *args)
            changed_geometry = DX.write_basis_artifacts(
                base / "third", model, self._mesh_source(217, upper_right=(3.0, 2.0, 2.0)), location, *args)
            self.assertEqual(first["source"]["sha256"], same_physics["source"]["sha256"])
            self.assertNotEqual(first["source"]["sha256"], changed_geometry["source"]["sha256"])
            payload = json.loads((base / "first" / first["source"]["path"]).read_text())
            entry = payload["sources"][0]
            self.assertIn("upper_right>2.0 2.0 2.0", entry["meshes"][0]["definition"])
            self.assertNotIn('mesh="17"', entry["source_xml"])
            self.assertIn('strength="8.0"', entry["source_xml"])

    def test_r2s_mesh_sources_keep_mesh_bins_weights_and_source_order(self):
        first_sources = [self._mesh_source(31, particle="photon"), self._mesh_source(32, particle="photon")]
        renamed_sources = [self._mesh_source(131, particle="photon"), self._mesh_source(132, particle="photon")]
        changed_sources = [self._mesh_source(231, upper_right=(4.0, 2.0, 2.0), particle="photon")]
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            first = DX.write_r2s_source_artifact(base / "first", 2, first_sources)
            renamed = DX.write_r2s_source_artifact(base / "renamed", 2, renamed_sources)
            changed = DX.write_r2s_source_artifact(base / "changed", 2, changed_sources)
            self.assertEqual(first["sha256"], renamed["sha256"])
            self.assertNotEqual(first["sha256"], changed["sha256"])
            payload = json.loads((base / "first" / first["path"]).read_text())
            self.assertEqual(len(payload["sources"]), 2)
            self.assertIn("dimension>2 2 2", payload["sources"][0]["meshes"][0]["definition"])
            self.assertIn('strength="8.0"', payload["sources"][0]["source_xml"])
            self.assertNotIn('mesh="31"', payload["sources"][0]["source_xml"])

    def test_real_openmc_microxs_model_xml_matches_observation_marker_without_solver(self):
        material = openmc.Material(material_id=701)
        material.add_nuclide("H1", 1.0)
        material.set_density("atom/b-cm", 0.01)
        model = openmc.Model(geometry=openmc.Geometry([openmc.Cell(cell_id=702, fill=material)]),
                             materials=openmc.Materials([material]))
        tally = openmc.Tally(name="MicroXS flux")
        tally.scores = ["flux"]
        model.tallies = openmc.Tallies([tally])
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "model.xml"
            model.export_to_model_xml(path)
            self.assertTrue(DS._is_microxs_model(path))
            from openmc import lib as openmc_lib
            lib = openmc_lib
            hook = DS.NeutronManagerRunEvidence(openmc, path)
            original_init = lib.init
            original_run = lib.run
            with hook:
                self.assertIsNot(lib.init, original_init)
                self.assertIsNot(lib.run, original_run)
            self.assertIs(lib.init, original_init)
            self.assertIs(lib.run, original_run)

    def test_real_openmc_full_emission_meshes_are_distinct_and_source_scope_is_recorded(self):
        """Synthetic HDF5 node coordinates exercise the pinned DAGMC bounds API; no transport is run."""
        import hashlib
        import h5py
        import numpy as np
        import openmc.deplete
        from openmc.deplete import d1s

        with tempfile.TemporaryDirectory() as temp:
            model_dir = Path(temp)
            h5m = model_dir / "rm_m.h5m"
            coordinates = np.array([[-230.0, -310.0, -90.0], [450.0, 330.0, 210.0]])
            with h5py.File(h5m, "w") as data:
                data.create_group("tstt/nodes").create_dataset("coordinates", data=coordinates)
            h5m_hash = hashlib.sha256(h5m.read_bytes()).hexdigest()
            (model_dir / "rm_m_model_card.json").write_text(json.dumps({
                "schema": "faris-rm-m-model-card/v1", "bounding_sphere_radius": {"value": 1000.0},
                "outputs": {"h5m_sha256": h5m_hash},
            }), encoding="utf-8")
            dag = openmc.DAGMCUniverse(str(h5m))
            activation, descriptor = R2S.activation_domain(openmc, model_dir, dag, 100.0)
            response = R2S.scoring_mesh(openmc)
            dose = R2S.dose_tally(openmc, response)
            self.assertFalse(any(isinstance(item, openmc.MeshBornFilter) for item in dose.filters))
            self.assertEqual(descriptor["source_problem_protocol"], DX.sc.FULL_EMISSION_PROTOCOL)
            self.assertEqual(descriptor["geometry"]["h5m_vertex_lower_left_cm"], [-230.0, -310.0, -90.0])
            self.assertEqual(activation.lower_left, (-300.0, -400.0, -100.0))
            self.assertEqual(activation.upper_right, (500.0, 400.0, 300.0))
            self.assertEqual(activation.dimension, (8, 8, 4))
            self.assertGreater(activation.dimension[0] * activation.dimension[1] * activation.dimension[2], 0)
            self.assertNotEqual(activation.dimension, response.dimension)
            self.assertIn("all material-bearing domains", descriptor["photon_source_scope"])
            self.assertIn("starting mesh", descriptor["discretization_limit"])

            material = openmc.Material(material_id=801)
            material.add_nuclide("H1", 1.0)
            material.set_density("atom/b-cm", 0.01)
            neutron = openmc.Model(geometry=openmc.Geometry([openmc.Cell(cell_id=802, fill=material)]),
                                   materials=openmc.Materials([material]))
            neutron.tallies = openmc.Tallies([dose])
            d1s.prepare_tallies(neutron, nuclides=["H1"])
            self.assertTrue(any(isinstance(item, openmc.ParentNuclideFilter) for item in neutron.tallies[0].filters))
            self.assertFalse(any(isinstance(item, openmc.MeshBornFilter) for item in neutron.tallies[0].filters))
            photon_dose = R2S.dose_tally(openmc, response)
            self.assertFalse(any(isinstance(item, openmc.MeshBornFilter) for item in photon_dose.filters))
            photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials,
                                  tallies=openmc.Tallies([photon_dose]))
            manager = openmc.deplete.R2SManager(neutron, activation, photon_model=photon)
            self.assertIs(manager.domains, activation)
            self.assertIs(photon.tallies[0].filters[0].mesh, response)

    def test_actual_r3_bases_share_all_six_physical_hashes_across_mesh_id_orders(self):
        """Build actual OpenMC model/source objects; synthetic H5M/card are identity fixtures only."""
        import hashlib

        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            model_dir = base / "model"
            model_dir.mkdir()
            h5m = model_dir / "rm_m.h5m"
            h5m.write_bytes(b"synthetic H5M identity fixture, not a solver geometry\n")
            roles = [{"name": "plasma", "volume_id": 1, "tag": "void", "h5m_material_tag": None}]
            (model_dir / "rm_m_roles.json").write_text(json.dumps({"solids": roles}), encoding="utf-8")
            card = {"schema": "faris-rm-m-model-card/v1", "outputs": {
                "h5m_sha256": hashlib.sha256(h5m.read_bytes()).hexdigest()}}
            (model_dir / "rm_m_model_card.json").write_text(json.dumps(card), encoding="utf-8")
            resulting = {}
            for method in ("d1s", "r2s"):
                out = base / method
                code = ("import json,sys; from pathlib import Path; "
                        "sys.path.insert(0,sys.argv[1]); "
                        "from test_r3_source_serialization import _actual_basis_smoke; "
                        "print(json.dumps(_actual_basis_smoke(sys.argv[2],Path(sys.argv[3]),Path(sys.argv[4]))))")
                result = subprocess.run(
                    [sys.executable, "-c", code, str(Path(__file__).resolve().parent), method,
                     str(model_dir), str(out)], check=True, capture_output=True, text=True)
                resulting[method] = json.loads(result.stdout.strip().splitlines()[-1])

            self.assertEqual(set(resulting["d1s"]["hashes"]),
                             {"geometry", "materials", "source", "irradiation", "response", "location"})
            self.assertNotEqual(resulting["d1s"]["source_mesh_id"], resulting["r2s"]["source_mesh_id"])
            self.assertEqual(resulting["d1s"]["hashes"], resulting["r2s"]["hashes"])


if __name__ == "__main__":
    unittest.main()
