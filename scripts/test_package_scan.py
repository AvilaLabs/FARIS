# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the restricted-binary package scan."""
import tempfile
import unittest
from pathlib import Path

import package_scan


def put(root: Path, relative: str, data: bytes) -> None:
    (root / relative).parent.mkdir(parents=True, exist_ok=True)
    (root / relative).write_bytes(data)


PE = b"MZ" + bytes(0x3A) + (0x40).to_bytes(4, "little") + b"PE\0\0" + bytes(20)


class PackageScanTests(unittest.TestCase):
    # Verifies: LEG-016
    def test_listed_programs_and_text_files_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            put(root, "bin/faris", b"\x7fELF" + bytes(20))
            put(root, "bin/faris-app", b"\x7fELF" + bytes(20))
            put(root, "bin/avila-core.exe", PE)
            put(root, "bin/faris.exe", PE)
            put(root, "README.md", b"MZ is only text here\n")
            put(root, "tools/build_activation_inputs.py", b"print(1)\n")
            self.assertEqual(package_scan.scan(root), [])

    # Verifies: LEG-016
    def test_an_unlisted_native_binary_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            put(root, "bin/faris", b"\x7fELF" + bytes(20))
            put(root, "bin/helper", b"\x7fELF" + bytes(20))
            put(root, "lib/helper.dll", PE)
            put(root, "lib/helper.dylib", b"\xcf\xfa\xed\xfe" + bytes(20))
            found = package_scan.scan(root)
            self.assertEqual(len(found), 3)
            self.assertTrue(all("not a listed FARIS program" in f for f in found))

    # Verifies: LEG-016
    def test_restricted_tool_and_data_names_are_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("bin/openmc", "lib/libdagmc.txt", "data/Fe56.h5", "data/fendl32.xml", "geometry/model.h5m"):
                put(root, name, b"text\n")
            put(root, "docs/notes.txt", b"text\n")
            found = package_scan.scan(root)
            self.assertEqual(len(found), 5)
            self.assertTrue(all("never bundled" in f for f in found))

    # Verifies: LEG-016
    def test_the_release_gate_refuses_a_package_with_a_restricted_file(self):
        import json
        import make_release
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp) / "pkg"
            put(pkg, "bin/openmc", b"x")
            index = {"schema_version": "faris-recorded-demo-package/v0.7",
                     "files": [{"path": "bin/openmc", "part": "app"}, {"path": "e.txt", "part": "evidence"}],
                     "parts": {"evidence": {"archive_name": "FARIS-1.0.0-evidence.tar.gz"}},
                     "local_runtime": {"platform": {"os": "linux", "arch": "x86_64"}}}
            put(pkg, "package-index.json", json.dumps(index).encode())
            put(pkg, "package-index.sha256", b"x\n")
            put(pkg, "e.txt", b"e\n")
            with self.assertRaisesRegex(SystemExit, "restricted or unlisted binary"):
                make_release.split_package(pkg, "1.0.0")


if __name__ == "__main__":
    unittest.main()
