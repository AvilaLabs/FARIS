# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest

MODULE = Path(__file__).with_name("bluemira_worker.py")
SPEC = importlib.util.spec_from_file_location("faris_bluemira_worker_metrics", MODULE)
assert SPEC and SPEC.loader
worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(worker)


class BluemiraWorkerMetricTests(unittest.TestCase):
    def test_reports_actual_native_timeline_not_continuous_budget(self):
        lifecycle = SimpleNamespace(
            T=SimpleNamespace(plant_life=5.22544, ft=[0.0, 5.0598]),
            params=SimpleNamespace(A_global=1.0),
            total_planned_maintenance=5_184_000.0,
            total_ramptime=0.0,
            t_unplanned_m=0.0,
            n_blk_replace=1,
            n_div_replace=4,
            fpy=5.061190855587916,
            phase_names=["Phase P1.1", "Phase M1.1"],
        )
        metrics = worker.native_timeline_metrics(lifecycle, "2.14.0", 20261109)
        self.assertEqual(metrics["calendar_years"], 5.22544)
        self.assertEqual(metrics["full_power_years"], 5.0598)
        self.assertNotEqual(metrics["full_power_years"], metrics["continuous_damage_budget_fpy_input"])
        self.assertAlmostEqual(metrics["overall_availability"], 5.0598 / 5.22544)
        self.assertIn("Timeline.ft[-1]", metrics["timeline_metrics_basis"])


if __name__ == "__main__":
    unittest.main()
