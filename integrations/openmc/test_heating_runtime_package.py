#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Regression checks for runtime archive integrity and preserved notices."""
from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / "risk_tests"))
import build_heating_runtime as builder
import verify_heating_runtime as verifier


class HeatingRuntimePackage(unittest.TestCase):
    def runtime(self, root: Path) -> Path:
        prefix = root / "runtime"
        (prefix / "lib").mkdir(parents=True)
        (prefix / "lib/libopenmc.so").write_bytes(b"fixture library")
        (prefix / "LICENSE-OpenMC-MIT").write_text("fixture notice")
        (prefix / "faris-openmc-0.15.3-compensated-heating.patch").write_text("fixture patch")
        (prefix / "runtime-manifest.json").write_text('{}\n')
        rows = builder.manifest(prefix)
        files = {"schema": verifier.FILES_SCHEMA, "files": rows, "sha256": builder.digest_json(rows)}
        (prefix / "runtime-files.sha256.json").write_text(json.dumps(files))
        return prefix

    def test_archive_bytes_ignore_source_mtimes_and_host_ownership(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            prefix = self.runtime(root)
            first, second = root / "first.tar.gz", root / "second.tar.gz"
            builder.write_package(prefix, first)
            os.utime(prefix / "lib/libopenmc.so", (123456, 123456))
            builder.write_package(prefix, second)
            self.assertEqual(builder.sha256_file(first), builder.sha256_file(second))
            with tarfile.open(first) as archive:
                for member in archive.getmembers():
                    self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))

    def test_extracted_inventory_catches_corruption_and_extra_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            prefix = self.runtime(root)
            archive = root / "runtime.tar.gz"
            builder.write_package(prefix, archive)
            extracted = verifier.extract_safe(archive, root / "extracted")
            verifier.verify_files(extracted)
            library = extracted / "lib/libopenmc.so"
            library.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "hash/type mismatch"):
                verifier.verify_files(extracted)
            library.write_bytes(b"fixture library")
            (extracted / "unlisted.txt").write_text("extra")
            with self.assertRaisesRegex(ValueError, "file inventory differs"):
                verifier.verify_files(extracted)
            (extracted / "unlisted.txt").unlink()
            nested = extracted / "nested/runtime-files.sha256.json"
            nested.parent.mkdir()
            nested.write_text("unlisted nested manifest")
            with self.assertRaisesRegex(ValueError, "file inventory differs"):
                verifier.verify_files(extracted)

    def test_extract_rejects_traversal_and_members_below_symlinks(self):
        for variant in ("traversal", "symlink-parent"):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                archive = root / "unsafe.tar"
                with tarfile.open(archive, "w") as output:
                    if variant == "traversal":
                        output.addfile(tarfile.TarInfo("faris-openmc-runtime/../escape"))
                    else:
                        link = tarfile.TarInfo("faris-openmc-runtime/link")
                        link.type = tarfile.SYMTYPE
                        link.linkname = "lib"
                        output.addfile(link)
                        output.addfile(tarfile.TarInfo("faris-openmc-runtime/link/payload"))
                with self.assertRaises(ValueError):
                    verifier.extract_safe(archive, root / "extracted")
                self.assertFalse((root / "escape").exists())

    def test_staging_preserves_each_pinned_vendor_notice(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            prefix, source = root / "runtime", root / "source"
            prefix.mkdir()
            (prefix / "LICENSE-OpenMC-MIT").write_text("upstream OpenMC notice")
            names = {"Catch2", "fmt", "pugixml", "xtensor", "xtl"}
            self.assertEqual(set(builder.VENDOR_LICENSES), names)
            for name, filename in builder.VENDOR_LICENSES.items():
                path = source / "vendor" / name / filename
                path.parent.mkdir(parents=True)
                path.write_text(f"notice for {name}")
            notices = builder.stage_licenses(source, prefix)
            self.assertEqual(len(notices), 7)
            for relative, digest in notices.items():
                self.assertEqual(builder.sha256_file(prefix / relative), digest)


if __name__ == "__main__":
    unittest.main()
