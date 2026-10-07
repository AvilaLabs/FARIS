import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import test_maintenance_coupling_test as T  # noqa: E402

MC = T.MC
RE = T.load("restart_equivalence", "restart_equivalence.py")


class RelativeDifferenceTests(unittest.TestCase):
    def test_difference_is_relative_to_the_larger_value_with_a_floor_from_the_curve_maximum(self):
        self.assertEqual(RE.max_relative_difference([1.0, 2.0], [1.0, 2.0]), 0.0)
        self.assertAlmostEqual(RE.max_relative_difference([1.0, 2.0], [1.0, 1.0]), 0.5)
        # a value far below the curve's maximum is judged against 1e-9 of that maximum, not against itself
        self.assertAlmostEqual(RE.max_relative_difference([1.0, 0.0], [1.0, 1e-12]), 1e-12 / 1e-9)
        self.assertEqual(RE.max_relative_difference([0.0, 0.0], [0.0, 0.0]), 0.0)
        self.assertIsNone(RE.max_relative_difference([None, None], [None, None]))
        with self.assertRaises(ValueError):
            RE.max_relative_difference([1.0], [1.0, 2.0])
        with self.assertRaises(ValueError):
            RE.max_relative_difference([1.0], [None])


class EquivalenceScriptTests(unittest.TestCase):
    def build_full(self, d, dose):
        rig = T.Rig(Path(d), T.AmendmentTwoTests.HOT, f_values=(1.0,), sweep_labels=("0.30",), amendment=2,
                    horizon_y=8.0)
        T.write_exe(Path(d) / "actinv", T.FAKE_ACTINV_RESTARTABLE)
        if dose:
            (Path(d) / "response.json").write_text('{"schema": "actinv-photon-response-1"}\n')
            rig.config["photon_response"] = "response.json"
        cfg = MC.resolve_paths(rig.config, Path(d))
        base = json.loads((Path(d) / "assumptions.json").read_text())
        (Path(d) / "full").mkdir()
        runner = MC.Runner(cfg, Path(d) / "full", base, MC.CLASSES)
        runner.decay_curves("port/reference", cfg["arrangements"]["port/reference"], 1.0, {}, MC.BARE)
        return next((Path(d) / "full").rglob("decay-times.json")).parent / "decay", Path(d) / "full" / "decay-cache"

    def run_script(self, d, spec_dir, cache, *extra):
        out = Path(d) / "equiv.json"
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = RE.main(["--spec-dir", str(spec_dir), "--decay-cache", str(cache), "--actinv", str(Path(d) / "actinv"),
                            "--data-dir", str(Path(d) / "data"), "--output", str(out), *extra])
        return code, err.getvalue(), (json.loads(out.read_text()) if out.exists() else None)

    def test_restart_matches_full_for_heat_and_dose_and_reports_the_floor_figures(self):
        for dose in (False, True):
            with tempfile.TemporaryDirectory() as d:
                spec_dir, cache = self.build_full(d, dose)
                code, err, res = self.run_script(d, spec_dir, cache, "--workers", "2")
                self.assertEqual(code, 0, err)
                self.assertTrue(res["passed"])
                self.assertEqual(res["tolerance"], 1e-6)
                self.assertLessEqual(res["worst_relative_difference"], 1e-12)
                self.assertGreater(len(res["curves"]), 1)
                for row in res["curves"]:
                    self.assertIsNotNone(row["heat_max_relative_difference"])
                    self.assertEqual(row["dose_max_relative_difference"] is not None, dose)
                    self.assertIsNotNone(row["n_states_below_floor"])
                    self.assertIn("heat_bound_from_below_floor_W_per_g", row)
                self.assertEqual(res["runs"]["trunk_cache_hits"], 0)
                # the run's own folders are untouched: restart specs live under the work directory
                self.assertEqual(list(spec_dir.glob("restart")), [])
                self.assertTrue(list((Path(d) / "equiv.json.work").rglob("*.trunk.spec.json")))

    def test_a_subset_can_be_sampled_and_a_tight_tolerance_can_fail(self):
        with tempfile.TemporaryDirectory() as d:
            spec_dir, cache = self.build_full(d, False)
            code, err, res = self.run_script(d, spec_dir, cache, "--components", "first-wall",
                                             "--limit-installations", "1")
            self.assertEqual(code, 0, err)
            self.assertEqual({(r["component"], r["installation"]) for r in res["curves"]}, {("first-wall", 1)})
            code, err, res = self.run_script(d, spec_dir, cache, "--components", "nothing")
            self.assertEqual(code, 2)
            self.assertIn("no continuation provenance", err)

    def test_a_difference_beyond_the_tolerance_fails(self):
        with tempfile.TemporaryDirectory() as d:
            spec_dir, cache = self.build_full(d, False)
            for points in cache.glob("*.points.json"):  # corrupt the full curves
                doc = json.loads(points.read_text())
                doc["steps"] = [[s[0], s[1] * 1.001, s[2], s[3]] for s in doc["steps"]]
                points.write_text(json.dumps(doc))
            code, err, res = self.run_script(d, spec_dir, cache, "--tolerance", "1e-6")
            self.assertEqual(code, 1)
            self.assertFalse(res["passed"])
            self.assertGreater(res["worst_relative_difference"], 5e-4)


if __name__ == "__main__":
    unittest.main()
