"""Tests for fwcadis_spike.py. Run with the OpenMC 0.15.3 environment's python.

Nothing here runs OpenMC transport, MGXS generation or random ray.
"""

import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest

import fwcadis_spike as FW

CONTROL = Path("/home/connoravila/.cache/avila-night/faris-wt/campaign-20261005/runs/control-reference")


def manifest(penetration=None):
    return {
        "major_radius_m": 3.3,
        "penetration": penetration,
    }, {"components": [{"id": "vessel", "inner_minor_radius_m": 2.01, "outer_minor_radius_m": 2.06},
                       {"id": "magnets", "inner_minor_radius_m": 2.12, "outer_minor_radius_m": 2.28}]}


class Extents(unittest.TestCase):
    def test_control_extents_from_scenario(self):
        m, v = manifest()
        e = FW.peak_mesh_extents(m, v)
        self.assertEqual(e["outboard"]["lower_left_m"], [5.42, -0.15, -0.15])
        self.assertEqual(e["outboard"]["upper_right_m"], [5.58, 0.15, 0.15])
        self.assertEqual(e["inboard"]["lower_left_m"], [1.02, -0.15, -0.15])
        self.assertEqual(e["inboard"]["upper_right_m"], [1.18, 0.15, 0.15])
        self.assertEqual(e["outboard"]["dimensions"], e["inboard"]["dimensions"])

    def test_port_bounds_set_transverse_extent(self):
        m, v = manifest({"bounds_m": {"minimum_xyz_m": [4.34, -0.2, -0.1], "maximum_xyz_m": [5.59, 0.2, 0.1]}})
        e = FW.peak_mesh_extents(m, v)
        self.assertEqual(e["outboard"]["lower_left_m"], [5.42, -0.2, -0.1])
        self.assertEqual(e["inboard"]["upper_right_m"], [1.18, 0.2, 0.1])

    def test_ww_mesh_covers_torus(self):
        m, v = manifest()
        w = FW.ww_mesh_extents(m, v)
        self.assertGreaterEqual(w["upper_right_m"][0], 3.3 + 2.28)
        self.assertGreaterEqual(w["upper_right_m"][1], 2.28)
        self.assertEqual(w["dimensions"], [56, 24, 56])

    def test_missing_magnets_fails(self):
        m, v = manifest()
        v["components"].pop()
        with self.assertRaises(RuntimeError):
            FW.peak_mesh_extents(m, v)


class Helpers(unittest.TestCase):
    def test_709_group_bounds(self):
        b = FW.load_709_bounds()
        self.assertEqual(len(b), 710)
        self.assertTrue(all(b[i] < b[i + 1] for i in range(709)))
        self.assertAlmostEqual(float(b[0]), 1e-5)
        self.assertAlmostEqual(float(b[-1]), 1e9)

    def test_source_group_isolated(self):
        g = FW.source_group(FW.RR_GROUP_EDGES_EV, 14.1e6)
        self.assertEqual((g["lower_ev"], g["upper_ev"]), (1.35e7, 1.45e7))
        self.assertEqual(g["group_index_1_based_high_to_low"], 2)
        self.assertIn(FW.FAST_FLUX_LOWER_EV, FW.RR_GROUP_EDGES_EV)

    def test_vov(self):
        import numpy as np
        flat = FW.variance_of_variance(np.ones((10, 2)))
        self.assertTrue(np.isnan(flat).all())
        x = np.zeros((10, 1))
        x[0, 0] = 1.0
        spiky = FW.variance_of_variance(x)[0]
        smooth = FW.variance_of_variance(np.arange(10.0).reshape(10, 1) + 5.0)[0]
        self.assertGreater(spiky, smooth)


class Refusals(unittest.TestCase):
    def run_main(self, argv):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code = FW.main(argv)
        return code, err.getvalue()

    def test_generate_refuses_without_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            code, err = self.run_main(["generate", "--input", "/nonexistent.json", "--work-dir", tmp])
            self.assertEqual(code, 1)
            self.assertIn("--i-have-the-cpu", err)
            self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_compare_refuses_without_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            code, err = self.run_main(["compare", "--input", "/nonexistent.json", "--generate-record", "/nonexistent.json", "--work-dir", tmp])
            self.assertEqual(code, 1)
            self.assertIn("--i-have-the-cpu", err)
            self.assertEqual(list(Path(tmp).iterdir()), [])


