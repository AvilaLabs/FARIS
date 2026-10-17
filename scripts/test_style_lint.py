# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the prose style lint."""
import json
import unittest

import style_lint as lint

TERMS = json.loads(lint.TERMS.read_text(encoding="utf-8"))


def findings(text, terms=TERMS):
    return lint.lint_text("x.md", text, terms, set())


class StyleLintTests(unittest.TestCase):
    # Verifies: DOC-054
    def test_american_spellings_in_prose_are_flagged(self):
        found = findings("The colour is right.\nThe color is wrong, and so is the Normalization.\n")
        self.assertEqual(len(found), 2)
        self.assertIn("x.md:2: STYLE: color -> use colour", found[0])
        self.assertIn("Normalization", found[1])

    # Verifies: DOC-054
    def test_code_links_and_urls_are_not_prose(self):
        text = "Use `normalization` and [artefact](https://example.org/artifact/color).\n\n```\ncolor = 1\n```\n"
        self.assertEqual(findings(text), [])

    # Verifies: DOC-054
    def test_terminology_variants_are_flagged_and_the_allow_list_is_exact(self):
        self.assertEqual(len(findings("Send an e-mail.\n")), 1)
        terms = dict(TERMS, allow_list=[{"sentence": "A quotation says color."}, {"sentence": "Never used."}])
        used = set()
        self.assertEqual(lint.lint_text("x.md", "A quotation says color. The color is wrong.\n", terms, used).__len__(), 1)
        self.assertEqual(used, {"A quotation says color."})

    # Verifies: DOC-054
    def test_the_repository_prose_has_zero_findings(self):
        self.assertEqual(lint.run(), [])


if __name__ == "__main__":
    unittest.main()
