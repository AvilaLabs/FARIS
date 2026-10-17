#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""INT-002: the shell-only extraction script recovers the manifest and every blob of the archived study files."""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

REPO = Path(__file__).resolve().parent.parent
SCRIPT = REPO / "scripts" / "shell_extract_study.sh"
FIXTURES = sorted((REPO / "crates" / "faris-study" / "tests" / "fixtures").glob("*.faris"))
TOOLS = ("unzip", "zipinfo", "zstd", "jq", "sha256sum", "dd", "od", "awk")
MISSING = [t for t in TOOLS if shutil.which(t) is None]
TYPE = b"application/vnd.avila-labs.faris-study"


def run(file: Path, out: Path) -> subprocess.CompletedProcess:
    return subprocess.run(["bash", str(SCRIPT), str(file), str(out)], capture_output=True, text=True, timeout=120)


class ShellExtractTests(unittest.TestCase):
    def setUp(self):
        if MISSING and not os.environ.get("CI"):
            self.skipTest(f"needs {', '.join(MISSING)}")
        self.assertEqual(MISSING, [], "CI must have the standard tools")
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_there_are_at_least_five_archived_fixtures(self):
        # Verifies: INT-002
        self.assertGreaterEqual(len(FIXTURES), 5)

    def test_every_archived_study_file_is_recovered_with_standard_tools_only(self):
        # Verifies: INT-002
        total_blobs = 0
        for fixture in FIXTURES:
            out = self.tmp / fixture.stem
            result = run(fixture, out)
            self.assertEqual(result.returncode, 0, f"{fixture.name}: {result.stderr}")
            self.assertEqual((out / "mimetype").read_bytes(), TYPE)
            manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
            with zipfile.ZipFile(fixture) as z:
                self.assertEqual(z.infolist()[0].filename, "mimetype")
                self.assertEqual(z.infolist()[0].compress_type, zipfile.ZIP_STORED)
                blob_entries = [n for n in z.namelist() if n.startswith("blobs/")]
            recovered = sorted(p.name for p in (out / "blobs").iterdir())
            self.assertEqual(recovered, sorted(n.split("/", 1)[1] for n in blob_entries), fixture.name)
            self.assertEqual(sorted(b["sha256"] for b in manifest.get("blobs", [])), recovered, fixture.name)
            for blob in (out / "blobs").iterdir():
                self.assertEqual(hashlib.sha256(blob.read_bytes()).hexdigest(), blob.name)
            total_blobs += len(recovered)
        self.assertGreater(total_blobs, 0, "no fixture held a blob")

    def test_a_file_whose_first_entry_is_not_mimetype_is_refused(self):
        # Verifies: INT-002
        bad = self.tmp / "bad.faris"
        with zipfile.ZipFile(bad, "w") as z:
            z.writestr("manifest.json", '{"format": "faris-study/1"}')
            z.writestr("mimetype", TYPE)
        result = run(bad, self.tmp / "out")
        self.assertEqual(result.returncode, 1)
        self.assertIn("first entry", result.stderr)

    def test_a_blob_that_does_not_hash_to_its_name_is_refused(self):
        # Verifies: INT-002
        bad = self.tmp / "bad.faris"
        name = "blobs/" + "0" * 64
        with zipfile.ZipFile(bad, "w") as z:
            z.writestr("mimetype", TYPE)
            z.writestr("manifest.json", '{"format": "faris-study/1", "blobs": []}')
            z.writestr(name, b"not zero")
        result = run(bad, self.tmp / "out")
        self.assertEqual(result.returncode, 1)
        self.assertIn("do not hash to the name", result.stderr)

    def test_a_blob_missing_from_the_manifest_table_is_refused(self):
        # Verifies: INT-002
        bad = self.tmp / "bad.faris"
        data = b"x"
        with zipfile.ZipFile(bad, "w") as z:
            z.writestr("mimetype", TYPE)
            z.writestr("manifest.json", '{"format": "faris-study/1", "blobs": []}')
            z.writestr("blobs/" + hashlib.sha256(data).hexdigest(), data)
        result = run(bad, self.tmp / "out")
        self.assertEqual(result.returncode, 1)
        self.assertIn("lists 0 blobs", result.stderr)

    def test_ci_runs_this_test_where_the_tools_exist(self):
        # Verifies: INT-002
        ci = (REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("unittest discover -s scripts", ci)


if __name__ == "__main__":
    unittest.main()
