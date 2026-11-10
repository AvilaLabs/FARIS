# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import importlib.util
import json
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


@unittest.skipIf(importlib.util.find_spec("numpy") is None, "NumPy is not installed")
class NativeTimelineJSONEncoderTests(unittest.TestCase):
    def test_numpy_scalar_and_array_values_are_preserved_as_json_numbers(self):
        import numpy as np

        payload = {"blanket_change_index": np.int64(612),
                   "calendar_time": np.float64(5.25),
                   "fusion_time": np.array([0.0, 1.5, 2.25])}
        decoded = json.loads(json.dumps(payload, cls=worker.NativeTimelineJSONEncoder,
                                        allow_nan=False))
        self.assertEqual(decoded["blanket_change_index"], 612)
        self.assertEqual(decoded["calendar_time"], 5.25)
        self.assertEqual(decoded["fusion_time"], [0.0, 1.5, 2.25])
        self.assertIs(type(decoded["blanket_change_index"]), int)

    def test_unsupported_values_still_fail_closed(self):
        with self.assertRaises(TypeError):
            json.dumps({"unsupported": object()}, cls=worker.NativeTimelineJSONEncoder)


if __name__ == "__main__":
    unittest.main()
