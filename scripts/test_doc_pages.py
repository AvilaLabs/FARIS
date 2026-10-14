# SPDX-License-Identifier: AGPL-3.0-only
"""Checks on the documentation pages themselves: kind labels, the licence footer and the limitations page."""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KINDS = {"tutorial", "how-to", "reference", "explanation"}
# Generated listings carry no kind label, and the handbook contents page is not a page. The two maintenance protocols
# are frozen texts whose SHA-256 is checked by scripts/maintenance_validation.py and maintenance_coupling_test.py, so
# an added line would break them; both are explanation.
EXEMPT = {
    "docs/notes/MAINTENANCE_COUPLING_VALIDATION.md",
    "docs/notes/MAINTENANCE_COUPLING_TEST.md",
    "docs/THIRD_PARTY_NOTICES.md",
    "docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md",
    "docs/guide/SUMMARY.md",
}


def pages():
    found = [ROOT / "README.md", ROOT / "CONTRIBUTING.md"]
    for pattern in ("docs/*.md", "docs/guide/*.md", "docs/notes/*.md", "docs/defects/*.md"):
        found += sorted(ROOT.glob(pattern))
    return [p for p in found if p.relative_to(ROOT).as_posix() not in EXEMPT]


class DocumentationPageTests(unittest.TestCase):
    def test_every_page_declares_one_of_the_four_kinds(self):
        # Verifies: DOC-001
        problems = []
        for page in pages():
            first = page.read_text(encoding="utf-8").split("\n", 1)[0]
            match = re.fullmatch(r"<!-- kind: ([a-z-]+) -->", first)
            if not match or match.group(1) not in KINDS:
                problems.append(page.relative_to(ROOT).as_posix())
        self.assertEqual(problems, [], "pages without a first-line '<!-- kind: tutorial|how-to|reference|explanation -->'")

    def test_the_handbook_adds_the_licence_footer_to_every_page(self):
        # Verifies: DOC-037
        book = (ROOT / "book.toml").read_text(encoding="utf-8")
        match = re.search(r'additional-js\s*=\s*\[(.*?)\]', book)
        self.assertIsNotNone(match, "book.toml has no additional-js")
        script = (ROOT / "docs/licence-footer.js")
        self.assertIn('"docs/licence-footer.js"', match.group(1))
        text = script.read_text(encoding="utf-8")
        self.assertIn("AGPL-3.0-only", text)
        self.assertIn("main.appendChild", text)
        licence = (ROOT / "LICENSE").read_text(encoding="utf-8")
        self.assertIn("GNU AFFERO GENERAL PUBLIC LICENSE", licence)

    def test_limitations_rows_name_a_requirement_that_exists(self):
        # Verifies: DOC-009
        known = set()
        for table in (ROOT / "docs/requirements").glob("[0-9][0-9]-*.md"):
            known |= set(re.findall(r"^\| ([A-Z]+-\d+) \|", table.read_text(encoding="utf-8"), re.M))
        page = (ROOT / "docs/guide/limitations.md").read_text(encoding="utf-8")
        self.assertRegex(page, r"Last checked: \d{4}-\d{2}-\d{2}")
        rows = [line for line in page.splitlines() if line.startswith("| ") and not line.startswith(("| Gap", "| ---"))]
        self.assertGreater(len(rows), 10)
        for row in rows:
            cells = [c.strip() for c in row.strip("|").split("|")]
            self.assertEqual(len(cells), 3, row)
            self.assertIn(cells[1], known, row)
            self.assertTrue(cells[2], row)
        summary = (ROOT / "docs/guide/SUMMARY.md").read_text(encoding="utf-8")
        self.assertIn("(limitations.md)", summary)

    def test_every_handbook_page_is_in_the_contents(self):
        summary = (ROOT / "docs/guide/SUMMARY.md").read_text(encoding="utf-8")
        listed = set(re.findall(r"\(([\w./-]+\.md)\)", summary))
        on_disk = {p.name for p in (ROOT / "docs/guide").glob("*.md")} - {"SUMMARY.md"}
        self.assertEqual(on_disk - listed, set())

    def test_every_tour_stop_has_a_row_with_a_manual_link(self):
        # Verifies: DOC-056 (the titles only; the tour text is still a separate copy)
        source = (ROOT / "crates/faris-app/src/tour.rs").read_text(encoding="utf-8")
        titles = re.findall(r'title: "((?:[^"\\]|\\.)*)",', source)
        self.assertEqual(len(titles), 12)
        page = (ROOT / "docs/guide/quick-start.md").read_text(encoding="utf-8")
        for title in titles:
            rows = [line for line in page.splitlines() if line.startswith(f"| {title} |")]
            self.assertEqual(len(rows), 1, title)
            self.assertIn("](", rows[0], title)

    def test_the_external_tool_table_lists_a_licence_for_every_row(self):
        # Verifies: LEG-016
        page = (ROOT / "docs/guide/external-tools.md").read_text(encoding="utf-8")
        rows = [line for line in page.splitlines() if line.startswith("| ") and not line.startswith(("| Tool", "| ---"))]
        self.assertGreaterEqual(len(rows), 8)
        for row in rows:
            cells = [c.strip() for c in row.strip("|").split("|")]
            self.assertEqual(len(cells), 5, row)
            self.assertTrue(all(cells), row)
        for tool in ("OpenMC", "Avila Core", "ACTINV", "DAGMC", "OpenCASCADE"):
            self.assertIn(tool, page)


if __name__ == "__main__":
    unittest.main()
