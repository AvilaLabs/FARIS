# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the handbook licence lint."""
import json
import tempfile
import unittest
from pathlib import Path

import check_doc_licence as lint

GOOD = {"origin": "faris-capture", "licence": "AGPL-3.0-only", "credit": "Captured from the FARIS desktop app."}


class DocLicenceTests(unittest.TestCase):
    # Verifies: DOC-037
    def test_the_repository_passes(self):
        credits = json.loads((lint.ROOT / lint.CREDITS).read_text(encoding="utf-8"))
        self.assertEqual(lint.check_footer(), [])
        self.assertEqual(lint.check_credits(lint.figure_files(), lint.shown_figures(), credits), [])

    # Verifies: DOC-037
    def test_a_figure_without_a_credit_or_owner_is_reported(self):
        credits = {"figures": {
            "docs/images/a.png": GOOD,
            "docs/images/b.png": {"origin": "other", "licence": "CC-BY-4.0", "credit": "From a paper."},
            "docs/images/c.png": {"origin": "faris-capture", "licence": "", "credit": ""},
            "docs/images/gone.png": GOOD,
        }}
        files = ["docs/images/a.png", "docs/images/b.png", "docs/images/c.png", "docs/images/d.png"]
        found = lint.check_credits(files, {"https://example.org/x.png"}, credits)
        text = "\n".join(found)
        self.assertIn("b.png: a figure not captured from FARIS must name its owner", text)
        self.assertIn("c.png: entry needs a licence", text)
        self.assertIn("d.png: figure has no entry", text)
        self.assertIn("gone.png: credit entry for a figure", text)
        self.assertIn("external figure", text)
        self.assertNotIn("a.png", text)

    # Verifies: DOC-037
    def test_every_built_page_must_load_the_footer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a.html").write_text('<script src="docs/licence-footer-e24de7b0.js"></script>')
            (root / "b.html").write_text("<p>no footer</p>")
            self.assertEqual(lint.check_built(root), ["b.html: does not load the licence footer script"])


if __name__ == "__main__":
    unittest.main()
