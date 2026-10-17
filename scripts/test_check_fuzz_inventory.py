#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_fuzz_inventory as cfi  # noqa: E402

ROOT = cfi.ROOT
CI = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")


def copy_inputs(dest: Path) -> None:
    for rel in (cfi.PAGE, cfi.ENTRIES):
        (dest / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / rel, dest / rel)
    shutil.copytree(ROOT / cfi.TARGETS, dest / cfi.TARGETS)


class FuzzInventoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        copy_inputs(self.tmp)

    def test_the_repository_inventory_matches_the_fuzz_targets(self):
        # Verifies: SEC-020
        self.assertEqual(cfi.check(), [])
        self.assertGreaterEqual(len(cfi.table_rows((ROOT / cfi.PAGE).read_text(encoding="utf-8"))), 17)

    def test_a_target_file_without_a_row_fails(self):
        # Verifies: SEC-020
        (self.tmp / cfi.TARGETS / "new_parser.rs").write_text("faris_fuzz_entries::scenario(data);\n", encoding="utf-8")
        self.assertTrue(any("new_parser.rs is a fuzz target with no row" in p for p in cfi.check(self.tmp)))

    def test_a_row_without_a_target_file_fails(self):
        # Verifies: SEC-020
        (self.tmp / cfi.TARGETS / "png_header.rs").unlink()
        self.assertTrue(any("png_header, which has no file" in p for p in cfi.check(self.tmp)))

    def test_a_target_that_calls_no_defined_or_listed_entry_fails(self):
        # Verifies: SEC-020
        target = self.tmp / cfi.TARGETS / "scenario.rs"
        target.write_text("faris_fuzz_entries::missing_entry(data);\n", encoding="utf-8")
        self.assertTrue(any("missing_entry, which" in p for p in cfi.check(self.tmp)))
        target.write_text("", encoding="utf-8")
        self.assertTrue(any("calls no function" in p for p in cfi.check(self.tmp)))
        entries = self.tmp / cfi.ENTRIES
        entries.write_text(entries.read_text(encoding="utf-8") + "\npub fn orphan(_data: &[u8]) {}\n", encoding="utf-8")
        target.write_text("faris_fuzz_entries::orphan(data);\n", encoding="utf-8")
        self.assertTrue(any("orphan is not listed in ENTRIES" in p for p in cfi.check(self.tmp)))

    def test_a_claimed_entry_point_the_entries_never_call_fails(self):
        # Verifies: SEC-020
        page = self.tmp / cfi.PAGE
        page.write_text(page.read_text(encoding="utf-8").replace("`design::parse_design`", "`design::parse_imaginary`"), encoding="utf-8")
        self.assertTrue(any("never calls parse_imaginary" in p for p in cfi.check(self.tmp)))

    def test_ci_runs_the_inventory_check(self):
        # Verifies: SEC-020
        self.assertIn("python3 scripts/check_fuzz_inventory.py", CI)


if __name__ == "__main__":
    unittest.main()
