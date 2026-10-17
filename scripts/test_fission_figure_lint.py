# SPDX-License-Identifier: AGPL-3.0-only
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fission_figure_lint as L  # noqa: E402


def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def table(acceptance: str) -> str:
    return f"| AAA-001 | text | {acceptance} | test | basis [V] | F1 | No |\n"


class FissionFigureLint(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    # Verifies: VAL-081
    def test_the_repository_uses_no_fission_figure_as_an_acceptance_number(self):
        self.assertEqual(L.run(), [])

    # Verifies: VAL-081
    def test_a_fission_figure_in_an_acceptance_column_is_found(self):
        write(self.root / "docs" / "requirements" / "01-a.md", table("TBR within 100 pcm of the benchmark."))
        found = L.run(self.root)
        self.assertEqual(len(found), 1)
        self.assertIn("'pcm' in the acceptance of AAA-001", found[0])

    # Verifies: VAL-081
    def test_boron_ppm_and_nodal_power_in_a_user_document_are_found_unless_labelled_context(self):
        write(self.root / "README.md", "FARIS matches boron to 10 ppm.\n\nThe nodal power rms stays below 1 %.\n\n"
                                       "For context, fission cores quote critical boron within 25 ppm.\n")
        found = L.run(self.root)
        self.assertEqual(len(found), 2)
        self.assertTrue(found[0].startswith("README.md:1:") and found[1].startswith("README.md:3:"))

    # Verifies: VAL-081
    def test_a_fission_figure_in_a_rust_string_is_found_but_a_comment_is_not(self):
        write(self.root / "crates" / "x" / "src" / "lib.rs",
              '// within 50 pcm is a fission habit\nconst T: &str = "accept 50 pcm";\n')
        found = L.run(self.root)
        self.assertEqual(len(found), 1)
        self.assertIn("lib.rs:2:", found[0])

    # Verifies: VAL-081
    def test_an_impurity_ppm_is_not_a_fission_figure(self):
        write(self.root / "docs" / "requirements" / "01-a.md", table("Cu impurity at most 50 ppm by weight."))
        self.assertEqual(L.run(self.root), [])


if __name__ == "__main__":
    unittest.main()
