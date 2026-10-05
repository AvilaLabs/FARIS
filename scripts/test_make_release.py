import importlib.util
import os
import tarfile
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("make_release", Path(__file__).with_name("make_release.py"))
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


def package(root: Path) -> Path:
    pkg = root / "pkg"
    (pkg / "bin").mkdir(parents=True)
    (pkg / "README.md").write_text("package\n")
    (pkg / "bin" / "faris").write_text("#!/bin/sh\necho faris 0.1.0\n")
    os.chmod(pkg / "bin" / "faris", 0o555)
    return pkg


class MakeReleaseTests(unittest.TestCase):
    def test_same_package_gives_the_same_archive_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = package(root)
            a, b = root / "a.tar.gz", root / "b.tar.gz"
            RELEASE.write_archive(pkg, a, "FARIS-0.1.0-linux-x86_64", 1_790_000_000)
            os.utime(pkg / "README.md", (1, 1))  # a different mtime on disk must not matter
            RELEASE.write_archive(pkg, b, "FARIS-0.1.0-linux-x86_64", 1_790_000_000)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            with tarfile.open(a) as tar:
                names = tar.getnames()
                self.assertEqual(names[0], "FARIS-0.1.0-linux-x86_64")
                self.assertIn("FARIS-0.1.0-linux-x86_64/bin/faris", names)
                member = tar.getmember("FARIS-0.1.0-linux-x86_64/bin/faris")
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 1_790_000_000))
                self.assertTrue(member.mode & 0o100)

    def test_symbolic_links_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = package(root)
            (pkg / "link").symlink_to(pkg / "README.md")
            with self.assertRaises(SystemExit):
                RELEASE.write_archive(pkg, root / "c.tar.gz", "top", 0)

    def test_changelog_section_needs_a_release_date(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            saved = RELEASE.ROOT
            RELEASE.ROOT = root
            try:
                (root / "CHANGELOG.md").write_text(
                    "# Changelog\n\n## 0.2.0 — unreleased\n\nnext\n\n## 0.1.0 — 2026-10-06\n\nfirst\n")
                body, date = RELEASE.changelog_section("0.1.0")
                self.assertEqual((body, date), ("first\n", "2026-10-06"))
                with self.assertRaisesRegex(SystemExit, "unreleased"):
                    RELEASE.changelog_section("0.2.0")
                with self.assertRaisesRegex(SystemExit, "no section"):
                    RELEASE.changelog_section("0.3.0")
            finally:
                RELEASE.ROOT = saved


if __name__ == "__main__":
    unittest.main()
