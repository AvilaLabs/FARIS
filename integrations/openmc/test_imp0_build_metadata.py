#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure checks for the explicit OpenMC archival-version fallback."""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "integrations/openmc/risk_tests"))
import build_mcnp_imp0_runtime as BUILDER  # noqa: E402


class ArchivalVersionMetadata(unittest.TestCase):
    def test_uses_verified_base_commit_and_patch_variant_without_fake_dev_suffix(self) -> None:
        rendered = BUILDER.render_archival_version_metadata(BUILDER.SOURCE_HEAD)
        self.assertIn(f"commit: {BUILDER.SOURCE_HEAD}\n", rendered)
        self.assertIn("describe-name: v0.15.3-faris-aim-imp0\n", rendered)
        self.assertNotIn("$Format:", rendered)
        name = re.search(r"^describe-name: (.+)$", rendered, re.MULTILINE).group(1)
        match = re.match(r"^v?(\d+\.\d+\.\d+)", name)
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), "0.15.3")
        self.assertNotRegex(name, r"-\d+-g[0-9a-f]+")

    def test_version_header_must_match_source_and_patch_variant(self) -> None:
        header = f'''constexpr int VERSION_MAJOR {{0}};
constexpr int VERSION_MINOR {{15}};
constexpr int VERSION_RELEASE {{3}};
constexpr bool VERSION_DEV {{false}};
constexpr const char* VERSION_COMMIT_COUNT = "";
constexpr const char* VERSION_COMMIT_HASH = "{BUILDER.SOURCE_HEAD}";
'''
        result = BUILDER.verify_generated_version_header(header, BUILDER.SOURCE_HEAD)
        self.assertEqual(result["release"], 3)
        self.assertEqual(result["commit_hash"], BUILDER.SOURCE_HEAD)
        with self.assertRaisesRegex(RuntimeError, "not 0.15.3"):
            BUILDER.verify_generated_version_header(header.replace("VERSION_RELEASE {3}", "VERSION_RELEASE {4}"), BUILDER.SOURCE_HEAD)
        with self.assertRaisesRegex(RuntimeError, "commit differs"):
            BUILDER.verify_generated_version_header(header, "0" * 40)

    def test_rejects_non_full_commit_identity(self) -> None:
        for invalid in ("27e38e894", "g" * 40, "27e38e894697bb32a1dac7848d2618818b6b8daf\n"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                BUILDER.render_archival_version_metadata(invalid)


if __name__ == "__main__":
    unittest.main()
