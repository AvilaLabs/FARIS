# SPDX-License-Identifier: AGPL-3.0-only
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check_install_size.py")
DESKTOP = Path(__file__).resolve().parent.parent / ".github/workflows/desktop.yml"


def run(folder: Path, *extra: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(SCRIPT), str(folder), *extra], capture_output=True, text=True)


class InstallSizeTests(unittest.TestCase):
    # Verifies: PERF-045, PLAT-026
    def test_a_folder_over_the_limit_fails_and_one_under_it_passes(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / "bin").mkdir()
            (root / "bin" / "faris").write_bytes(bytes(2_000_000))
            (root / "licenses").mkdir()
            (root / "licenses" / "notices.md").write_bytes(bytes(1_500_000))
            self.assertEqual(run(root).returncode, 0)
            self.assertEqual(run(root, "--limit-mb", "3.4").returncode, 1)
            self.assertEqual(run(root, "--limit-mb", "3.6").returncode, 0)
            self.assertIn("3.5 MB", run(root).stdout)

    # Verifies: PERF-045, PLAT-026
    def test_the_release_build_workflow_runs_the_check_on_the_collected_folder_at_150_mb(self):
        text = DESKTOP.read_text(encoding="utf-8")
        self.assertIn("python3 scripts/check_install_size.py out", text)
        self.assertNotIn("--limit-mb", text)
        # It runs after the binaries are collected and before they are uploaded.
        self.assertLess(text.index("Collect binaries and build record"), text.index("check_install_size.py"))
        self.assertLess(text.index("check_install_size.py"), text.index("actions/upload-artifact"))
        self.assertIn("LIMIT_MB = 150", SCRIPT.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
