#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Focused safety tests for the disposable synthetic web smoke tree."""
from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "prepare_web_smoke", Path(__file__).with_name("prepare_web_smoke.py")
)
PREPARE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(PREPARE)


class PrepareWebSmokeTests(unittest.TestCase):
    def make_trees(self, root: Path) -> tuple[Path, Path, Path]:
        dist = root / "dist"
        fixture = root / "fixture"
        output = root / "smoke"
        (dist / "demo").mkdir(parents=True)
        (dist / "index.html").write_text("production page", encoding="utf-8")
        (dist / "demo/manifest.json").write_text("production demo", encoding="utf-8")
        fixture.mkdir()

        core = []
        paths = ["operating-assumptions.json"]
        paths += [
            f"{pair}/bundles/{variant}.transport-bundle.json"
            for pair in ("port", "control")
            for variant in ("reference", "breeder-emphasis")
        ]
        for relative in paths:
            path = fixture / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            payload = f"synthetic fixture: {relative}".encode()
            path.write_bytes(payload)
            core.append({
                "path": relative,
                "bytes": len(payload),
                "sha256": f"sha256:{hashlib.sha256(payload).hexdigest()}",
            })
        (fixture / "manifest.json").write_text(json.dumps({
            "schema": "faris-web-demo/v1",
            "fixture_kind": "synthetic-ci-smoke",
            "core": core,
            "sweep": [],
        }), encoding="utf-8")
        return dist, fixture, output

    def test_copies_dist_and_replaces_demo_only_in_isolated_output(self):
        with tempfile.TemporaryDirectory() as temp:
            dist, fixture, output = self.make_trees(Path(temp))
            production_page = (dist / "index.html").read_bytes()
            production_manifest = (dist / "demo/manifest.json").read_bytes()

            PREPARE.prepare(dist, fixture, output)

            self.assertEqual((output / "index.html").read_bytes(), production_page)
            self.assertEqual(
                (output / "demo/manifest.json").read_bytes(),
                (fixture / "manifest.json").read_bytes(),
            )
            self.assertEqual((dist / "index.html").read_bytes(), production_page)
            self.assertEqual((dist / "demo/manifest.json").read_bytes(), production_manifest)
            self.assertTrue((output / "demo/control/bundles/reference.transport-bundle.json").is_file())

    def test_rejects_same_size_fixture_tampering_even_with_safe_inventory(self):
        with tempfile.TemporaryDirectory() as temp:
            dist, fixture, output = self.make_trees(Path(temp))
            victim = fixture / "operating-assumptions.json"
            original = victim.read_bytes()
            victim.write_bytes(b"X" * len(original))

            with self.assertRaisesRegex(ValueError, "digest differs"):
                PREPARE.prepare(dist, fixture, output)
            self.assertFalse(output.exists())

    def test_refuses_output_inside_or_overlapping_production_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            dist, fixture, _ = self.make_trees(Path(temp))
            production_page = (dist / "index.html").read_bytes()
            unsafe = dist / "new-smoke-copy"

            with self.assertRaisesRegex(ValueError, "must be separate trees"):
                PREPARE.prepare(dist, fixture, unsafe)
            self.assertFalse(unsafe.exists())
            self.assertEqual((dist / "index.html").read_bytes(), production_page)


if __name__ == "__main__":
    unittest.main()
