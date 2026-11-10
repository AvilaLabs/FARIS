# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import ast
import importlib.util
import json
import math
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

MODULE = Path(__file__).with_name("run_controlled_case.py")
SPEC = importlib.util.spec_from_file_location("faris_controlled_lifecycle", MODULE)
assert SPEC and SPEC.loader
controlled = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controlled)


class ControlledCaseTests(unittest.TestCase):
    @staticmethod
    def blue_metrics(calendar_years=5.225442889963153, full_power_years=0.9685633442013221 * 5.225442889963153):
        return {
            "version": "2.14.0",
            "calendar_years": calendar_years,
            "full_power_years": full_power_years,
            "overall_availability": full_power_years / calendar_years,
            "operational_load_factor_input": 1.0,
            "planned_maintenance": 5_184_000.0,
            "ramp_time": 0.0,
            "unplanned_downtime": 0.0,
            "blanket_replacements": 1,
            "divertor_replacements": 4,
            "continuous_damage_budget_fpy_input": 5.061190855587917,
            "source_api": "bluemira.fuel_cycle.lifecycle.LifeCycle.make_timeline",
        }

    @staticmethod
    def timeline_record(calendar_years, full_power_years):
        return {
            "time": [0.0, calendar_years],
            "fusion_time": [0.0, full_power_years],
            "DT_rate": [0.0, 1.8618137864158527e20],
            "DD_rate": [0.0, 0.0],
            "blanket_change_index": 85074,
            "A_global": 1.0,
        }

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
        process = json.loads(files["process"].read_text(encoding="utf-8"))
        short_life = process["blanket_lifetime_fpy"]
        long_life = process["divertor_lifetime_fpy"]
        self.assertEqual(long_life, 2 * short_life)
        self.assertEqual(math.ceil(long_life / short_life) - 1, 1)
        self.assertIn("not a divertor service-life claim", process["provenance"]["divertor_lifetime_fpy"]["reference"])

    def test_near_returns_false_for_missing_nonfinite_and_boolean_operands(self):
        self.assertFalse(controlled.near(None, 0.0))
        self.assertFalse(controlled.near(0.0, None))
        self.assertFalse(controlled.near(float("nan"), 0.0))
        self.assertFalse(controlled.near(0.0, float("inf")))
        self.assertFalse(controlled.near(True, 1.0))

    def test_bluemira_metrics_are_routed_from_worker_stdout_and_checked_against_native_timeline(self):
        calendar_years = 5.2254429994676395
        full_power_years = 5.059375617917712
        timeline = self.timeline_record(calendar_years, full_power_years)
        metrics = self.blue_metrics(calendar_years, full_power_years)
        stdout = "Rich console log before JSON\n" + json.dumps(metrics) + "\n"

        routed = controlled.bluemira_output_metrics(timeline, stdout)

        self.assertTrue(routed["native_timeline_artifact_matches_metrics"])
        self.assertAlmostEqual(routed["calendar_years"], timeline["time"][-1])
        self.assertAlmostEqual(routed["full_power_years"], timeline["fusion_time"][-1])
        self.assertAlmostEqual(routed["overall_availability"], timeline["fusion_time"][-1] / timeline["time"][-1])
        self.assertNotEqual(routed["full_power_years"], routed["continuous_damage_budget_fpy_input"])
        self.assertNotIn("overall_availability", timeline)

    def test_missing_bluemira_metric_does_not_fall_back_to_continuous_fpy(self):
        calendar_years = 5.2254429994676395
        full_power_years = 5.059375617917712
        timeline = self.timeline_record(calendar_years, full_power_years)
        metrics = self.blue_metrics(calendar_years, full_power_years)
        del metrics["overall_availability"]

        routed = controlled.bluemira_output_metrics(timeline, json.dumps(metrics))

        self.assertFalse(routed["native_timeline_artifact_matches_metrics"])
        self.assertFalse(controlled.near(routed.get("overall_availability"), 0.9685633442013221))

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
                "full_power_years": availability * horizon, "ramp_time": 0.0,
                "native_timeline_artifact_matches_metrics": True}
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
                "divertor_replacements": 4, "full_power_years": availability * horizon, "ramp_time": 0.0,
                "native_timeline_artifact_matches_metrics": True}
        faris = {"lifetime_net_electricity_mwh": 10.0, "total_replacement_downtime_s": 1.0,
                 "availability_fraction": 0.5}
        result = controlled.evaluate(case, process, blue, faris, faris)
        self.assertEqual(result["status"], "incomplete; native output checks did not all match")
        self.assertFalse(result["checks"]["peer_availability_agrees"])

    def test_run_persists_full_report_for_actual_v2_availability_disagreement(self):
        case, _ = controlled.load_case()
        process = {
            "plant_horizon_years_input": 5.225442889963153,
            "unplanned_unavailability": dict.fromkeys(("uubop", "uucd", "uudiv", "uufuel", "uufw", "uumag", "uuves"), 0.0),
            "replacement_duration_years": {"blanket": 60 / 365.25},
            "plant_availability_fraction": 0.9685633442013221,
            "capacity_factor": 0.9685633442013221,
        }
        calendar_years = 5.2254429994676395
        full_power_years = 5.059375617917712
        blue = self.blue_metrics(calendar_years, full_power_years)

        def fake_bounded_run(argv, **_kwargs):
            target = Path(argv[3])
            if "process_availability_worker.py" in argv[1]:
                target.write_text(json.dumps(process), encoding="utf-8")
                stdout = ""
            else:
                target.write_text(json.dumps(self.timeline_record(calendar_years, full_power_years)), encoding="utf-8")
                stdout = "Rich console log before JSON\n" + json.dumps(blue) + "\n"
            return {"return_code": 0, "stdout": stdout, "stderr": ""}

        with tempfile.TemporaryDirectory() as temp_dir, patch.object(controlled.peer_compare, "bounded_run", fake_bounded_run):
            output_dir = Path(temp_dir) / "result"
            result = controlled.run("python-process", "python-bluemira", output_dir)
            persisted = json.loads((output_dir / "comparison.json").read_text(encoding="utf-8"))

        self.assertEqual(result["comparison"]["status"], "incomplete; native output checks did not all match")
        self.assertFalse(result["comparison"]["checks"]["peer_availability_agrees"])
        self.assertTrue(result["comparison"]["checks"]["bluemira_timeline_artifact_matches_metrics"])
        self.assertAlmostEqual(persisted["native_outputs"]["bluemira"]["calendar_years"], calendar_years)
        self.assertAlmostEqual(persisted["native_outputs"]["bluemira"]["overall_availability"],
                               full_power_years / calendar_years)
        self.assertIn("peer_run_records", persisted)


if __name__ == "__main__":
    unittest.main()
