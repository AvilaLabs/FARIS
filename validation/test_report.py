# SPDX-License-Identifier: AGPL-3.0-only
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import report as R  # noqa: E402
from validation.manifest import load_manifest  # noqa: E402
from validation.fixtures import IDENT, clone, manifest, run_record  # noqa: E402


class Report(unittest.TestCase):
    def suite(self, with_run=True):
        m = manifest()
        records = {"fixture": run_record(m, [d["reference"]["value"] * 1.01 for d in m["detectors"]])} if with_run else {}
        return R.build_suite([m], records, IDENT if with_run else None)

    # Verifies: VAL-030
    def test_markdown_carries_the_distribution_and_worst_case(self):
        text = R.render_markdown(self.suite())
        for label in ("| count | mean | median | std dev | 5th | 95th | min | max |", "Worst case:", "Bias (mean log C/E)", "seed 20261009"):
            self.assertIn(label, text)

    # Verifies: VAL-075
    def test_markdown_lists_what_is_not_covered(self):
        self.assertIn("Not covered by this case:", R.render_markdown(self.suite()))

    # Verifies: VAL-056
    def test_coverage_comes_with_its_list(self):
        suite = self.suite(with_run=False)
        self.assertEqual(suite["coverage"]["scored"], 0)
        self.assertEqual(len(suite["coverage"]["unscored"]), 6)
        self.assertTrue(all(u["why"] and u["next_step"] for u in suite["coverage"]["unscored"]))

    def test_report_lint_is_clean_and_catches_missing_next_step(self):
        suite = self.suite()
        self.assertEqual(R.lint_report(suite), [])
        bad = clone(suite)
        bad["cases"][0]["rows"][0].update(verdict="INCONCLUSIVE", why="x", next_step=None)
        self.assertTrue(any("without why and next step" in p for p in R.lint_report(bad)))
        bad = clone(suite)
        bad["cases"][0]["rows"][0]["evidence_class"] = None
        self.assertTrue(any("evidence class" in p for p in R.lint_report(bad)))

    def test_run_record_needs_a_current_identity(self):
        m = manifest()
        with self.assertRaises(Exception):
            R.build_suite([m], {"fixture": run_record(m, [1.0] * 6)}, None)

    # Verifies: VAL-091
    def test_one_command_writes_json_and_markdown_for_the_registered_cases(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(R.main(["report", "--out", d]), 0)
            suite = json.loads((Path(d) / "report.json").read_text(encoding="utf-8"))
            self.assertEqual({c["case_id"] for c in suite["cases"]}, {p.parent.name for p in R.discover()})
            self.assertTrue((Path(d) / "report.md").read_text(encoding="utf-8").startswith("# FARIS validation report"))
            self.assertEqual(suite["coverage"]["scored"], 0)

    # Verifies: VAL-091
    def test_report_carries_every_section_and_records_its_hash(self):
        with tempfile.TemporaryDirectory() as d, tempfile.TemporaryDirectory() as d2:
            record = Path(d) / "record.json"
            record.write_text(json.dumps({"version": "x"}), encoding="utf-8")
            self.assertEqual(R.main(["report", "--out", d, "--release-record", str(record)]), 0)
            self.assertEqual(R.main(["report", "--out", d2]), 0)
            suite = json.loads((Path(d) / "report.json").read_text(encoding="utf-8"))
            for key in ("methods", "matrix", "cases", "qualified_ranges", "known_issues", "last_run"):
                self.assertTrue(suite[key], key)
            self.assertTrue(all("distribution" in cls for c in suite["cases"] for cls in c["classes"].values()))
            text = (Path(d) / "report.md").read_text(encoding="utf-8")
            for heading in ("## Methods", "## V&V matrix", "## Known issues", "## Qualified ranges", "## Last run of each benchmark"):
                self.assertIn(heading, text)
            rec = json.loads(record.read_text(encoding="utf-8"))
            self.assertEqual(rec["version"], "x")
            self.assertEqual(rec["vv_report_sha256"], hashlib.sha256((Path(d) / "report.json").read_bytes()).hexdigest())
            self.assertIn(rec["vv_report_sha256"], (Path(d) / "report.sha256").read_text(encoding="utf-8"))
            # The same inputs give the same bytes, so the recorded hash identifies the report.
            self.assertEqual((Path(d) / "report.json").read_bytes(), (Path(d2) / "report.json").read_bytes())

    # Verifies: VAL-091
    def test_last_run_date_comes_from_the_run_record(self):
        m = manifest()
        record = run_record(m, [1.0] * 6)
        self.assertEqual(R.build_suite([m], {"fixture": record}, IDENT)["last_run"]["fixture"], "run date not recorded in the run record")
        from validation.scoring import seal_run_record
        dated = seal_run_record(dict(record, run_date="2026-10-12"))
        self.assertEqual(R.build_suite([m], {"fixture": dated}, IDENT)["last_run"]["fixture"], "2026-10-12")

    def test_registered_cases_with_a_synthetic_record_score_and_go_stale(self):
        m = load_manifest(R.CASES_DIR / "oktavian-al" / "manifest.json")
        calc = [d["reference"]["value"] for d in m["detectors"]]
        suite = R.build_suite([m], {"oktavian-al": run_record(m, calc, u_mc=1e-6)}, IDENT)
        counts = suite["cases"][0]["verdict_counts"]
        # The normalisation is confirmed; the photon bins and the first neutron bin stay blocked, so only the other 133 neutron bins may be scored.
        self.assertEqual(counts["PASS"] + counts["FAIL"] + counts["INCONCLUSIVE"], 133)
        self.assertEqual(counts["NOT_EVALUATED"], 58)
        self.assertEqual(R.lint_report(suite), [])


if __name__ == "__main__":
    unittest.main()
