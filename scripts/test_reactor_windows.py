"""Weight windows for the reactor study's Production tier: the pure parts, with no OpenMC.

The generator (window_generator.py) is shared with `faris design transport`; the helper that points it at the
study's torus is reactor_windows.py; the worker (reactor_transport.py) only reads the finished file. The runs
that prove the windows do not bias the answer are in integrations/openmc/test_windows_csg_openmc.py.
"""
import importlib.util
import sys
import tempfile
import types
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

OPENMC = Path(__file__).resolve().parent.parent / "integrations" / "openmc"
sys.path.insert(0, str(OPENMC))

import design_transport as DT  # noqa: E402
import reactor_transport as RT  # noqa: E402
import reactor_windows as RW  # noqa: E402
import window_generator as WG  # noqa: E402


def request():
    return {"responses": [
        {"id": "magnets-fast-flux", "domain": {"kind": "component", "component_id": "magnets"},
         "score": {"kind": "flux_above", "energy_min_ev": 1.0e5}},
        {"id": "total-tritium-production", "domain": {"kind": "whole_model"},
         "score": {"kind": "particle_production", "particle": "tritium", "score": "H3-production"}},
    ]}


class SharedGenerator(unittest.TestCase):
    # Verifies: PERF-025
    def test_design_transport_and_reactor_study_use_one_implementation(self):
        for name in ("label_windows", "apply_windows", "export_model", "group_edges", "solve_windows",
                     "condense_multigroup", "library_entries", "window_grid", "response_lines"):
            self.assertIs(getattr(DT, name), getattr(WG, name), name)
        # the worker reads the mesh the generator labels, by the same id
        self.assertEqual(RT.WINDOW_MESH_ID, WG.WINDOW_MESH_ID)
        # and refuses the seeds the window steps use (NUC-053)
        self.assertEqual(sorted(RT.RESERVED_WINDOW_SEEDS), sorted((WG.MGXS_SEED, WG.WINDOW_SEED)))

    def test_group_structure_and_adjoint_lines_for_the_fast_threshold(self):
        edges = WG.group_edges(1.0e5)
        self.assertEqual(edges, WG.GROUP_EDGES_EV)
        lines = WG.response_lines(edges, 1.0e5)
        self.assertEqual(len(lines), 10)
        self.assertTrue(all(1.0e5 < line < 2.0e7 for line in lines))
        # a threshold that is not an edge is added as one
        self.assertIn(2.0e5, WG.group_edges(2.0e5))

    def test_library_entries_accept_the_ids_openmc_0_16_adds_and_refuse_ambiguity(self):
        stored = ["tungsten_natural_1", "flibe_cold_solid_2", "faris_filler_6"]
        names = ["tungsten-natural", "flibe-cold-solid", "faris-filler"]
        self.assertEqual(WG.library_entries(names, stored),
                         dict(zip(names, stored)))
        # the plain names of 0.15 still match
        self.assertEqual(WG.library_entries(["a-b"], ["a_b"]), {"a-b": "a_b"})
        with self.assertRaisesRegex(ValueError, "0 entries"):
            WG.library_entries(["missing"], stored)
        with self.assertRaisesRegex(ValueError, "2 entries"):
            WG.library_entries(["a"], ["a_1", "a_2"])

    def test_window_mesh_covers_the_torus_in_25_cm_cells(self):
        manifest = {"major_radius_m": 3.3, "variants": [{"components": [{"outer_minor_radius_m": 2.28}]}]}
        lower, upper, dimension = RW.bounds(manifest)
        self.assertEqual((lower, upper), ([-563.0, -233.0, -563.0], [563.0, 233.0, 563.0]))
        self.assertEqual(dimension, [45, 19, 45])

    def test_non_positive_totals_are_repaired_to_the_smallest_positive_one(self):
        self.assertEqual(WG.repaired_totals([0.2, 0.1, 0.3]), ([0.2, 0.1, 0.3], 0))
        self.assertEqual(WG.repaired_totals([3.0e-4, -5.0e-4, 0.0, 1.0e-3]), ([3.0e-4, 3.0e-4, 3.0e-4, 1.0e-3], 2))
        self.assertEqual(WG.repaired_totals([-1.0]), ([1.0e-4], 1))


