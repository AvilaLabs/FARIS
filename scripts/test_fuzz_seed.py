# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
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


if __name__ == "__main__":
    unittest.main()
