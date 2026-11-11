# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure checks for response-specific reactor stop targets (NUC-043)."""

import importlib.util
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)


class ResponseTargets(unittest.TestCase):
    def test_response_specific_targets_override_the_legacy_scalar(self):
        rule = WORKER.stop_rule({
            "batches": 100,
            "target_relative_error": 0.05,
            "watch_responses": ["magnets-fast-flux", "total-tritium-production"],
            "response_targets": {"magnets-fast-flux": 0.05, "total-tritium-production": 0.01},
        })
        self.assertEqual(rule["response_targets"], {
            "magnets-fast-flux": 0.05,
            "total-tritium-production": 0.01,
        })
        self.assertEqual(rule["target_relative_error"], 0.05)  # exact: authored plan threshold

    def test_target_map_cannot_name_an_unwatched_response(self):
        with self.assertRaises(ValueError):
            WORKER.stop_rule({
                "batches": 100,
                "target_relative_error": 0.05,
                "watch_responses": ["total-tritium-production"],
                "response_targets": {"magnets-fast-flux": 0.05},
            })

    def test_guard_collects_errors_after_an_unresolved_response(self):
        class StubRunningSum:
            def __init__(self):
                self.values = []

            def add(self, value):
                self.values.append(value)

            @property
            def relative_error(self):
                return None if all(value == 0 for value in self.values) else 0.04

            @property
            def variance_of_variance(self):
                return None

        old = WORKER.RunningSum
        WORKER.RunningSum = StubRunningSum
        try:
            rule = {"response_ids": ["unresolved", "resolved"], "response_targets": {"unresolved": 0.05, "resolved": 0.05}}
            verdict = WORKER.guard_verdict(rule, {"response-unresolved": [0.0, 0.0], "response-resolved": [1.0, 2.0]})
            self.assertEqual(verdict["relative_errors"], {"unresolved": None, "resolved": 0.04})
            self.assertFalse(verdict["met"])
        finally:
            WORKER.RunningSum = old

    def test_legacy_scalar_only_still_targets_magnet(self):
        rule = WORKER.stop_rule({"batches": 100, "target_relative_error": 0.03})
        self.assertEqual(rule["response_ids"], ["magnets-fast-flux"])
        self.assertEqual(rule["response_targets"], {"magnets-fast-flux": 0.03})

    def test_target_thresholds_are_applied_per_tally(self):
        class Trigger:
            def __init__(self, kind, threshold):
                self.kind = kind
                self.threshold = threshold
                self.scores = []

        OpenMC = type("OpenMC", (), {"Trigger": Trigger})

        class Tally:
            scores = ["flux"]
            triggers = []

        class Settings:
            pass

        rule = WORKER.stop_rule({
            "batches": 100,
            "target_relative_error": 0.05,
            "watch_responses": ["magnets-fast-flux", "total-tritium-production"],
            "response_targets": {"magnets-fast-flux": 0.05, "total-tritium-production": 0.01},
        })
        tallies = {"response-magnets-fast-flux": Tally(), "response-total-tritium-production": Tally()}
        settings = Settings()
        WORKER.apply_stop_rule(OpenMC, settings, tallies, rule)
        self.assertEqual(tallies["response-magnets-fast-flux"].triggers[0].threshold, 0.05)  # exact: authored trigger
        self.assertEqual(tallies["response-total-tritium-production"].triggers[0].threshold, 0.01)  # exact: authored trigger
        self.assertEqual(settings.batches, 30)

    def test_budget_and_unsettled_guards_apply_to_the_response_specific_limit(self):
        rule = WORKER.stop_rule({
            "batches": 40,
            "target_relative_error": 0.05,
            "watch_responses": ["total-tritium-production"],
            "response_targets": {"total-tritium-production": 0.01},
        })
        code, _ = WORKER.judge_stop(rule, 40, 0.02, 0.02, met=False, target=0.01)
        self.assertEqual(code, "budget_exhausted")
        code, text = WORKER.judge_stop(rule, 40, 0.009, 0.2, met=True, target=0.01)
        self.assertEqual(code, "target_unsettled")
        self.assertIn("not reliable", text)


if __name__ == "__main__":
    unittest.main()