class HelperInput(unittest.TestCase):
    def test_the_response_is_the_stop_rule_one_in_a_component(self):
        self.assertEqual(RW.window_response(request()), ("magnets", 1.0e5))
        missing = {"responses": request()["responses"][1:]}
        with self.assertRaisesRegex(ValueError, "does not define"):
            RW.window_response(missing)
        wrong = request()
        wrong["responses"][0]["score"] = {"kind": "flux"}
        with self.assertRaisesRegex(ValueError, "not a flux above"):
            RW.window_response(wrong)
        whole = request()
        whole["responses"][0]["domain"] = {"kind": "whole_model"}
        with self.assertRaisesRegex(ValueError, "not scored in a component"):
            RW.window_response(whole)

    def test_data_temperature_is_openmcs_rounded_label(self):
        def physics(*temperatures):
            return {"materials": [{"recipe": {"kind": "nuclide_mixture", "nuclear_data_temperature_k": t}}
                                  for t in temperatures] + [{"recipe": {"kind": "void"}}]}

        self.assertEqual(RW.data_temperature(physics(293.6, 293.6)), 294.0)
        with self.assertRaisesRegex(ValueError, "one data temperature"):
            RW.data_temperature(physics(293.6, 600.0))


class WorkerSide(unittest.TestCase):
    def sampling(self, target=0.05):
        sampling = {"batches": 100, "particles_per_batch": 1000, "seed": 1, "threads": 1}
        if target is not None:
            sampling["target_relative_error"] = target
        return sampling

    def plan(self, sha="0" * 64):
        return {"kind": "weight_windows", "response_id": "magnets-fast-flux", "windows_file": "windows.h5",
                "windows_sha256": sha}

    # Verifies: PERF-025
    def test_an_input_without_windows_is_analog_and_says_so(self):
        inp = {"sampling": self.sampling()}
        self.assertIsNone(RT.variance_reduction_plan(inp))
        self.assertEqual(RT.variance_reduction_receipt(inp), {"mode": "analog"})

    def test_windows_need_a_target_run_aimed_at_the_watched_response(self):
        inp = {"sampling": self.sampling(), "variance_reduction": self.plan()}
        self.assertEqual(RT.variance_reduction_plan(inp)["response_id"], "magnets-fast-flux")
        with self.assertRaisesRegex(ValueError, "target-error run"):
            RT.variance_reduction_plan({"sampling": self.sampling(None), "variance_reduction": self.plan()})
        other = dict(self.plan(), response_id="total-tritium-production")
        with self.assertRaisesRegex(ValueError, "another response"):
            RT.variance_reduction_plan({"sampling": self.sampling(), "variance_reduction": other})
        with self.assertRaisesRegex(ValueError, "unsupported"):
            RT.variance_reduction_plan({"sampling": self.sampling(), "variance_reduction": dict(self.plan(), kind="x")})

    def test_the_window_file_is_checked_against_its_hash_in_the_input(self):
        with tempfile.TemporaryDirectory() as scratch:
            run = Path(scratch)
            out = run / "solver"
            out.mkdir()
            (run / "windows.h5").write_bytes(b"windows")
            good = self.plan(RT.sha256(run / "windows.h5"))
            self.assertEqual(RT.windows_path(good, out), run / "windows.h5")
            with self.assertRaisesRegex(ValueError, "differs from the hash"):
                RT.windows_path(self.plan("1" * 64), out)
            with self.assertRaisesRegex(ValueError, "plain file name"):
                RT.windows_path(dict(good, windows_file="../windows.h5"), out)
            with self.assertRaisesRegex(ValueError, "missing"):
                RT.windows_path(dict(good, windows_file="other.h5"), out)

    def test_the_window_mesh_goes_into_the_settings_file(self):
        with tempfile.TemporaryDirectory() as scratch:
            settings = Path(scratch) / "settings.xml"
            settings.write_text("<?xml version='1.0' encoding='utf-8'?>\n<settings>\n  <run_mode>fixed source</run_mode>\n</settings>\n")
            mesh = types.SimpleNamespace(to_xml_element=lambda: ET.fromstring('<mesh id="9000"><dimension>1 1 1</dimension></mesh>'))
            RT.add_mesh_to_settings(settings, mesh)
            root = ET.parse(settings).getroot()
            self.assertEqual([m.get("id") for m in root.findall("mesh")], ["9000"])
            self.assertIsNotNone(root.find("run_mode"))


if __name__ == "__main__":
    unittest.main()
