# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for the documentation link checker (DOC-026)."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_links as cl  # noqa: E402


def tree(files: dict[str, str]) -> Path:
    root = Path(tempfile.mkdtemp())
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    (root / "docs").mkdir(exist_ok=True)
    return root


class LinkTests(unittest.TestCase):
    def problems(self, files):
        return cl.check(tree(files))[0]

    def test_a_good_tree_passes_and_counts_its_links(self):
        # Verifies: DOC-026
        root = tree({
            "README.md": "See [guide](docs/a.md#second-part) and [top](#intro) and ![img](docs/p.png).\n\n# Intro\n",
            "docs/a.md": "# A\n\n## Second part\n\nBack to [readme](../README.md#intro).\n",
            "docs/p.png": "x",
        })
        problems, stats = cl.check(root)
        self.assertEqual(problems, [])
        self.assertEqual(stats["internal"], 4)
        self.assertEqual(stats["anchors"], 3)

    def test_a_missing_file_a_missing_anchor_and_an_escape_from_the_repository_fail(self):
        # Verifies: DOC-026
        found = self.problems({
            "README.md": "[a](docs/gone.md) [b](docs/a.md#nope) [c](../outside.md)\n",
            "docs/a.md": "# A\n",
        })
        self.assertEqual(len(found), 3)
        self.assertIn("no such file", found[0])
        self.assertIn("no heading or anchor 'nope'", found[1])
        self.assertIn("leaves the repository", found[2])

    def test_heading_slugs_follow_the_github_rule(self):
        # Verifies: DOC-026
        self.assertEqual(cl.slug("Hello, World!"), "hello-world")
        self.assertEqual(cl.slug("The `faris` command (v2)"), "the-faris-command-v2")
        self.assertEqual(cl.slug("snake_case word"), "snake_case-word")
        found = self.problems({
            "README.md": "[x](docs/a.md#repeat-1) [y](docs/a.md#the-faris-command-v2)\n",
            "docs/a.md": "# Repeat\n\n# Repeat\n\n# The `faris` command (v2)\n",
        })
        self.assertEqual(found, [])

    def test_html_anchors_count_and_code_is_ignored(self):
        # Verifies: DOC-026
        found = self.problems({
            "README.md": '[x](docs/a.md#custom)\n\n```\n[bad](nowhere.md)\n```\n\nInline `[bad](nowhere.md)` is code.\n',
            "docs/a.md": '<a id="custom"></a>\ntext\n',
        })
        self.assertEqual(found, [])

    def test_external_links_are_only_syntax_checked_and_never_fetched(self):
        # Verifies: DOC-026
        found = self.problems({
            "README.md": "[ok](https://example.org/a?b=c#d) [mail](mailto:a@example.org) [bad](https://) "
                         "[bad2](https://exa mple.org) [bad3](ftp://example.org)\n",
        })
        self.assertEqual(len(found), 2)
        text = "\n".join(found)
        self.assertIn("no valid host", text)
        self.assertIn("unsupported link scheme", text)
        self.assertNotIn("example.org/a", text)

    def test_reference_definitions_and_autolinks_are_checked(self):
        # Verifies: DOC-026
        found = self.problems({"README.md": "[x][r] <https://example.org/x>\n\n[r]: docs/gone.md\n"})
        self.assertEqual(len(found), 1)
        self.assertIn("docs/gone.md", found[0])

    def test_the_repositorys_own_documents_have_no_broken_link(self):
        # Verifies: DOC-026
        root = Path(__file__).resolve().parent.parent
        problems, stats = cl.check(root)
        self.assertEqual(problems, [])
        self.assertGreater(stats["internal"], 500)


if __name__ == "__main__":
    unittest.main()
