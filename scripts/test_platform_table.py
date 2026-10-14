import tempfile
import unittest
from pathlib import Path

import platform_table as pt

WORKFLOW = "name: x\njobs:\n  native:\n    runs-on: macos-14\n  other:\n    runs-on: x\n"


def table(*rows):
    head = "| Platform | Tier | Runner | Evidence | What it shows |\n| --- | --- | --- | --- | --- |\n"
    return f"{pt.START}\n{head}" + "\n".join(rows) + f"\n{pt.END}\n"


class PlatformTableTests(unittest.TestCase):
    def run_check(self, text):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "p.yml").write_text(WORKFLOW)
            return pt.check(text, Path(d))

    def test_real_job_passes(self):
        problems, gaps = self.run_check(table("| A | 1 | `macos-14` | `p.yml:native` | ok |"))
        self.assertEqual((problems, gaps), ([], []))

    def test_missing_job_fails(self):
        problems, _ = self.run_check(table("| A | 1 | `macos-14` | `p.yml:nope` | ok |"))
        self.assertEqual(len(problems), 1)

    def test_missing_workflow_and_label_fail(self):
        problems, _ = self.run_check(table("| A | 1 | `macos-14` | `q.yml:native` | ok |"))
        self.assertIn("does not exist", problems[0])
        problems, _ = self.run_check(table("| A | 1 | `macos-99` | `p.yml:native` | ok |"))
        self.assertIn("macos-99", problems[0])

    def test_none_needs_reason_and_is_listed(self):
        problems, gaps = self.run_check(table("| A | 1 | none | NONE: no runner | ok |"))
        self.assertEqual((problems, gaps), ([], ["A"]))
        problems, _ = self.run_check(table("| A | 1 | none | NONE | ok |"))
        self.assertEqual(len(problems), 1)

    def test_row_naming_no_job_fails(self):
        problems, _ = self.run_check(table("| A | 1 | `macos-14` | CI | ok |"))
        self.assertEqual(len(problems), 1)

    def test_shipped_table_matches_workflows(self):
        problems, _ = pt.check(pt.TABLE.read_text(), pt.WORKFLOWS)
        self.assertEqual(problems, [])


if __name__ == "__main__":
    unittest.main()
