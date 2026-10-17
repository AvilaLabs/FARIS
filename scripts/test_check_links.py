# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
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

    def test_a_nested_link_is_reported_as_malformed(self):
        # Verifies: DOC-026
        found = self.problems({"README.md": "ok\n\nBasis [V-2]([Hullman 2015](https://doi.org/10.1/x))\n"})
        self.assertEqual(len(found), 1)
        self.assertIn("README.md:3: malformed nested link", found[0])

    def test_online_check_fetches_each_address_once_and_separates_dead_from_refusing(self):
        # Verifies: DOC-026
        seen = []

        def fetch(url):
            seen.append(url)
            return {"https://a.org/ok": 200, "https://a.org/dead": 404, "https://a.org/bot": 403,
                    "https://a.org/gone": "no answer (timed out)"}[url]

        # the fragment of a cited address is not sent to the server
        cited = {"https://a.org/ok": ["x.md:1"], "https://a.org/dead#frag": ["x.md:2", "y.md:3"],
                 "https://a.org/bot": ["z.md:4"], "https://a.org/gone": ["w.md:5"]}
        broken, warnings = cl.online_check(cited, fetch)
        self.assertEqual(sorted(seen), ["https://a.org/bot", "https://a.org/dead", "https://a.org/gone", "https://a.org/ok"])
        self.assertEqual(len(broken), 2)
        self.assertTrue(any("404" in b and "x.md:2, y.md:3" in b for b in broken))
        self.assertTrue(any("no answer" in b for b in broken))
        self.assertEqual(len(warnings), 1)
        self.assertIn("403", warnings[0])

    def test_fetch_status_reads_a_local_server(self):
        # Verifies: DOC-026
        import threading
        from http.server import BaseHTTPRequestHandler, HTTPServer

        class Handler(BaseHTTPRequestHandler):
            def do_HEAD(self):
                self.send_response(200 if self.path == "/ok" else 404)
                self.end_headers()

            do_GET = do_HEAD

            def log_message(self, *args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            base = f"http://127.0.0.1:{server.server_port}"
            self.assertEqual(cl.fetch_status(base + "/ok", timeout=5), 200)
            self.assertEqual(cl.fetch_status(base + "/missing", timeout=5), 404)
        finally:
            server.shutdown()
        self.assertIsInstance(cl.fetch_status("http://127.0.0.1:9/", timeout=2), str)

    def test_the_weekly_workflow_runs_the_online_check(self):
        # Verifies: DOC-026
        text = (Path(__file__).resolve().parent.parent / ".github/workflows/links.yml").read_text(encoding="utf-8")
        self.assertIn("schedule:", text)
        self.assertRegex(text, r'cron: "\S+ \S+ \* \* 1"')
        self.assertIn("check_links.py --online", text)

    def test_the_repositorys_own_documents_have_no_broken_link(self):
        # Verifies: DOC-026
        root = Path(__file__).resolve().parent.parent
        problems, stats = cl.check(root)
        self.assertEqual(problems, [])
        self.assertGreater(stats["internal"], 500)


if __name__ == "__main__":
    unittest.main()
