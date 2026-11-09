# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import ast
import importlib.util
from pathlib import Path
import unittest

MODULE = Path(__file__).with_name("run_controlled_case.py")
SPEC = importlib.util.spec_from_file_location("faris_controlled_lifecycle", MODULE)
assert SPEC and SPEC.loader
controlled = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controlled)


class ControlledCaseTests(unittest.TestCase):
    def test_process_worker_uses_pinned_dataclass_location(self):
        worker = Path(__file__).with_name("process_availability_worker.py")
        tree = ast.parse(worker.read_text(encoding="utf-8"))
        imports = {
            node.module: {alias.name for alias in node.names}
            for node in ast.walk(tree)
            if isinstance(node, ast.ImportFrom)
        }
        self.assertIn("DataStructure", imports.get("process.core.model", set()))

    def test_case_inputs_are_hash_pinned_and_authored(self):
        case, files = controlled.load_case()
        self.assertEqual(case["case_id"], "no-port-breeder-one-boundary-outage-control")
        self.assertTrue(files["process"].is_file())
        self.assertTrue(files["bluemira"].is_file())
        self.assertTrue(files["faris"].is_file())
        self.assertEqual(case["authored_inputs"]["blanket_cycle_count"], 2)
        self.assertEqual(case["authored_inputs"]["expected_blanket_replacement_count"], 1)
        self.assertIn("not inferred material service life", case["authored_inputs"]["provenance"]["blanket_lifetime_full_power_years"])

    def test_match_requires_native_outputs_and_keeps_faris_separate(self):
        case, _ = controlled.load_case()
        horizon = case["authored_inputs"]["calendar_horizon_years"]
        campaign_life = case["authored_inputs"]["blanket_lifetime_full_power_years"]
        outage_years = case["authored_inputs"]["fixed_blanket_replacement_days"] / 365.25
        availability = 2 * campaign_life / (2 * campaign_life + outage_years)
        process = {
            "plant_horizon_years_input": horizon,
            "unplanned_unavailability": dict.fromkeys(("uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"), 0.0),
            "replacement_duration_years": {"blanket": 60 / 365.25},
            "pulse_burn_seconds": case["authored_inputs"]["pulse_period_seconds"],
            "pulse_total_seconds": case["authored_inputs"]["pulse_period_seconds"],
            "plant_availability_fraction": availability,
            "capacity_factor": availability,
        }
        blue = {"calendar_years": horizon, "blanket_replacements": 1,
                "unplanned_downtime": 0.0, "overall_availability": availability,
                "operational_load_factor_input": 1.0,
                "planned_maintenance": 60 * 86400,
                "divertor_replacements": 4,
                "full_power_years": availability * horizon}
        faris = {"lifetime_net_electricity_mwh": 10.0, "total_replacement_downtime_s": 1.0,
                 "availability_fraction": 0.5}
        result = controlled.evaluate(case, process, blue, faris, {**faris, "lifetime_net_electricity_mwh": 12.0})
        self.assertEqual(result["status"], "matched controlled code-to-code baseline")
        self.assertEqual(result["faris_computed_minus_fixed"]["lifetime_net_electricity_mwh"], 2.0)
        blue["blanket_replacements"] = 2
        result = controlled.evaluate(case, process, blue, faris, faris)
        self.assertEqual(result["status"], "incomplete; native output checks did not all match")
        self.assertFalse(result["checks"]["bluemira_one_boundary_blanket_replacement"])

    def test_native_availability_disagreement_stays_incomplete(self):
        case, _ = controlled.load_case()
        horizon = case["authored_inputs"]["calendar_horizon_years"]
        campaign_life = case["authored_inputs"]["blanket_lifetime_full_power_years"]
        outage_years = case["authored_inputs"]["fixed_blanket_replacement_days"] / 365.25
        availability = 2 * campaign_life / (2 * campaign_life + outage_years)
        process = {
            "plant_horizon_years_input": horizon,
            "unplanned_unavailability": dict.fromkeys(("uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"), 0.0),
            "replacement_duration_years": {"blanket": 60 / 365.25},
            "pulse_burn_seconds": case["authored_inputs"]["pulse_period_seconds"],
            "pulse_total_seconds": case["authored_inputs"]["pulse_period_seconds"],
            "plant_availability_fraction": availability,
            "capacity_factor": availability,
        }
        blue = {"calendar_years": horizon, "blanket_replacements": 1,
                "unplanned_downtime": 0.0, "overall_availability": availability - 2e-4,
                "operational_load_factor_input": 1.0, "planned_maintenance": 60 * 86400,
                "divertor_replacements": 4, "full_power_years": availability * horizon}
        faris = {"lifetime_net_electricity_mwh": 10.0, "total_replacement_downtime_s": 1.0,
                 "availability_fraction": 0.5}
        result = controlled.evaluate(case, process, blue, faris, faris)
        self.assertEqual(result["status"], "incomplete; native output checks did not all match")
        self.assertFalse(result["checks"]["peer_availability_agrees"])


if __name__ == "__main__":
    unittest.main()
