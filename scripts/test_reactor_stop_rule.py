"""The reactor worker's target-error stop rule: trigger settings and the relaxed integrity checks.

No OpenMC is needed: a stand-in module supplies the few OpenMC classes the rule configures. The run
that proves OpenMC really stops early is in integrations/openmc/test_stop_rule_openmc.py.
"""
import importlib.util
import tempfile
import types
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ADAPTER = load("reactor_transport", ROOT.parent / "integrations" / "openmc" / "reactor_transport.py")


class FakeTrigger:
    def __init__(self, trigger_type, threshold, ignore_zeros=False):
        self.trigger_type, self.threshold, self.scores = trigger_type, threshold, []


class FakeTally:
    def __init__(self, name, scores):
        self.name, self.scores, self.triggers = name, scores, []


FAKE_OPENMC = types.SimpleNamespace(Trigger=FakeTrigger)


def plan(batches=200, target=0.05):
    sampling = {"batches": batches, "particles_per_batch": 1000, "seed": 1, "threads": 1}
    if target is not None:
        sampling["target_relative_error"] = target
    return sampling


class StopRuleSettings(unittest.TestCase):
    # Verifies: NUC-043
    def test_fixed_plan_has_no_rule_and_changes_nothing(self):
        self.assertIsNone(ADAPTER.stop_rule(plan(target=None)))
        settings = types.SimpleNamespace(batches=200, trigger_active=None)
        ADAPTER.apply_stop_rule(FAKE_OPENMC, settings, {}, None)
        self.assertEqual((settings.batches, settings.trigger_active), (200, None))

    # Verifies: PERF-025, NUC-043
    def test_target_sets_openmc_triggers_on_the_magnet_flux_only(self):
        rule = ADAPTER.stop_rule(plan(batches=200, target=0.05))
        self.assertEqual(rule["min_batches"], 30)
        self.assertEqual(rule["max_batches"], 200)
        magnet = FakeTally("response-magnets-fast-flux", ["flux"])
        other = FakeTally("response-total-tritium-production", ["H3-production"])
        settings = types.SimpleNamespace()
        ADAPTER.apply_stop_rule(FAKE_OPENMC, settings, {magnet.name: magnet, other.name: other}, rule)
        self.assertEqual(settings.batches, 30)
        self.assertTrue(settings.trigger_active)
        self.assertEqual(settings.trigger_max_batches, 200)
        self.assertEqual(settings.trigger_batch_interval, 1)
        self.assertEqual(other.triggers, [])
        (trigger,) = magnet.triggers
        self.assertEqual((trigger.trigger_type, trigger.threshold, trigger.scores), ("rel_err", 0.05, ["flux"]))

    def test_bad_targets_and_missing_response_are_refused(self):
        for target in (0.0, -1.0, 0.6, float("nan")):
            with self.assertRaises(ValueError):
                ADAPTER.stop_rule(plan(target=target))
        with self.assertRaises(ValueError):
            ADAPTER.stop_rule(plan(batches=29))
        with self.assertRaises(ValueError):
            ADAPTER.apply_stop_rule(FAKE_OPENMC, types.SimpleNamespace(), {}, ADAPTER.stop_rule(plan()))


class StopJudgement(unittest.TestCase):
    RULE = {"response_id": "magnets-fast-flux", "target_relative_error": 0.05, "min_batches": 30, "max_batches": 100, "batch_interval": 1}

    # Verifies: NUC-043
    def test_reasons(self):
        self.assertEqual(ADAPTER.judge_stop(None, 100, None)[0], "fixed_count")
        self.assertEqual(ADAPTER.judge_stop(self.RULE, 41, 0.049)[0], "target_met")
        self.assertEqual(ADAPTER.judge_stop(self.RULE, 100, 0.049)[0], "target_met")
        self.assertEqual(ADAPTER.judge_stop(self.RULE, 100, 0.08)[0], "budget_exhausted")
        self.assertEqual(ADAPTER.judge_stop(self.RULE, 100, None)[0], "budget_exhausted")

    def test_unexplained_stops_are_refused(self):
        for realized, achieved in ((41, 0.08), (41, None), (29, 0.01), (101, 0.08)):
            with self.assertRaises(ValueError, msg=f"{realized} {achieved}"):
                ADAPTER.judge_stop(self.RULE, realized, achieved)


class BatchFiles(unittest.TestCase):
    def read(self, batches_written, low, high):
        def fake_read(path, names):
            batch = int(Path(path).name.split(".")[1])
            return batch, batch, {"t": float(batch)}

        saved = ADAPTER.read_scalar_sums
        ADAPTER.read_scalar_sums = fake_read
        try:
            with tempfile.TemporaryDirectory() as folder:
                out = Path(folder)
                for batch in batches_written:
                    (out / f"statepoint.{batch}.h5").write_bytes(b"")
                result = ADAPTER.read_batch_sums(None, out, low, high, ["t"])
                return result, sorted(p.name for p in out.glob("statepoint.*.h5"))
        finally:
            ADAPTER.read_scalar_sums = saved

    # Verifies: NUC-043
    def test_any_count_in_range_is_accepted_and_only_the_last_file_is_kept(self):
        (sums, realized), left = self.read(range(1, 48), 30, 100)
        self.assertEqual(realized, 47)
        self.assertEqual(sums["t"][:3], [1.0, 2.0, 3.0])
        self.assertEqual(left, ["statepoint.47.h5"])

    def test_fixed_run_still_needs_exactly_its_count(self):
        (_, realized), _ = self.read(range(1, 101), 100, 100)
        self.assertEqual(realized, 100)
        with self.assertRaises(ValueError):
            self.read(range(1, 99), 100, 100)

    def test_gaps_and_out_of_range_are_refused(self):
        for written in (list(range(1, 20)), list(range(1, 101)) + [101], [b for b in range(1, 50) if b != 10]):
            with self.assertRaises(ValueError):
                self.read(written, 30, 100)


if __name__ == "__main__":
    unittest.main()
