# SPDX-License-Identifier: AGPL-3.0-only
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import usability_report as ur  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
TEMPLATE = (REPO / ur.TEMPLATE).read_text(encoding="utf-8")
HASH = "a" * 64


def filled() -> str:
    """The template with its placeholders replaced, as a finished report would be."""
    text = TEMPLATE
    text = text.replace("[64 hex digits]", HASH)
    return re.sub(r"\[[^\]\n]{2,}\](?!\()", "x", text)


class UsabilityReportTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / ur.REPORTS).mkdir(parents=True)
        (self.root / ur.TEMPLATE).write_text(TEMPLATE, encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    # Verifies: UX-009
    def test_the_repository_template_and_reports_pass(self):
        self.assertEqual(ur.check(REPO), [])

    # Verifies: UX-009
    def test_the_template_has_the_six_common_industry_format_headings_in_order(self):
        self.assertEqual([h for level, h in ur.headings(TEMPLATE) if level == 2], ur.TOP)
        self.assertEqual(ur.structure_problems(TEMPLATE), [])

    # Verifies: UX-009
    def test_a_filled_report_passes_and_the_unfilled_template_does_not_as_a_report(self):
        (self.root / ur.REPORTS / "r.md").write_text(filled(), encoding="utf-8")
        self.assertEqual(ur.check(self.root), [])
        self.assertTrue(ur.report_problems(TEMPLATE))

    # Verifies: UX-009
    def test_a_missing_reordered_or_unhashed_report_fails(self):
        text = filled()
        for broken, needle in ((text.replace("## Method", "## Methods"), "second-level headings"),
                               (text.replace("### Tasks\n", ""), "'Tasks'"),
                               (text.replace(HASH, "abc", 1), "Protocol hash"),
                               (text.replace("Executive summary", "Summary"), "second-level headings")):
            (self.root / ur.REPORTS / "r.md").write_text(broken, encoding="utf-8")
            problems = ur.check(self.root)
            self.assertTrue(any(needle in p for p in problems), (needle, problems))
        swapped = text.replace("## Results", "## TMP").replace("## Method", "## Results").replace("## TMP", "## Method")
        (self.root / ur.REPORTS / "r.md").write_text(swapped, encoding="utf-8")
        self.assertTrue(ur.check(self.root))


if __name__ == "__main__":
    unittest.main()
