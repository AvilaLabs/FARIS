import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "summarize_transport_campaign", Path(__file__).with_name("summarize_transport_campaign.py"))
SUMMARY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SUMMARY)


def run_record(status="SUCCEEDED", artifact="a" * 64):
    return {
        "scenario_sha256": "s" * 64, "variant_id": "reference", "physics_sha256": "p" * 64,
        "raw_artifact_sha256": artifact, "sampling": {"batches": 100, "particles_per_batch": 100000,
                                                      "seed": 1, "threads": 7},
        "execution": {"execution_status": status, "exit_code": 0 if status == "SUCCEEDED" else 1,
                      "elapsed_seconds": 10.0},
        "scientific_qualification": "NOT_EVALUATED",
        "normalized": {
            "scenario_id": "demo", "histories": 10_000_000,
            "solver": {"name": "OpenMC", "version": "0.15.3"},
            "results": [
                {"response_id": "blanket-tritium", "domain": {"kind": "component"},
                 "mean": 2.0, "standard_error": 0.01, "unit": "u"},
                {"response_id": "magnets-port-sector-fast-flux", "domain": {"kind": "component_region"},
                 "mean": 4.0, "standard_error": 2.0, "unit": "u"},
                {"response_id": "blanket-tritium", "domain": {"kind": "mesh"},
                 "mean": 9.0, "standard_error": 9.0, "unit": "u"},
            ],
        },
    }


class SummaryTests(unittest.TestCase):
    def call(self, root: Path, *argv: str) -> int:
        with mock.patch.object(sys, "argv", ["summarize", *argv]):
            return SUMMARY.main()

    def test_headline_responses_skip_mesh_bins_and_ensembles_bind_to_the_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "run.json").write_text(json.dumps(run_record()))
            (root / "ens.json").write_text(json.dumps({
                "transport_artifact_sha256": "a" * 64, "samples_requested": 200,
                "samples_accepted": 200, "rejections": 5, "status": {"status": "not_evaluated"}}))
            self.call(root, "--run", f"r={root / 'run.json'}", "--ensemble", f"r={root / 'ens.json'}",
                      "--output", str(root / "out.json"))
            report = json.loads((root / "out.json").read_text())
            run = report["runs"][0]
            self.assertEqual(run["headline_responses"]["blanket-tritium"]["relative_standard_error"], 0.005)
            self.assertEqual(run["headline_responses"]["magnets-port-sector-fast-flux"]["relative_standard_error"], 0.5)
            self.assertAlmostEqual(run["history_ensemble"]["rejected_fraction_of_draws"], 5 / 205)
            self.assertIn("not a licensing, safety or design basis", report["scope"])

    def test_failed_runs_and_foreign_ensembles_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "bad.json").write_text(json.dumps(run_record(status="FAILED")))
            with self.assertRaisesRegex(SystemExit, "did not succeed"):
                self.call(root, "--run", f"r={root / 'bad.json'}", "--output", str(root / "o1.json"))
            (root / "run.json").write_text(json.dumps(run_record()))
            (root / "ens.json").write_text(json.dumps({"transport_artifact_sha256": "b" * 64}))
            with self.assertRaisesRegex(SystemExit, "another transport artifact"):
                self.call(root, "--run", f"r={root / 'run.json'}", "--ensemble", f"r={root / 'ens.json'}",
                          "--output", str(root / "o2.json"))
            self.assertFalse((root / "o1.json").exists() or (root / "o2.json").exists())


if __name__ == "__main__":
    unittest.main()
