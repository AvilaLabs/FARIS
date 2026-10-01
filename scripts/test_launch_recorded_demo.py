# SPDX-License-Identifier: AGPL-3.0-only
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from launch_recorded_demo import split_app_override


class AppOverrideTests(unittest.TestCase):
    def test_no_override_leaves_arguments_untouched(self):
        self.assertEqual(split_app_override(["--step", "compare"]), (None, ["--step", "compare"]))

    def test_override_is_removed_and_other_arguments_pass_through(self):
        self.assertEqual(
            split_app_override(["--sweep-bundle", "a.json", "--app", "/x/faris-app", "--step", "compare"]),
            ("/x/faris-app", ["--sweep-bundle", "a.json", "--step", "compare"]),
        )
        self.assertEqual(split_app_override(["--app=/y/app"]), ("/y/app", []))

    def test_malformed_overrides_are_rejected(self):
        for arguments in (["--app"], ["--app", "--step"], ["--app="], ["--app", "a", "--app=b"]):
            with self.assertRaises(ValueError, msg=arguments):
                split_app_override(arguments)


if __name__ == "__main__":
    unittest.main()
