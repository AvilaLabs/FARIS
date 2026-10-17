# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the readability gate."""
import unittest

import readability as rd


class ReadabilityTests(unittest.TestCase):
    # Verifies: DOC-053
    def test_code_tables_headings_and_links_are_not_prose(self):
        text = "# Title\n\n| a | b |\n| - | - |\n\n```\nlet x = 1;\n```\n\nSee [the page](https://example.org/x) and `code`.\n"
        self.assertEqual(rd.sentences(rd.prose(text)), ["See the page and x."])

    # Verifies: DOC-053
    def test_plain_text_scores_low_and_dense_text_scores_high(self):
        easy = "The cat sat on the mat. The dog ran to the park. We like to play in the sun.\n"
        hard = ("Characterisation of heterogeneous radiological consequences necessitates comprehensive "
                "consideration of multidimensional uncertainty propagation methodologies.\n")
        self.assertLess(rd.measure(easy)["grade"], 3)
        self.assertGreater(rd.measure(hard)["grade"], 16)
        self.assertEqual(rd.measure(easy)["sentences"], 3)

    # Verifies: DOC-053
    def test_syllables_and_passive_voice(self):
        self.assertEqual([rd.syllables(w) for w in ("cat", "table", "make", "water", "reading")], [1, 2, 1, 2, 2])
        m = rd.measure("The file is stored on disk. The tool reads the file. The run was stopped early.\n")
        self.assertAlmostEqual(m["passive"], 2 / 3, places=2)

    # Verifies: DOC-053
    def test_the_handbook_meets_the_gates(self):
        problems, report = rd.check()
        self.assertEqual(problems, [])
        self.assertGreater(len(report), 30)
        self.assertLessEqual(report["(manual)"]["mean_sentence"], 20)


if __name__ == "__main__":
    unittest.main()
