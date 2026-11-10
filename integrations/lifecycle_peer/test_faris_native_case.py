# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).with_name("run_faris_native_case.py")
SPEC = importlib.util.spec_from_file_location("faris_native_lifecycle_case", MODULE)
assert SPEC and SPEC.loader
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


class FarisNativeLifecycleCaseTests(unittest.TestCase):
    def test_case_files_hash_and_shared_peer_horizon(self):
        files = native.verify_case_files()
        peer_case = json.loads((MODULE.parent / "cases/controlled_fixed/manifest.json").read_text())
        self.assertAlmostEqual(peer_case["authored_inputs"]["calendar_horizon_years"],
                               native.CASE["matched_case"]["horizon_years"], places=12)
        self.assertAlmostEqual(peer_case["authored_inputs"]["fusion_power_mw"], 525.0)
        self.assertIn("not experimentally validated", native.CASE["scope"])
        self.assertEqual(set(files), {"scenario", "physics", "history_template", "maintenance_assumptions"})

    def test_preflight_rejects_calibration_target_not_generated_by_runner(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "assumptions.json"
            assumptions = json.loads(native.verify_case_files()["maintenance_assumptions"].read_text())
            assumptions["classes"]["blanket"]["threshold"]["design"] = "stale-design-id"
            path.write_text(json.dumps(assumptions))
            with self.assertRaisesRegex(ValueError, "stale-design-id.*generated designs"):
                native.validate_calibration_targets(path, {native.CASE["case_id"]})

    def test_threshold_comes_from_native_transport_rate_and_authored_interval(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            rates = root / "rates.json"
            history = root / "history.json"
            template = native.verify_case_files()["history_template"]
            rates.write_text(json.dumps({
                "component_average_flux_n_m2_s": {
                    "blanket": {"mean": 123.0, "unit": "neutrons/m²/s", "response_id": "blanket-flux"}
                }
            }))
            result = native.make_history(rates, template, history)
            authored = json.loads(history.read_text())
            self.assertAlmostEqual(result["integrated_flux_limit_n_m2"],
                                   123.0 * native.CASE["matched_case"]["blanket_lifetime_full_power_seconds"])
            self.assertAlmostEqual(authored["service_limits"][0]["limit"], result["integrated_flux_limit_n_m2"])
            self.assertEqual(authored["planned_outages"], [])
            self.assertIn("not a material limit", authored["service_limits"][0]["provenance"])
            self.assertEqual(len(authored["service_limits"]), 1)

    def test_native_status_requires_fixed_and_computed_histories_at_common_horizon(self):
        h = native.CASE["matched_case"]["horizon_s"]
        d = native.CASE["matched_case"]["fixed_blanket_replacement_seconds"]
        fixed = {"history_end_s": h, "outcome": "horizon_completed",
                 "replacements": [
                     {"class": "blanket", "start_s": 10.0, "end_s": 10.0 + d},
                 ], "lifetime_net_electricity_mwh": 99.0}
        computed = {"status": "EVALUATED", "summary": {
            "history_end_s": h, "outcome": "horizon_completed",
            "replacements": [{"class": "blanket", "start_s": 10.0, "end_s": 10.0 + d}],
            "lifetime_net_electricity_mwh": 88.0,
        }}
        result = {"schema_version": "faris-maintenance-result/v0.1",
                  "designs": {native.CASE["case_id"]: {
                      "fixed": fixed, "computed": computed,
                      "thresholds": {"blanket": {"status": "EVALUATED", "source": "calibrated"}},
                  }}}
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "result.json"
            path.write_text(json.dumps(result))
            summary = native.native_check(path)
        self.assertTrue(all(summary["checks"].values()))
        self.assertEqual(summary["faris_native_energy_kept_separate"]["fixed_lifetime_net_electricity_mwh"], 99.0)

        result["designs"][native.CASE["case_id"]]["computed"]["status"] = "NOT_EVALUATED"
        result["designs"][native.CASE["case_id"]]["computed"]["summary"] = None
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "result.json"
            path.write_text(json.dumps(result))
            summary = native.native_check(path)
        self.assertFalse(summary["checks"]["computed_history_evaluated"])
        self.assertIn("incomplete", summary["status"])


if __name__ == "__main__":
    unittest.main()
