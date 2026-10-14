# SPDX-License-Identifier: AGPL-3.0-only
import unittest

import check_repository_size as crs


class RepositorySizeTests(unittest.TestCase):
    # Verifies: QA-090

    def test_small_tree_passes(self):
        self.assertEqual(crs.check({"a.json": 10, "docs/b.md": 20}), [])

    def test_one_large_file_is_named(self):
        problems = crs.check({"big.json": crs.MAX_FILE_BYTES + 1})
        self.assertEqual(len(problems), 1)
        self.assertIn("big.json", problems[0])

    def test_total_over_the_limit_fails(self):
        sizes = {f"f{i}.json": crs.MAX_FILE_BYTES for i in range(6)}
        self.assertTrue(any("add up" in p for p in crs.check(sizes)))

    def test_nuclear_data_and_archives_are_refused(self):
        self.assertEqual(len(crs.check({"data/Fe56.h5": 5, "x/lib.ZIP": 5})), 2)

    def test_this_repository_passes(self):
        self.assertEqual(crs.check(crs.tracked_sizes(crs.ROOT)), [])


if __name__ == "__main__":
    unittest.main()
