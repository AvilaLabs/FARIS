#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure checks for the explicit OpenMC archival-version fallback."""

from __future__ import annotations

import re
import sys
import unittest
from unittest.mock import patch
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

    def test_patch_digest_and_cell_declaration_include_are_pinned(self) -> None:
        self.assertEqual(
            BUILDER.sha256_file(BUILDER.IMP0_PATCH),
            BUILDER.IMP0_PATCH_SHA256,
        )
        patch = BUILDER.IMP0_PATCH.read_text(encoding="utf-8")
        self.assertIn('+#include "openmc/cell.h"', patch)

    def test_rejects_non_full_commit_identity(self) -> None:
        for invalid in ("27e38e894", "g" * 40, "27e38e894697bb32a1dac7848d2618818b6b8daf\n"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                BUILDER.render_archival_version_metadata(invalid)

    def test_capability_probe_requires_exact_identity_record(self) -> None:
        python = Path("/selected/openmc-python")
        library = Path("/isolated/libopenmc.so")
        digest = "a" * 64
        report = {
            "schema": "faris.openmc-imp0-capability-probe/1.0.0",
            "python_executable": str(python.resolve()),
            "openmc_version": "0.15.3",
            "library_path": str(library.resolve()),
            "library_sha256": digest,
            "patch_version": 1,
        }
        encoded = __import__("json").dumps(report, sort_keys=True, separators=(",", ":"))
        self.assertEqual(
            BUILDER.validate_capability_probe(
                encoded, python=python, library=library, library_sha256=digest
            ),
            report,
        )
        with self.assertRaisesRegex(RuntimeError, "invalid JSON"):
            BUILDER.validate_capability_probe(
                encoded + "\n{}", python=python, library=library, library_sha256=digest
            )
        report["library_path"] = "/other/libopenmc.so"
        encoded = __import__("json").dumps(report)
        with self.assertRaisesRegex(RuntimeError, "identity differs"):
            BUILDER.validate_capability_probe(
                encoded, python=python, library=library, library_sha256=digest
            )

    def test_capability_probe_runs_in_selected_python_with_runtime_environment(self) -> None:
        python = Path(sys.executable)
        library = Path(__file__).resolve()
        env = {"LD_LIBRARY_PATH": "/isolated/lib:/selected/runtime/lib"}
        digest = BUILDER.sha256_file(library)
        report = {
            "schema": "faris.openmc-imp0-capability-probe/1.0.0",
            "python_executable": str(python.resolve()),
            "openmc_version": "0.15.3",
            "library_path": str(library),
            "library_sha256": digest,
            "patch_version": 1,
        }
        completed = __import__("subprocess").CompletedProcess(
            [str(python)], 0,
            __import__("json").dumps(report, sort_keys=True, separators=(",", ":")),
            "",
        )
        with patch.object(BUILDER.subprocess, "run", return_value=completed) as mocked:
            result = BUILDER.probe_imp0_capability(python, library, env)
        self.assertEqual(result, report)
        argv = mocked.call_args.args[0]
        self.assertEqual(argv[0], str(python.resolve()))
        self.assertEqual(argv[1:3], ["-c", BUILDER.CAPABILITY_PROBE])
        self.assertEqual(mocked.call_args.kwargs["env"], env)


if __name__ == "__main__":
    unittest.main()
