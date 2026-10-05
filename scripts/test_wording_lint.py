import copy
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wording_lint as lint

TERMS = lint.load_terms()


def check(text: str, terms: dict = TERMS) -> list[str]:
    """Rules fired by one sentence of text."""
    return [f.rule for f in lint.lint_text("t", [(text, 1)], terms)]


class ClaimTests(unittest.TestCase):
    # Verifies: LEG-041
    def test_claims_are_flagged(self):
        for text in ["The design is licensed.", "FARIS is certified for use.", "A qualified result.",
                     "This is safety-approved.", "Approved for construction.", "A validated design.",
                     "Qualification achieved.", "It carries regulator approval and safety approval."]:
            self.assertIn("LEG-041", check(text), text)

    # Verifies: LEG-041
    def test_negated_and_limited_uses_are_allowed(self):
        for text in ["It is not a licensing basis.", "Nothing here is a qualification claim.",
                     "No service-life claim is qualified.", "This never gives safety approval.",
                     "Used without certification.", "Qualification is NOT_EVALUATED.",
                     "Licensing is not evaluated here."]:
            self.assertEqual(check(text), [], text)

    # Verifies: LEG-041
    def test_negation_must_be_in_the_same_sentence_and_close(self):
        self.assertIn("LEG-041", check("It is not complete. The design is licensed."))
        far = "not one two three four five six seven eight nine ten licensed"
        self.assertIn("LEG-041", check(far))

    # Verifies: LEG-041
    def test_allow_list_is_exact_sentence_and_must_be_used(self):
        terms = copy.deepcopy(TERMS)
        terms["allow_list"] = [{"rule": "LEG-041", "text": "The code is licensed under X.", "reason": "licence"}]
        self.assertEqual(check("The code is licensed under X.", terms), [])
        self.assertIn("LEG-041", check("The code is licensed under Y.", terms))
        with tempfile.TemporaryDirectory() as tmp:
            findings = lint.run(Path(tmp), terms)
        self.assertEqual([f.rule for f in findings], ["ALLOW-LIST"])


class TwinTests(unittest.TestCase):
    # Verifies: LEG-042
    def test_twin_is_flagged(self):
        for text in ["A digital twin of the plant.", "Digital-twin ready.", "Twin models."]:
            self.assertIn("LEG-042", check(text), text)

    # Verifies: LEG-042
    def test_twin_only_in_allow_listed_sentence(self):
        self.assertEqual(check("It is not a digital twin."), [])  # listed in the shipped terms
        self.assertIn("LEG-042", check("It is not a digital twin of anything."))
        self.assertEqual(check("Entwined values and twinkling lights."), [])


class VerdictTests(unittest.TestCase):
    # Verifies: LEG-043
    def test_variant_spellings_are_flagged(self):
        for text in ["Result: NOT EVALUATED.", "Result: Not Evaluated.", "A not-evaluated arrangement.",
                     "Status NOT-EVALUATED", "The check PASSED.", "The check FAILED.", "Passed", "In-conclusive result",
                     "Verdict: Non-conclusive"]:
            self.assertIn("LEG-043", check(text), text)

    # Verifies: LEG-043
    def test_canonical_words_and_ordinary_english_are_clean(self):
        for text in ["Verdict NOT_EVALUATED.", "The arrangement is not evaluated.", "Not evaluated; no claim.",
                     "INCONCLUSIVE within noise.", "The result is inconclusive.", "Values were passed to the worker.",
                     "Evaluated alone is fine.", "PASS and FAIL are verdicts."]:
            self.assertEqual(check(text), [], text)

    # Verifies: LEG-043
    def test_title_case_only_allowed_at_sentence_start(self):
        self.assertEqual(check("Inconclusive results are shown."), [])
        self.assertIn("LEG-043", check("The verdict is Inconclusive."))


class CodenameTests(unittest.TestCase):
    # Verifies: LEG-044
    def test_codenames_are_flagged(self):
        for text in ["Built with OMNIGEN.", "See the North Star plan.", "north-star goal", "Via Jev tools.",
                     "The Aftermatter run.", "Astra roadmap", "McFly order", "ORACLE data"]:
            self.assertIn("LEG-044", check(text), text)

    # Verifies: LEG-044
    def test_codenames_are_whole_word_and_case_sensitive(self):
        for text in ["The oracle said nothing.", "Jevons paradox.", "Astrapia", "A starlit north."]:
            self.assertEqual(check(text), [], text)

    # Verifies: LEG-044
    def test_file_names_are_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            scripts = Path(tmp) / "scripts"
            scripts.mkdir()
            (scripts / "package_omnigen.py").write_text("")
            (scripts / "fine_name.py").write_text("")
            findings = lint.lint_names(Path(tmp), TERMS)
        self.assertEqual([f.path for f in findings], ["scripts/package_omnigen.py"])


class ExtractionTests(unittest.TestCase):
    def test_rust_strings_skip_comments_tests_and_find_raw_strings(self):
        source = '''
// licensed in a comment
/// "certified" in a doc comment
fn a() { let s = "string one"; let r = r#"raw "quoted" text"#; let c = '"'; let l: &'static str = "two"; }
#[cfg(test)]
mod tests { fn t() { let s = "licensed in a test"; } }
fn b() { "after" }
'''
        found = [text for text, _ in lint.rust_strings(source)]
        self.assertEqual(found, ["string one", 'raw "quoted" text', "two", "after"])

    def test_python_strings_skip_docstrings_and_comments(self):
        source = '"""docstring licensed"""\n# comment licensed\nx = "kept text"\ndef f():\n    """doc licensed"""\n    return "also kept"\n'
        found = [text for text, _ in lint.python_strings(source)]
        self.assertEqual([t.strip("\"'") for t in found], ["kept text", "also kept"])

    def test_markdown_paragraph_negation_spans_wrapped_lines(self):
        units = lint.markdown_units("It is not\na licensing basis.\n\n- item licensed\n")
        findings = lint.lint_text("m.md", units, TERMS)
        self.assertEqual([(f.line, f.rule) for f in findings], [(4, "LEG-041")])

    def test_repository_is_clean(self):
        self.assertEqual([f.render() for f in lint.run()], [])


if __name__ == "__main__":
    unittest.main()
