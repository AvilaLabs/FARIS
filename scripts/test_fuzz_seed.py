# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import json
import tempfile
import unittest
from pathlib import Path

import fuzz_seed

ROOT = Path(__file__).resolve().parent.parent


class FuzzSeed(unittest.TestCase):
    # Verifies: QA-033
    # Verifies: SEC-020
    def test_every_fuzz_target_gets_seeds_from_the_repository(self):
        targets = sorted(p.stem for p in (ROOT / "fuzz" / "fuzz_targets").glob("*.rs"))
        self.assertTrue(targets)
        with tempfile.TemporaryDirectory() as out:
            counts = fuzz_seed.build(Path(out))
            for target in targets:
                self.assertGreater(counts.get(target, 0), 0, f"{target} has no seeds")
                self.assertTrue(any((Path(out) / target).iterdir()))
            self.assertEqual(set(counts) - set(targets), set(), "seeds for a target that does not exist")

    def test_shutdown_dose_target_gets_both_documented_parser_formats(self):
        with tempfile.TemporaryDirectory() as out:
            counts = fuzz_seed.build(Path(out))
            self.assertGreaterEqual(counts.get("shutdown_dose_records", 0), 2)
            target = Path(out) / "shutdown_dose_records"
            schemas = {
                fuzz_seed.schema_of(seed.read_bytes())
                for seed in target.iterdir()
            }
            self.assertIn("faris-shutdown-dose-compare-request/v0.1", schemas)
            self.assertIn("faris-shutdown-dose-estimate/v0.1", schemas)

    # Verifies: QA-033
    def test_helper_seed_corpus_reaches_heating_worker_and_preflight_schemas(self):
        with tempfile.TemporaryDirectory() as out:
            fuzz_seed.build(Path(out))
            seeds = [json.loads(path.read_text()) for path in (Path(out) / "helper_outputs").iterdir()]
        by_schema = {seed.get("schema"): seed for seed in seeds if isinstance(seed, dict)}
        self.assertIn("faris-design-heating/v1", by_schema)
        self.assertIn("faris-design-heating-preflight/v1", by_schema)
        worker = by_schema["faris-design-heating/v1"]
        self.assertEqual(worker["status"], "CALCULATED")
        self.assertEqual(worker["runtime_identity"]["openmc_version"], "0.16.0")
        self.assertEqual(worker["execution_identity"]["run_h5m"]["path"], "/run/geometry/model-heating.h5m")
        self.assertEqual(worker["solids"][0]["peak"]["volume_estimator"]["rays"], 1000)
        self.assertTrue(worker["solids"][0]["voxel_scores"][0]["closure_residual"]["within_3se"])
        preflight = by_schema["faris-design-heating-preflight/v1"]
        self.assertEqual(preflight["status"], "READY")
        self.assertEqual(preflight["runtime_identity"], worker["runtime_identity"])


if __name__ == "__main__":
    unittest.main()
