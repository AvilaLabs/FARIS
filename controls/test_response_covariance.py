import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


CHECK = load_module("check_response_covariance", Path(__file__).with_name("check_response_covariance.py"))
WORKER = load_module("reactor_transport", ROOT / "integrations/openmc/reactor_transport.py")

VALUES = {
    "flux": [1.0, 2.0, 4.0, 3.0, 5.0],
    "rate": [0.5, 0.75, 2.5, 1.0, 1.25],
    "heat": [-2.0, 1.0, 0.5, 3.5, -1.0],
}
UNITS = {"flux": "cm_per_source", "rate": "events_per_source", "heat": "ev_per_source"}
REQUEST = {
    "fusion_power_mw": 500.0,
    "source": {"energy_per_reaction_ev": 17.6e6, "neutrons_per_reaction": 1.0},
}


def mean(values):
    return sum(values) / len(values)


def build_run(directory, normalize=True):
    """Write a synthetic run directory using the worker's own helpers."""
    ids = list(VALUES)
    matrix = WORKER.batch_mean_covariance([VALUES[i] for i in ids])
    solver = Path(directory) / "solver"
    solver.mkdir(parents=True)
    batch_doc = {"method": WORKER.COVARIANCE_METHOD, "n_batches": 5, "response_ids": ids, "values": VALUES}
    batch_path = solver / "transport-batch-values.json"
    batch_path.write_text(json.dumps(batch_doc))
    tallies = [{"response_id": i, "estimator": "tracklength", "unit": UNITS[i], "mean": mean(VALUES[i]),
                "standard_error": matrix[k][k] ** 0.5} for k, i in enumerate(ids)]
    raw = [v for row in matrix for v in row]
    artifact = {"tallies": tallies, "response_covariance": {
        "method": WORKER.COVARIANCE_METHOD, "batches": 5, "response_ids": ids, "raw_per_source": raw,
        "batch_values_file": batch_path.name, "batch_values_sha256": CHECK.digest(batch_path)}}
    (solver / "transport-artifact.json").write_text(json.dumps(artifact))
    (Path(directory) / "input.json").write_text(json.dumps({"request": REQUEST}))
    if normalize:
        rate = 500.0e6 / (17.6e6 * 1.602176634e-19)
        factors = {"cm_per_source": 0.01, "events_per_source": 1.0, "ev_per_source": 1.602176634e-19}
        scales = [rate * factors[UNITS[i]] for i in ids]
        integrated = [matrix[a][b] * scales[a] * scales[b] for a in range(3) for b in range(3)]
        results = [{"response_id": i, "integrated_standard_error": (matrix[k][k] ** 0.5) * scales[k]}
                   for k, i in enumerate(ids)]
        record = {"normalized": {"results": results, "response_covariance": {
            "response_ids": ids, "integrated": integrated}}}
        (Path(directory) / "run.json").write_text(json.dumps(record))
    return solver


class WorkerHelperTests(unittest.TestCase):
    def test_per_batch_values_difference_cumulative_sums(self):
        self.assertEqual(WORKER.per_batch_values([1.0, 3.0, 7.0]), [1.0, 2.0, 4.0])

    # Verifies: UNC-011
    def test_covariance_matches_hand_calculation(self):
        matrix = WORKER.batch_mean_covariance([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0]])
        # Sample variances 1 and 4, covariance 2, each divided by n = 3.
        self.assertAlmostEqual(matrix[0][0], 1.0 / 3.0, places=15)
        self.assertAlmostEqual(matrix[1][1], 4.0 / 3.0, places=15)
        self.assertAlmostEqual(matrix[0][1], 2.0 / 3.0, places=15)
        self.assertEqual(matrix[0][1], matrix[1][0])

    def test_verification_rejects_inconsistent_mean_or_error(self):
        values = [1.0, 2.0, 4.0, 3.0]
        se = (sum((v - 2.5) ** 2 for v in values) / 3 / 4) ** 0.5
        WORKER.verify_batch_values("r", values, 2.5, se)
        with self.assertRaisesRegex(ValueError, "mean of batch values"):
            WORKER.verify_batch_values("r", values, 2.6, se)
        with self.assertRaisesRegex(ValueError, "batch standard error"):
            WORKER.verify_batch_values("r", values, 2.5, se * 1.01)
        with self.assertRaisesRegex(ValueError, "at least 2 batches"):
            WORKER.verify_batch_values("r", [1.0], 1.0, 0.0)

    def test_zero_mean_checks_only_the_mean(self):
        WORKER.verify_batch_values("r", [1.0, -1.0], 0.0, 0.0)
        with self.assertRaisesRegex(ValueError, "mean of batch values"):
            WORKER.verify_batch_values("r", [1.0, -1.0], 0.5, 0.0)

    def test_one_batch_is_rejected_by_covariance(self):
        with self.assertRaises(ValueError):
            WORKER.batch_mean_covariance([[1.0]])


class ControlTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.run_dir = Path(self.temp.name) / "run"
        self.solver = build_run(self.run_dir)

    def rewrite(self, name, change):
        path = self.solver / name
        data = json.loads(path.read_text())
        change(data)
        path.write_text(json.dumps(data))

    def test_consistent_run_passes_with_correlations(self):
        report = CHECK.check(self.run_dir)
        self.assertLess(report["max_scaled_raw_difference"], 1e-12)
        self.assertTrue(report["integrated_checked"])
        self.assertLess(report["max_scaled_integrated_difference"], 1e-12)
        corr = report["correlation"]
        for i in range(3):
            self.assertAlmostEqual(corr[i][i], 1.0, places=12)
        self.assertAlmostEqual(corr[0][1], corr[1][0], places=12)
        self.assertLessEqual(abs(corr[0][1]), 1.0 + 1e-12)

    def test_perturbed_raw_entry_fails(self):
        self.rewrite("transport-artifact.json",
                     lambda d: d["response_covariance"]["raw_per_source"].__setitem__(1, 1.0e3))
        with self.assertRaisesRegex(ValueError, "differs"):
            CHECK.check(self.run_dir)

    def test_changed_batch_values_fail_digest(self):
        self.rewrite("transport-batch-values.json", lambda d: d["values"]["flux"].__setitem__(0, 9.0))
        with self.assertRaisesRegex(ValueError, "sha256"):
            CHECK.check(self.run_dir)

    def test_batch_values_must_match_tally_standard_error(self):
        def change(data):
            data["tallies"][0]["standard_error"] *= 1.5
        self.rewrite("transport-artifact.json", change)
        with self.assertRaisesRegex(ValueError, "standard error"):
            CHECK.check(self.run_dir)

    def test_wrong_integrated_scaling_fails(self):
        path = self.run_dir / "run.json"
        record = json.loads(path.read_text())
        record["normalized"]["response_covariance"]["integrated"][1] *= 1.001
        path.write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, "differs"):
            CHECK.check(self.run_dir)

    # Verifies: UNC-011
    def test_missing_covariance_is_an_error(self):
        artifact = json.loads((self.solver / "transport-artifact.json").read_text())
        with self.assertRaises(ValueError):
            CHECK.check_raw(dict(copy.deepcopy(artifact), response_covariance=None), {}, "")


if __name__ == "__main__":
    unittest.main()
