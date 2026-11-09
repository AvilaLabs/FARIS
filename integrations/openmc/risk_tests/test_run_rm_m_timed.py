# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Cheap mocked checks for the RM-M stage timing wrapper."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("run_rm_m_timed", HERE / "run_rm_m_timed.py")
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)


class TimedWrapperTests(unittest.TestCase):
    def test_wrapper_preserves_return_value_and_records_boundary(self):
        with tempfile.TemporaryDirectory() as temporary:
            trace = DRIVER.Trace(Path(temporary) / "trace.jsonl")
            sentinel = object()
            wrapped = DRIVER.timed_wrapper(trace, "mock-stage", lambda value: value)
            try:
                self.assertIs(wrapped(sentinel), sentinel)
            finally:
                trace.close()
            rows = [json.loads(line) for line in Path(temporary, "trace.jsonl").read_text().splitlines()]
            self.assertEqual([row["event"] for row in rows], ["start", "end"])
            self.assertEqual(rows[-1]["outcome"], "return")

    def test_wrapper_preserves_original_exception_object(self):
        with tempfile.TemporaryDirectory() as temporary:
            trace = DRIVER.Trace(Path(temporary) / "trace.jsonl")
            failure = RuntimeError("expected failure")

            def raise_same():
                raise failure

            wrapped = DRIVER.timed_wrapper(trace, "mock-stage", raise_same)
            try:
                with self.assertRaises(RuntimeError) as raised:
                    wrapped()
                self.assertIs(raised.exception, failure)
            finally:
                trace.close()
            rows = [json.loads(line) for line in Path(temporary, "trace.jsonl").read_text().splitlines()]
            self.assertEqual([row["event"] for row in rows], ["start", "end"])
            self.assertEqual(rows[-1]["outcome"], "exception")
            self.assertEqual(rows[-1]["exception"], "expected failure")


if __name__ == "__main__":
    unittest.main()
