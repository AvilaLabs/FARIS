"""Tests for ../openmc_ci.py: a required environment turns a skip into a failure. Standard library only."""

import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import openmc_ci  # noqa: E402


class SkipOrFail(unittest.TestCase):
    def test_skips_when_openmc_is_not_required(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop(openmc_ci.REQUIRE_ENV, None)
            with self.assertRaises(unittest.SkipTest):
                openmc_ci.skip_or_fail("no OpenMC here")

    def test_fails_when_openmc_is_required(self):
        with mock.patch.dict(os.environ, {openmc_ci.REQUIRE_ENV: "1"}):
            with self.assertRaises(AssertionError) as caught:
                openmc_ci.skip_or_fail("no OpenMC here")
            self.assertIn("no OpenMC here", str(caught.exception))

    def test_a_missing_module_follows_the_same_rule(self):
        with mock.patch.dict(os.environ, {openmc_ci.REQUIRE_ENV: "1"}):
            with self.assertRaises(AssertionError):
                openmc_ci.require_modules("no_such_module_faris")
        os.environ.pop(openmc_ci.REQUIRE_ENV, None)
        with self.assertRaises(unittest.SkipTest):
            openmc_ci.require_modules("no_such_module_faris")


class DataFolder(unittest.TestCase):
    def test_no_folder_skips_or_fails(self):
        with mock.patch.dict(os.environ, {openmc_ci.REQUIRE_ENV: "1"}):
            os.environ.pop(openmc_ci.DATA_ENV, None)
            with self.assertRaises(AssertionError):
                openmc_ci.neutron_data_dir()

    def test_a_file_with_the_wrong_bytes_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            for name in openmc_ci.DATA_SHA256:
                (Path(folder) / name).write_bytes(b"not nuclear data")
            with mock.patch.dict(os.environ, {openmc_ci.DATA_ENV: folder}):
                with self.assertRaises(AssertionError) as caught:
                    openmc_ci.neutron_data_dir()
            self.assertIn("not the FENDL-3.2 file", str(caught.exception))

    def test_the_library_file_lists_both_nuclides(self):
        with tempfile.TemporaryDirectory() as folder:
            path = openmc_ci.cross_sections_xml(Path(folder), Path(folder))
            text = path.read_text()
            self.assertIn('materials="H1"', text)
            self.assertIn('materials="Fe56"', text)


if __name__ == "__main__":
    unittest.main()
