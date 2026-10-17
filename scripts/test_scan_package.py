#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
import hashlib
import io
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import scan_package as sp  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = (ROOT / ".github" / "workflows" / "desktop.yml").read_text(encoding="utf-8")
RELEASE = (ROOT / "scripts" / "make_release.py").read_text(encoding="utf-8")


def put(folder: Path, name: str, data: bytes = b"x") -> None:
    path = folder / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class PackageScanTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def test_a_clean_package_passes(self):
        # Verifies: SEC-072, LEG-016
        for name in ("bin/faris", "bin/faris-app", "licenses/notices.md", "scripts/actinv.py", "README.md"):
            put(self.root, name)
        self.assertEqual(sp.scan(self.root), [])

    def test_a_restricted_tool_program_is_found_by_name(self):
        # Verifies: SEC-072, LEG-016
        for name in ("bin/openmc", "bin/avila-core", "tools/ACTINV.exe", "x/mcnp6"):
            put(self.root, name)
        problems = sp.scan(self.root)
        self.assertEqual(len(problems), 4, problems)
        self.assertTrue(all("restricted or separately licensed tool" in p for p in problems))

    def test_nuclear_data_is_refused_unless_it_is_in_the_manifest_by_hash(self):
        # Verifies: SEC-072
        listed = b"a small library the provider lets us ship"
        put(self.root, "data/H1.h5", listed)
        put(self.root, "data/Fe56.h5", b"not listed")
        put(self.root, "data/cross_sections.xml", b"<index/>")
        put(self.root, "data/n.ace", b"ace")
        allowed = {hashlib.sha256(listed).hexdigest(): "integrations/windows-spike/data/H1.h5"}
        problems = sp.scan(self.root, allowed)
        self.assertEqual(len(problems), 3, problems)
        self.assertFalse(any("H1.h5" in p for p in problems))
        self.assertTrue(any("Fe56.h5" in p and "not in the data manifest" in p for p in problems))
        self.assertTrue(any("cross_sections.xml" in p for p in problems))

    def test_the_repository_manifest_lists_its_data_file_with_every_field(self):
        # Verifies: SEC-072
        digests = sp.manifest_digests()
        self.assertEqual(list(digests.values()), ["integrations/windows-spike/data/H1.h5"])
        spike = ROOT / "integrations" / "windows-spike" / "data"
        problems = sp.scan(spike)
        self.assertFalse(any("H1.h5" in p for p in problems), problems)
        self.assertTrue(any("cross_sections.xml" in p for p in problems), problems)

    def test_archives_are_scanned_like_folders(self):
        # Verifies: SEC-072, LEG-016
        archive = self.root / "pkg.zip"
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("FARIS/bin/openmc", "x")
            z.writestr("FARIS/bin/faris", "x")
        self.assertEqual(len(sp.scan(archive)), 1)
        tarball = self.root / "pkg.tar.gz"
        with tarfile.open(tarball, "w:gz") as t:
            for name in ("FARIS/data/Fe56.h5", "FARIS/bin/faris"):
                info = tarfile.TarInfo(name)
                info.size = 1
                t.addfile(info, io.BytesIO(b"x"))
        problems = sp.scan(tarball)
        self.assertEqual(len(problems), 1)
        self.assertIn("Fe56.h5", problems[0])

    def test_a_missing_target_fails_and_the_command_exit_status_follows_the_findings(self):
        # Verifies: SEC-072
        self.assertEqual(sp.scan(self.root / "nothing"), [f"{self.root / 'nothing'} does not exist"])
        put(self.root, "bin/openmc")
        self.assertEqual(sp.main([str(self.root)]), 1)
        clean = self.root / "clean"
        put(clean, "bin/faris")
        self.assertEqual(sp.main([str(clean)]), 0)

    def test_the_release_build_and_make_release_run_the_scan(self):
        # Verifies: SEC-072, LEG-016
        self.assertIn("python3 scripts/scan_package.py out", DESKTOP)
        self.assertLess(DESKTOP.index("scan_package.py"), DESKTOP.index("actions/upload-artifact"))
        self.assertIn("scan_package.scan(package)", RELEASE)


if __name__ == "__main__":
    unittest.main()
