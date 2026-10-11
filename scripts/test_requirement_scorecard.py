import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import requirement_scorecard as sc
import requirement_trace as rt

HEADER = "| ID | Requirement | Target | Verification | Basis | Phase | Now |\n| --- | --- | --- | --- | --- | --- | --- |\n"


def reqs(rows: list[str], name: str = "01-x.md") -> dict[str, dict]:
    """Parse synthetic requirement rows the way requirement_trace does."""
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        (Path(tmp) / name).write_text("# X\n\n" + HEADER + "\n".join(rows) + "\n", encoding="utf-8")
        old = rt.REQUIREMENTS
        rt.REQUIREMENTS = Path(tmp)
        try:
            return rt.read_requirements()
        finally:
            rt.REQUIREMENTS = old


ROWS = [
    "| AAA-001 | a | t | v | OpenMC [V](https://x.org/a) | F1 | Met: done |",
    "| AAA-002 | b | t (provisional: guess; confirm before F2 gate) | v | [U]; FARIS choice | F2 | No |",
    "| AAA-003 | c | t | v | | F1 | Partial: some |",
    "| ~~AAA-004~~ | withdrawn | t | v | [V] | F1 | No |",
    "| AAA-005 | d | t | v | [internal] | F2 | Unmeasured |",
]


class ScorecardTests(unittest.TestCase):
    def setUp(self):
        self.reqs = reqs(ROWS)
        self.trace = {"AAA-002": ["t.rs::a"], "AAA-003": ["t.rs::b"]}
        sc.annotate(self.reqs, self.trace)

    def test_withdrawn_rows_are_excluded(self):
        self.assertEqual(list(self.reqs), ["AAA-001", "AAA-002", "AAA-003", "AAA-005"])

    def test_counts(self):
        counts = sc.tally(list(self.reqs), self.reqs)
        # total, Met, Partial, Unmeasured, No, traced, [V], [U], FARIS choice, [internal], no basis, provisional
        self.assertEqual(counts, [4, 1, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1])
        self.assertEqual(sc.cells(counts)[:7], ["4", "1", "1", "1", "1", "2", "50 %"])

    def test_phase_counts(self):
        f2 = [r for r in self.reqs if self.reqs[r]["phase"] == "F2"]
        self.assertEqual(sc.tally(f2, self.reqs)[:5], [2, 0, 0, 1, 1])

    def test_peers_and_link_names(self):
        self.assertEqual(sc.peers(list(self.reqs), self.reqs), ["OpenMC"])
        self.assertEqual(sc.link_name("V"), "")
        self.assertEqual(sc.link_name("V, via search"), "")
        self.assertEqual(sc.link_name("V: [NASA report"), "NASA report")
        self.assertEqual(sc.link_name("source"), "")

    def test_render_lists_exceptions(self):
        text = sc.render(self.reqs, self.trace)
        self.assertIn("## Met without a checker (1)", text)
        self.assertIn("- AAA-001 (01-x.md)", text)
        self.assertIn("## Traced but marked No (1)", text)
        self.assertIn("- AAA-002 (01-x.md): `t.rs::a`", text)

    def test_render_is_deterministic(self):
        self.assertEqual(sc.render(self.reqs, self.trace), sc.render(self.reqs, self.trace))


if __name__ == "__main__":
    unittest.main()
