# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Mocked artifact validation and descriptive summary tests for R3 campaigns."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import r3_campaign_summary as summary


def write_json(path: Path, value: dict) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")
    return hashlib.sha256(path.read_bytes()).hexdigest()


class CampaignSummaryTests(unittest.TestCase):
    def fixture(self, root: Path):
        root.mkdir(parents=True, exist_ok=True)
        reps = []
        for rep in (1, 2):
            offset = 70000 + (rep - 1) * 10
            reps.append({"replicate": rep, "d1s_seed": offset,
                         "r2s": [{"activation_cell_cm": cell, "neutron": offset + 1 + i * 2,
                                  "photon": offset + 2 + i * 2, "cli_seed": offset + 1 + i * 2}
                                 for i, cell in enumerate((100, 50, 25))]})
        plan = {"schema": "faris-r3-whole-workflow-campaign/v1", "replicate_count": 2, "replicates": reps}
        plan_path = root / "campaign-plan.json"
        plan_hash = write_json(plan_path, plan)
        tasks = []
        for rep in reps:
            n = rep["replicate"]
            dseed = rep["d1s_seed"]
            dpath = root / f"rep-{n:02d}/d1s/d1s_record.json"
            dhash = write_json(dpath, {"kind": "r3-d1s", "seed": dseed})
            tasks.append({"kind": "d1s", "replicate": n, "status": "COMPLETED", "returncode": 0,
                          "record_path": dpath.relative_to(root).as_posix(), "record_sha256": dhash})
            for grid in rep["r2s"]:
                cell = grid["activation_cell_cm"]
                rpath = root / f"rep-{n:02d}/r2s-{cell}cm/r2s_record.json"
                rhash = write_json(rpath, {"kind": "r3-r2s", "settings": {"activation_cell_cm": cell,
                                    "seeds": [grid["neutron"], grid["photon"]]}})
                tasks.append({"kind": "r2s", "replicate": n, "activation_cell_cm": cell,
                              "status": "COMPLETED", "returncode": 0,
                              "record_path": rpath.relative_to(root).as_posix(), "record_sha256": rhash})
                cpath = root / f"rep-{n:02d}/comparison-{cell}cm.json"
                ratio = 1.0 + 0.01 * n + cell / 10000
                cooling = {label: {"ratio_d1s_over_r2s": ratio, "central_ratio_in_band": True,
                                   "ratio_uncertainty": "unavailable: D1S/R2S covariance is unknown"}
                           for label in summary.COOLING_LABELS}
                chash = write_json(cpath, {"schema": "faris-r3/v1", "cooling_times": cooling,
                                           "runs": {"d1s": {"seed": dseed},
                                                    "r2s": {"settings": {"activation_cell_cm": cell,
                                                                          "seeds": [grid["neutron"], grid["photon"]]}}},
                                           "verdict": {"ratio_band": "INCONCLUSIVE: covariance unknown"}})
                tasks.append({"kind": "compare", "replicate": n, "activation_cell_cm": cell,
                              "status": "COMPLETED", "returncode": 0,
                              "comparison_path": cpath.relative_to(root).as_posix(), "comparison_sha256": chash})
        write_json(root / "campaign-state.json", {"schema": "faris-r3-whole-workflow-campaign-state/v1",
                    "campaign_plan_sha256": plan_hash, "status": "ALL_COMMANDS_EXITED_ZERO", "tasks": tasks})

    def test_summarizes_replicates_separately_per_grid_and_keeps_covariance_inconclusive(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "campaign"
            self.fixture(root)
            result = summary.summarize(root)
        self.assertEqual(result["replicate_count"], 2)
        self.assertEqual(len(result["points"]), 2 * 3 * 3)
        self.assertEqual(summary.COOLING_LABELS, ("1 d", "7 d", "30 d"))
        group = next(g for g in result["cross_replicate_by_grid"]
                     if g["activation_cell_cm"] == 50 and g["cooling_time"] == "1 d")
        self.assertEqual(group["available_replicates"], 2)
        self.assertEqual(len(group["ratio_values_by_replicate"]), 2)
        self.assertEqual([v["replicate"] for v in group["ratio_values_by_replicate"]], [1, 2])
        point = next(p for p in result["points"] if p["replicate"] == 1 and p["activation_cell_cm"] == 50)
        self.assertEqual((point["d1s_seed"], point["r2s_neutron_seed"], point["r2s_photon_seed"]), (70000, 70003, 70004))
        self.assertIn("unknown", point["ratio_uncertainty"])
        self.assertTrue(result["scientific_verdict"].startswith("INCONCLUSIVE"))
        cross = next(x for x in result["cross_grid_by_replicate"]
                     if x["replicate"] == 1 and x["cooling_time"] == "1 d")
        self.assertEqual([x["activation_cell_cm"] for x in cross["ordered_grid_values"]], [100, 50, 25])
        self.assertAlmostEqual(
            cross["adjacent_grid_differences"][0]["difference_fine_minus_coarse"], -0.005
        )

    def test_missing_failed_task_is_reported_without_fabricating_result(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "campaign"
            self.fixture(root)
            state_path = root / "campaign-state.json"
            state = json.loads(state_path.read_text())
            state["status"] = "STOPPED_ON_TASK_LIMIT_OR_FAILURE"
            target = next(i for i, row in enumerate(state["tasks"])
                          if row["kind"] == "compare" and row["replicate"] == 1 and row["activation_cell_cm"] == 25)
            state["tasks"][target]["status"] = "FAILED"
            state["tasks"][target]["returncode"] = 1
            write_json(state_path, state)
            # Its artifact is present but a failed source task cannot contribute a value.
            result = summary.summarize(root)
        missing = next(p for p in result["points"] if p["replicate"] == 1 and
                       p["activation_cell_cm"] == 25 and p["cooling_time"] == "1 d")
        self.assertEqual(missing["status"], "MISSING_OR_FAILED")
        self.assertIsNone(missing.get("ratio_d1s_over_r2s"))

    def test_rejects_mismatched_record_seed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "campaign"
            self.fixture(root)
            state_path = root / "campaign-state.json"
            state = json.loads(state_path.read_text())
            row = next(x for x in state["tasks"] if x["kind"] == "d1s" and x["replicate"] == 1)
            recpath = root / row["record_path"]
            row["record_sha256"] = write_json(recpath, {"kind": "r3-d1s", "seed": 999})
            write_json(state_path, state)
            with self.assertRaisesRegex(ValueError, "seed mismatch"):
                summary.summarize(root)

    def test_completed_status_rejects_missing_task_row(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "campaign"
            self.fixture(root)
            path = root / "campaign-state.json"
            state = json.loads(path.read_text())
            state["tasks"].pop()
            write_json(path, state)
            with self.assertRaisesRegex(ValueError, "missing planned"):
                summary.summarize(root)

    def test_missing_grid_comparison_is_kept_in_cross_grid_view(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "campaign"
            self.fixture(root)
            path = root / "campaign-state.json"
            state = json.loads(path.read_text())
            state["status"] = "STOPPED_ON_TASK_LIMIT_OR_FAILURE"
            state["tasks"] = [row for row in state["tasks"] if not (
                row["kind"] == "compare" and row["replicate"] == 1 and row["activation_cell_cm"] == 50)]
            write_json(path, state)
            result = summary.summarize(root)
        cross = next(x for x in result["cross_grid_by_replicate"]
                     if x["replicate"] == 1 and x["cooling_time"] == "1 d")
        self.assertEqual([x["status"] for x in cross["ordered_grid_values"]],
                         ["AVAILABLE", "MISSING_OR_FAILED", "AVAILABLE"])
        self.assertIsNone(cross["adjacent_grid_differences"][0]["difference_fine_minus_coarse"])
        self.assertIsNone(cross["adjacent_grid_differences"][1]["difference_fine_minus_coarse"])


if __name__ == "__main__":
    unittest.main()
