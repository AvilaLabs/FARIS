import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import export_lint as E

STATEMENT = E.research_statement()
TWIN = "FARIS outputs come from a design model, not a digital twin."
GOOD_CSV = (f"# {STATEMENT}\n"
            "arrangement,blanket_m,blanket_kind,breeding_ratio_h3_per_source_neutron,breeding_ratio_standard_error,breeding_ratio_kind\n"
            "a,0.45,authored,1.2,0.001,calculated\n")
GOOD_SVG = ('<svg><text x="1" y="1" text-anchor="start">Breeding, calculated</text>'
            '<text x="1" y="2" text-anchor="start">sampling uncertainty only, ±2 SE</text>'
            '<text x="1" y="3" text-anchor="middle">Blanket thickness (m)</text>'
            '<text x="1" y="4" text-anchor="middle" transform="rotate(-90 1 1)">Breeding ratio (dimensionless)</text>'
            f'<text x="1" y="5" text-anchor="start">{STATEMENT}</text></svg>')


def build(root: Path, csv_text: str = GOOD_CSV, svg: str = GOOD_SVG, twin: str = TWIN) -> Path:
    (root / "data").mkdir()
    (root / "charts").mkdir()
    files = {"data/comparison.csv": csv_text.encode(), "charts/sweep-breeding.svg": svg.encode()}
    for rel, raw in files.items():
        (root / rel).write_bytes(raw)
    manifest = {"schema": "faris-export/1", "faris_version": "1", "research_screening": STATEMENT, "generated_utc": "2026-10-14T00:00:00Z",
                "study_file": None, "note": twin,
                "files": [{"path": rel, "sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)} for rel, raw in files.items()]}
    (root / "export-manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    return root


def failing(export: Path) -> list[str]:
    return [f"{r}: {m}" for r, m in E.lint_export(export) if r not in E.ADVISORY]


class ExportLint(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    # Verifies: INT-040, INT-044, DOC-062, VIS-040, VIS-033, PRV-010
    def test_a_clean_export_has_no_finding_except_the_stamp(self):
        found = E.lint_export(build(self.root))
        self.assertEqual({r for r, _ in found}, {"INT-040-stamp"})

    # Verifies: INT-040
    def test_format_problems_are_failures(self):
        bad = GOOD_CSV.replace("\n", "\r\n").replace("0.45", '"0,45"')
        export = build(self.root, csv_text=bad)
        (export / "charts" / "sweep-breeding.svg").write_text(GOOD_SVG + " ", encoding="utf-8")  # no longer the hashed bytes
        found = failing(export)
        self.assertTrue(any("CR characters" in f for f in found))
        self.assertTrue(any("decimal comma" in f for f in found))
        self.assertTrue(any("full SHA-256 does not match" in f for f in found))

    # Verifies: INT-040, DOC-062
    def test_the_statement_must_be_the_first_line(self):
        found = failing(build(self.root, csv_text=GOOD_CSV.split("\n", 1)[1]))
        self.assertTrue(any("first line is not the research-screening statement" in f for f in found))

    # Verifies: INT-040, PRV-010
    def test_units_and_kinds_are_audited_per_column(self):
        csv_text = f"# {STATEMENT}\narrangement,mystery,flux_n_m2_s,flux_kind\na,3.2,4.1,calculated\n"
        found = [(r, m) for r, m in E.lint_export(build(self.root, csv_text=csv_text))]
        self.assertTrue(any(r == "INT-040-units" and "mystery" in m for r, m in found))
        self.assertTrue(any(r == "PRV-010" and "mystery" in m for r, m in found))
        self.assertTrue(any(r == "VIS-030" and "flux" in m for r, m in found))

    # Verifies: PRV-012, UX-046
    def test_not_evaluated_rows_need_a_reason_and_a_next_step(self):
        csv_text = f"# {STATEMENT}\nitem,state,reason,next_step,note\na,not_evaluated,,,\nb,not_evaluated,No covariance was recorded.,Rerun transport.,\nc,interval,N/A,,\n"
        found = [m for r, m in E.lint_export(build(self.root, csv_text=csv_text)) if r in ("PRV-012", "UX-046")]
        self.assertTrue(any("row 2" in m and "reason" in m for m in found))
        self.assertFalse(any("row 3" in m for m in found))
        self.assertTrue(any("row 4" in m and "N/A" in m for m in found))

    # Verifies: VIS-040, VIS-033
    def test_chart_axes_kind_and_error_scope_are_audited(self):
        svg = ('<svg><text text-anchor="start">Breeding</text><text text-anchor="start">±2 SE</text>'
               f'<text text-anchor="middle">Blanket thickness</text><text text-anchor="start">{STATEMENT}</text></svg>')
        rules = [r for r, _ in E.lint_export(build(self.root, svg=svg))]
        self.assertEqual(rules.count("VIS-040"), 3)  # x unit, y axis, kind
        self.assertIn("VIS-033", rules)

    # Verifies: INT-044
    def test_png_needs_an_svg_sidecar_and_files_must_be_documented(self):
        export = build(self.root)
        (export / "charts" / "lonely.png").write_bytes(b"x")
        (export / "notes.txt").write_text("x")
        found = failing(export)
        self.assertTrue(any("lonely.png" in f and "sidecar" in f for f in found))
        self.assertTrue(any("notes.txt" in f and "not documented" in f for f in found))

    # Verifies: DOC-062, VAL-090
    def test_a_claim_in_an_export_and_a_missing_twin_statement_are_found(self):
        csv_text = GOOD_CSV + "note,This design is licensed for use.,,,\n"
        found = E.lint_export(build(self.root, csv_text=csv_text, twin="nothing here"))
        self.assertTrue(any(r == "VAL-090" for r, _ in found))
        self.assertTrue(any(r == "DOC-062" and "manifest" in m for r, m in found))


if __name__ == "__main__":
    unittest.main()
