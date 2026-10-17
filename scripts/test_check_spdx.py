# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the SPDX header check."""
import tempfile
import unittest
from pathlib import Path

import check_spdx


class SpdxTests(unittest.TestCase):
    def test_a_file_without_the_lines_is_reported(self):
        # Verifies: LEG-002
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "good.rs").write_text("// SPDX-License-Identifier: AGPL-3.0-only\n// SPDX-FileCopyrightText: 2026 Avila Labs\n")
            (root / "no_holder.py").write_text("# SPDX-License-Identifier: AGPL-3.0-only\n")
            (root / "none.sh").write_text("#!/bin/sh\n")
            found = check_spdx.problems(["good.rs", "no_holder.py", "none.sh"], root)
            self.assertEqual(len(found), 3)
            self.assertTrue(all("good.rs" not in f for f in found))

    def test_every_tracked_source_file_carries_the_header(self):
        # Verifies: LEG-002
        files = check_spdx.tracked_files()
        self.assertGreater(len(files), 500)
        self.assertEqual(check_spdx.problems(files), [])


if __name__ == "__main__":
    unittest.main()
