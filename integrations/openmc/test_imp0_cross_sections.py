#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure tests for bounded cross-section index preflight."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "integrations/openmc/risk_tests"))
from imp0_cross_sections import MAX_INDEX_BYTES, validate_cross_sections_index  # noqa: E402


class CrossSectionIndexValidation(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.data = self.root / "data"
        self.data.mkdir()
        for name in ("C12.h5", "N14.h5", "O16.h5", "Pb204.h5", "H.h5", "Fe.h5"):
            (self.data / name).write_bytes(b"test-placeholder")
        self.index = self.root / "cross_sections.xml"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write_index(self, *, photon: bool = True, bad_path: bool = False) -> None:
        entries = [
            '<library materials="C12 N14 O16 Pb204" path="C12.h5" type="neutron"/>',
            '<library materials="H" path="H.h5" type="neutron"/>',
            '<library materials="Fe" path="Fe.h5" type="photon"/>' if photon else "",
            '<library materials="lwtr" path="missing.h5" type="thermal"/>' if bad_path else "",
        ]
        self.index.write_text(
            '<cross_sections><directory>data</directory>' + "".join(entries) + '</cross_sections>',
            encoding="utf-8",
        )

    def test_accepts_broader_fendl_style_catalogue_and_relative_directory(self) -> None:
        self.write_index()
        identity = validate_cross_sections_index(self.index)
        self.assertEqual(identity["catalogue_counts_by_type"], {"neutron": 5, "photon": 1})
        self.assertEqual(identity["library_entry_count"], 3)
        self.assertEqual(identity["resolved_library_paths"], sorted([
            str((self.data / "C12.h5").resolve()),
            str((self.data / "H.h5").resolve()),
            str((self.data / "Fe.h5").resolve()),
        ]))
        self.assertEqual(len(identity["sha256"]), 64)

    def test_refuses_missing_required_catalogue(self) -> None:
        self.write_index(photon=False)
        with self.assertRaisesRegex(ValueError, "no photon catalogue"):
            validate_cross_sections_index(self.index)

    def test_refuses_unresolved_library_path(self) -> None:
        self.write_index(bad_path=True)
        with self.assertRaisesRegex(ValueError, "does not exist"):
            validate_cross_sections_index(self.index)

    def test_refuses_oversized_or_non_xml_regular_input(self) -> None:
        self.index.write_bytes(b" " * (MAX_INDEX_BYTES + 1))
        with self.assertRaisesRegex(ValueError, "outside the allowed bound"):
            validate_cross_sections_index(self.index)
        self.index.write_text("<not-cross-sections/>", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected root"):
            validate_cross_sections_index(self.index)

    def test_hash_tracks_index_bytes(self) -> None:
        self.write_index()
        before = validate_cross_sections_index(self.index)["sha256"]
        self.index.write_text(self.index.read_text() + "\n", encoding="utf-8")
        after = validate_cross_sections_index(self.index)["sha256"]
        self.assertNotEqual(before, after)


if __name__ == "__main__":
    unittest.main()
