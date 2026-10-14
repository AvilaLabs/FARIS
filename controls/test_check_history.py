"""Runs the exact-decimal history control on every CI pass (QA-042).

`check_history.py` is a command-line control, so the CI step that discovers `test_*.py` never ran
it. This wrapper loads it, runs its own analytic cases, and replays it over the recorded engine
histories kept in `references/`, which re-computes burn, production and the site mass balance from
the source rates in 45-digit decimals. It checks recorded engine output; a freshly generated history
is checked by `scripts/make_history_refinement_report.py`.
"""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECORDED = [
    ROOT / "references/operating-history-processing-control-phases-history.json",
    ROOT / "references/operating-history-processing-control-restart-history.json",
]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


CHECK = load_module("check_history", Path(__file__).with_name("check_history.py"))


class HistoryControlInCI(unittest.TestCase):
    # Verifies: QA-042
    def test_the_decimal_controls_pass(self):
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(CHECK.HistoryControlTests)
        result = unittest.TextTestRunner(stream=open("/dev/null", "w"), verbosity=0).run(suite)
        self.assertGreater(result.testsRun, 0)
        self.assertTrue(result.wasSuccessful(), result.failures + result.errors)

    # Verifies: QA-042
    def test_recorded_engine_histories_close_in_decimals(self):
        for path in RECORDED:
            summary, _ = CHECK.verify_engine_history(str(path), None)
            self.assertEqual(summary["independent_source_rate_production_burn_checks"], "PASS", path.name)
            self.assertLessEqual(
                float(summary["max_decimal_balance_residual_kg"]), summary["mass_balance_tolerance_kg"], path.name
            )
            self.assertLessEqual(summary["max_engine_balance_residual_kg"], summary["mass_balance_tolerance_kg"])
            for audit in summary["continuous_processing_phase_audits"].values():
                # A phase the history does not reach is NOT_EVALUATED; it is never a FAIL.
                self.assertTrue(audit["status"] == "PASS" or audit["status"].startswith("NOT_EVALUATED"), audit)


if __name__ == "__main__":
    unittest.main()
