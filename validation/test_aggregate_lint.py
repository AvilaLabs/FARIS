# SPDX-License-Identifier: AGPL-3.0-only
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import aggregate_lint as L  # noqa: E402
from validation import report as R  # noqa: E402
from validation.fixtures import IDENT, clone, manifest, run_record  # noqa: E402


def suite():
    m = manifest()
    return R.build_suite([m], {"fixture": run_record(m, [d["reference"]["value"] * 1.01 for d in m["detectors"]])}, IDENT)


class AggregateLint(unittest.TestCase):
    # Verifies: VAL-035
    def test_the_repository_documents_carry_no_c_e_aggregate(self):
        self.assertEqual(L.run(), [])

    # Verifies: VAL-035
    def test_a_headline_or_cross_class_c_e_number_in_a_document_is_found(self):
        for text in ("The overall C/E was 0.97.", "A combined C/E of 1.02 over all classes.", "We average C/E across all libraries.",
                     "Mean C/E over the benchmarks is 0.99.", "The C/E, pooled across code versions, is 1.01."):
            self.assertEqual(len(L.lint_text("x.md", text)), 1, text)

    # Verifies: VAL-035
    def test_a_negated_or_per_class_statement_is_not_a_finding(self):
        for text in ("There is no overall C/E.", "FARIS never reports a headline C/E across classes.",
                     "C/E is summarised per response class, from all detectors: count, mean, median.",
                     "Bias is the mean of log C/E for one class."):
            self.assertEqual(L.lint_text("x.md", text), [], text)

    # Verifies: VAL-035
    def test_a_distribution_table_outside_a_response_class_section_is_found(self):
        good = R.render_markdown(suite())
        self.assertEqual(L.lint_report_markdown("report.md", good), [])
        bad = good.replace("### Response class", "### Everything", 1)
        self.assertEqual(len(L.lint_report_markdown("report.md", bad)), 1)

    # Verifies: VAL-035
    def test_the_generated_report_suite_is_clean_and_an_aggregate_key_or_mixed_library_is_found(self):
        s = suite()
        self.assertEqual(L.lint_suite(s), [])
        bad = clone(s)
        bad["overall"] = {"ce": 1.0}
        self.assertTrue(any("'overall'" in p for p in L.lint_suite(bad)))
        mixed = clone(s)
        row = next(r for r in mixed["cases"][0]["rows"] if r.get("ce") is not None)
        row["library_sha256"] = "a" * 64
        other = clone(row)
        other["library_sha256"] = "b" * 64
        mixed["cases"][0]["rows"].append(other)
        self.assertTrue(any("mixes library_sha256" in p for p in L.lint_suite(mixed)))
        fewer = clone(s)
        fewer["cases"][0]["rows"] = []
        self.assertTrue(any("stands on 0 rows" in p for p in L.lint_suite(fewer)))

    # Verifies: VAL-035
    def test_extra_files_are_scanned_and_the_command_exits_1_on_a_finding(self):
        with tempfile.TemporaryDirectory() as d:
            page = Path(d) / "page.md"
            page.write_text("Our overall C/E is 1.0.\n", encoding="utf-8")
            self.assertEqual(L.main(["--root", d, str(page)]), 1)
            page.write_text("Per class only.\n", encoding="utf-8")
            self.assertEqual(L.main(["--root", d, str(page)]), 0)


if __name__ == "__main__":
    unittest.main()
