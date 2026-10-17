# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the reproducible handbook build."""
import shutil
import tempfile
import unittest
from pathlib import Path

import build_docs


class BuildDocsTests(unittest.TestCase):
    # Verifies: DOC-036
    def test_the_tree_hash_depends_on_names_and_contents(self):
        with tempfile.TemporaryDirectory() as tmp:
            a, b = Path(tmp) / "a", Path(tmp) / "b"
            for directory in (a, b):
                (directory / "x").mkdir(parents=True)
                (directory / "x" / "page.html").write_text("one")
            self.assertEqual(build_docs.tree_hash(a), build_docs.tree_hash(b))
            (b / "x" / "page.html").write_text("two")
            self.assertNotEqual(build_docs.tree_hash(a), build_docs.tree_hash(b))
            (b / "x" / "page.html").write_text("one")
            (b / "x" / "page.html").rename(b / "x" / "other.html")
            self.assertNotEqual(build_docs.tree_hash(a), build_docs.tree_hash(b))

    # Verifies: DOC-036
    def test_every_file_the_book_reads_is_tracked(self):
        files = build_docs.book_sources()
        self.assertIn("docs/guide/index.md", files)
        self.assertIn("docs/licence-footer.js", files)
        self.assertEqual(build_docs.untracked(files), [])
        self.assertEqual(build_docs.untracked(["docs/guide/not-in-version-control.md"]), ["docs/guide/not-in-version-control.md"])

    # Verifies: DOC-036
    @unittest.skipUnless(shutil.which("mdbook"), "mdbook is not installed")
    def test_two_builds_of_the_handbook_have_the_same_hash(self):
        self.assertEqual(build_docs.main([]), 0)


if __name__ == "__main__":
    unittest.main()
