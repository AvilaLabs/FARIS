# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import json
from pathlib import Path
import tempfile
import unittest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fuzz_aggregate


class FuzzAggregateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fuzz = self.root / "fuzz"
        (self.fuzz / "fuzz_targets").mkdir(parents=True)
        for name in ("a", "b"):
            (self.fuzz / "fuzz_targets" / f"{name}.rs").write_text("")
        self.input = self.root / "jobs"
        self.input.mkdir()
        self.commit = "a" * 40
        for name in ("a", "b"):
            (self.input / f"{name}.json").write_text(json.dumps({
                "schema_version": "faris-fuzz-evidence/v0.2",
                "commit": self.commit, "driver": "libfuzzer", "dirty": False,
                "toolchain": "nightly", "seconds_per_target_requested": 3600,
                "finished": "2026-11-10T00:00:00Z",
                "checks": [{"name": name, "verdict": "PASS", "seconds": 3600,
                            "cpu_seconds": 3600, "crashes": 0, "exit_status": 0}],
            }))

    def tearDown(self):
        self.tmp.cleanup()

    def test_aggregates_complete_target_set(self):
        output = self.root / "evidence.json"
        result = fuzz_aggregate.aggregate(self.root, self.input, output, self.commit)
        self.assertEqual([c["name"] for c in result["checks"]], ["a", "b"])
        self.assertEqual(json.loads(output.read_text())["commit"], self.commit)

    def test_rejects_incomplete_or_mismatched_records(self):
        (self.input / "b.json").unlink()
        with self.assertRaisesRegex(ValueError, "evidence files differ"):
            fuzz_aggregate.aggregate(self.root, self.input, self.root / "x.json", self.commit)
        (self.input / "b.json").write_text(json.dumps({
            "schema_version": "faris-fuzz-evidence/v0.2", "commit": "b" * 40,
        }))
        with self.assertRaisesRegex(ValueError, "b: evidence commit"):
            fuzz_aggregate.aggregate(self.root, self.input, self.root / "x.json", self.commit)

    def test_rejects_extra_files_and_noninteger_thresholds(self):
        extra = self.input / "unexpected.json"
        extra.write_text("{}")
        with self.assertRaisesRegex(ValueError, "evidence files differ"):
            fuzz_aggregate.aggregate(self.root, self.input, self.root / "x.json", self.commit)
        extra.unlink()
        record = json.loads((self.input / "a.json").read_text())
        record["checks"][0]["cpu_seconds"] = 3600.5
        (self.input / "a.json").write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, "did not meet"):
            fuzz_aggregate.aggregate(self.root, self.input, self.root / "x.json", self.commit)


if __name__ == "__main__":
    unittest.main()
