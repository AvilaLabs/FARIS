# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

MODULE = Path(__file__).with_name("run_controlled_case.py")
SPEC = importlib.util.spec_from_file_location("faris_controlled_lifecycle", MODULE)
assert SPEC and SPEC.loader
controlled = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controlled)


class ControlledCaseTests(unittest.TestCase):
    def test_case_inputs_are_hash_pinned_and_authored(self):
        case, files = controlled.load_case()
        self.assertEqual(case["case_id"], "no-port-breeder-two-blanket-cycles")
        self.assertTrue(files["process"].is_file())
        self.assertTrue(files["bluemira"].is_file())
        self.assertTrue(files["faris"].is_file())
        self.assertEqual(case["authored_inputs"]["blanket_cycle_count"], 2)
        self.assertIn("not inferred material service life", case["authored_inputs"]["provenance"]["blanket_lifetime_full_power_years"])

    def test_match_requires_native_outputs_and_keeps_faris_separate(self):
        case, _ = controlled.load_case()
        horizon = case["authored_inputs"]["calendar_horizon_years"]
        availability = 1 - 2 * 60 / (365.25 * horizon)
        process = {
            "plant_horizon_years_input": horizon,
            "unplanned_unavailability": dict.fromkeys(("uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"), 0.0),
            "replacement_duration_years": {"blanket": 60 / 365.25},
            "plant_availability_fraction": availability,
            "capacity_factor": availability,
        }
        blue = {"calendar_years": horizon, "blanket_replacements": 2,
                "unplanned_downtime": 0.0, "overall_availability": availability,
                "target_global_availability": availability,
                "full_power_years": availability * horizon}
        faris = {"lifetime_net_electricity_mwh": 10.0, "total_replacement_downtime_s": 1.0,
                 "availability_fraction": 0.5}
        result = controlled.evaluate(case, process, blue, faris, {**faris, "lifetime_net_electricity_mwh": 12.0})
        self.assertEqual(result["status"], "matched controlled code-to-code baseline")
        self.assertEqual(result["faris_computed_minus_fixed"]["lifetime_net_electricity_mwh"], 2.0)
        blue["blanket_replacements"] = 3
        result = controlled.evaluate(case, process, blue, faris, faris)
        self.assertEqual(result["status"], "incomplete; native output checks did not all match")
        self.assertFalse(result["checks"]["bluemira_two_blanket_replacements"])


if __name__ == "__main__":
    unittest.main()