class RandomRayConfig(unittest.TestCase):
    """configure_random_ray on a stand-in model: every key must pass OpenMC 0.15.3's own validation."""

    def test_configuration_and_keys(self):
        openmc = FW.import_openmc()
        mat = openmc.Material(name="m")
        mat.add_nuclide("Fe56", 1.0)
        mat.set_density("g/cm3", 7.8)
        sphere = openmc.Sphere(r=100.0, boundary_type="vacuum")
        inner = openmc.Cell(fill=mat, region=-sphere)
        model = openmc.Model(geometry=openmc.Geometry([inner]), materials=openmc.Materials([mat]))
        mesh = openmc.RegularMesh()
        mesh.dimension = (4, 4, 4)
        mesh.lower_left = (-100, -100, -100)
        mesh.upper_right = (100, 100, 100)
        tallies = openmc.Tallies()
        t = openmc.Tally(name="rr")
        t.filters = [openmc.MeshFilter(mesh), openmc.EnergyFilter([FW.FAST_FLUX_LOWER_EV, FW.RR_GROUP_EDGES_EV[-1]])]
        t.scores = ["flux"]
        tallies.append(t)
        FW.configure_random_ray(openmc, model, mesh, FW.PLAN_DEFAULTS, FW.RR_GROUP_EDGES_EV, tallies,
                                ([-100.0] * 3, [100.0] * 3), FW.subdivision_domains(model))
        d = FW.describe_random_ray(model)
        self.assertEqual(d["weight_window_generator"]["method"], "fw_cadis")
        self.assertEqual(d["weight_window_generator"]["particle_type"], "neutron")
        self.assertEqual(d["energy_mode"], "multi-group")
        for key in ("adjoint", "distance_inactive_cm", "distance_active_cm", "ray_source_box_cm", "source_region_meshes"):
            self.assertIn(key, d["random_ray"])
        self.assertEqual(model.settings.weight_window_generators[0].energy_bounds, FW.RR_GROUP_EDGES_EV)
        xml = Path(tempfile.mkdtemp())
        model.export_to_model_xml(path=str(xml / "model.xml"))
        text = (xml / "model.xml").read_text()
        self.assertIn("fw_cadis", text)
        self.assertIn("source_region_meshes", text)


@unittest.skipUnless((CONTROL / "input.json").is_file(), "control-reference request not present")
class ControlReference(unittest.TestCase):
    """Builds the real model through the adapter (XML export only)."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.tmp.name)
        with contextlib.redirect_stdout(io.StringIO()):
            cls.code = FW.main(["mgxs-plan", "--input", str(CONTROL / "input.json"), "--work-dir", str(cls.root)])
        cls.plan = json.loads((cls.root / "plan" / "mgxs-plan.json").read_text())

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_plan_written_and_not_executed(self):
        self.assertEqual(self.code, 0)
        self.assertFalse(self.plan["executed"])
        wwg = self.plan["configuration"]["weight_window_generator"]
        self.assertEqual(wwg["method"], "fw_cadis")
        self.assertEqual(wwg["particle_type"], "neutron")
        rr = self.plan["configuration"]["random_ray"]
        for key in ("adjoint", "distance_inactive_cm", "distance_active_cm", "source_region_meshes", "ray_source_box_cm"):
            self.assertIn(key, rr)
        cm = self.plan["convert_to_multigroup"]
        for key in ("method", "groups_ev_ascending", "nparticles", "correction"):
            self.assertIn(key, cm)
        self.assertIn(1e5, cm["groups_ev_ascending"])
        self.assertTrue(self.plan["void"]["null_filled_cells"])

    def test_adapter_model_unchanged(self):
        # compose() is the adapter's own; its exported XML must match the XML of the recorded run.
        adapter = self.root / "plan" / "build" / "adapter-export"
        for name in ("geometry.xml", "materials.xml", "settings.xml", "tallies.xml"):
            recorded = CONTROL / "solver" / name
            if recorded.is_file():
                self.assertEqual(FW.sha256(adapter / name), FW.sha256(recorded), name)

    def test_spike_keeps_geometry_and_materials(self):
        build = self.root / "plan" / "build"
        for name in ("geometry.xml", "materials.xml"):
            self.assertEqual(FW.sha256(build / "adapter-export" / name), FW.sha256(build / "spike-export" / name))
        tallies = (build / "spike-export" / "tallies.xml").read_text()
        for name in ("peak-outboard-fast-flux", "peak-inboard-fast-flux", "spectrum-709-vessel", "spectrum-709-magnets"):
            self.assertIn(name, tallies)


if __name__ == "__main__":
    unittest.main()
