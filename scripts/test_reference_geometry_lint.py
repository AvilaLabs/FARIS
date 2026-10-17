# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference_geometry_lint as L  # noqa: E402

LONG = "ports, penetrations, coolant channels and every three-dimensional feature of the real machine are left out here"


def section(title, citation="[a paper](https://example.org/p)", use="teaching", omits=LONG, files="`scenarios/x/s.json`"):
    lines = [f"## {title}", ""]
    for label, text in (("Files", files), ("Citation", citation), ("Use", use), ("Omits", omits)):
        if text is not None:
            lines.append(f"- **{label}:** {text}")
    return "\n".join(lines) + "\n"


def page(*sections):
    return "# Reference geometries\n\nIntro.\n\n" + "\n".join(sections)


GOOD = [section("One"), section("Two", files="external, not vendored."), section("Three")]
SCENARIO = {"geometry": {}, "references": [{"id": "r", "title": "T", "url": "https://example.org", "use": "inspiration"}],
            "assumptions": ["Idealized circular torus with no resolved ports."]}


class ReferenceGeometryLint(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        (self.root / "docs").mkdir()
        (self.root / "scenarios" / "x").mkdir(parents=True)
        (self.root / "scenarios" / "x" / "s.json").write_text(json.dumps(SCENARIO), encoding="utf-8")

    def write_page(self, *sections):
        (self.root / L.PAGE).write_text(page(*sections), encoding="utf-8")

    # Verifies: GEO-009
    def test_the_repository_reference_geometries_state_citation_and_omissions(self):
        self.assertEqual(L.run(), [])

    # Verifies: GEO-009
    def test_a_complete_page_and_scenario_pass(self):
        self.write_page(*GOOD)
        self.assertEqual(L.run(self.root), [])

    # Verifies: GEO-009
    def test_fewer_than_three_sets_a_missing_citation_link_or_a_thin_omission_line_is_found(self):
        self.write_page(*GOOD[:2])
        self.assertTrue(any("2 reference sets" in f for f in L.run(self.root)))
        self.write_page(section("One", citation="a paper, no link"), section("Two", omits=None), section("Three", omits="none"))
        found = " | ".join(L.run(self.root))
        for part in ("One: the citation has no link", "Two: no Omits line", "Three: the omissions are not stated"):
            self.assertIn(part, found)

    # Verifies: GEO-009
    def test_a_listed_file_that_does_not_exist_is_found(self):
        self.write_page(section("One", files="`scenarios/x/gone.json`"), GOOD[1], GOOD[2])
        self.assertEqual(len(L.run(self.root)), 1)
        self.assertIn("gone.json does not exist", L.run(self.root)[0])

    # Verifies: GEO-009
    def test_a_scenario_without_references_or_a_stated_omission_is_found(self):
        self.write_page(*GOOD)
        bad = {"geometry": {}, "references": [{"id": "r", "title": "T", "url": "http://x", "use": ""}],
               "assumptions": ["All dimensions are authored."]}
        (self.root / "scenarios" / "x" / "s.json").write_text(json.dumps(bad), encoding="utf-8")
        found = " | ".join(L.run(self.root))
        for part in ("has no use", "has no https URL", "no assumption states what the geometry omits"):
            self.assertIn(part, found)
        (self.root / "scenarios" / "x" / "s.json").write_text(json.dumps({"geometry": {}}), encoding="utf-8")
        self.assertIn("no references", " | ".join(L.run(self.root)))


if __name__ == "__main__":
    unittest.main()
