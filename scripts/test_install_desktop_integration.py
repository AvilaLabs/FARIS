# SPDX-License-Identifier: AGPL-3.0-only
"""Installs and uninstalls the Linux desktop integration under a scratch XDG folder and diffs the file systems."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "install_desktop_integration.sh"
EXPECTED = {
    "mime/packages/avila-labs-faris-study.xml",
    "applications/faris.desktop",
    "icons/hicolor/256x256/apps/faris.png",
    "icons/hicolor/scalable/apps/faris.svg",
}


def files(root: Path) -> set:
    return {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}


@unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("bash"), "the script is for Linux")
class InstallDesktopIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.data = root / "data"
        self.data.mkdir()
        self.app = root / "faris-app"
        self.app.write_text("#!/bin/sh\n")
        self.app.chmod(0o755)
        self.env = dict(os.environ, XDG_DATA_HOME=str(self.data), HOME=str(root))

    def tearDown(self):
        self.temporary.cleanup()

    def run_script(self, *args):
        return subprocess.run(["bash", str(SCRIPT), *args], env=self.env, capture_output=True, text=True)

    def written_by_the_script(self):
        # The refresh step may add its own cache files (mime/*.cache, applications/mimeinfo.cache); they are
        # databases rebuilt from the entries, not files the installer places.
        return {f for f in files(self.data) if not f.startswith("mime/") or f.startswith("mime/packages/")
                if not f.endswith(".cache")}

    def test_install_writes_exactly_the_four_named_files_and_uninstall_removes_them(self):
        # Verifies: PLAT-023
        self.assertEqual(files(self.data), set())
        result = self.run_script(str(self.app))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.written_by_the_script(), EXPECTED)
        for name in EXPECTED:
            self.assertIn((self.data / name).as_posix(), result.stdout)
        result = self.run_script("--uninstall")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.written_by_the_script(), set())

    def test_the_mime_entry_names_the_type_the_glob_and_the_icon(self):
        # Verifies: PLAT-024
        self.assertEqual(self.run_script(str(self.app)).returncode, 0)
        xml = (self.data / "mime/packages/avila-labs-faris-study.xml").read_text()
        self.assertIn('type="application/vnd.avila-labs.faris-study"', xml)
        self.assertIn('<glob pattern="*.faris"/>', xml)
        self.assertIn('<icon name="faris"/>', xml)
        desktop = (self.data / "applications/faris.desktop").read_text()
        self.assertIn(f"Exec={self.app} %f", desktop)
        self.assertIn("MimeType=application/vnd.avila-labs.faris-study;", desktop)
        svg = (self.data / "icons/hicolor/scalable/apps/faris.svg").read_text()
        self.assertTrue(svg.startswith("<svg"))

    def test_a_relative_or_spaced_path_is_refused_and_nothing_is_written(self):
        self.assertEqual(self.run_script("faris-app").returncode, 2)
        spaced = self.app.parent / "a b"
        spaced.write_text("")
        spaced.chmod(0o755)
        self.assertEqual(self.run_script(str(spaced)).returncode, 2)
        self.assertEqual(files(self.data), set())

    def test_uninstall_leaves_other_user_files_alone(self):
        other = self.data / "applications" / "other.desktop"
        other.parent.mkdir(parents=True)
        other.write_text("[Desktop Entry]\n")
        self.assertEqual(self.run_script(str(self.app)).returncode, 0)
        self.assertEqual(self.run_script("--uninstall").returncode, 0)
        self.assertTrue(other.is_file())


if __name__ == "__main__":
    unittest.main()
