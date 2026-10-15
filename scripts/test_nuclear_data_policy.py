# SPDX-License-Identifier: AGPL-3.0-only
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import nuclear_data_policy as POLICY

REPO = Path(__file__).resolve().parents[1]


def registry(directory: Path, entry: dict, limit: int = 1024) -> Path:
    path = directory / "registry.json"
    path.write_text(json.dumps({"policy": "test", "max_file_bytes": limit, "files": [entry]}), encoding="utf-8")
    return path


def complete(path: str, data: bytes) -> dict:
    entry = {"path": path, "kind": "data", "sha256": hashlib.sha256(data).hexdigest()}
    entry.update({field: "stated" for field in POLICY.DATA_FIELDS if field != "sha256"})
    return entry


class NuclearDataPolicyTests(unittest.TestCase):
    # Verifies: QA-090, NUC-069
    def test_the_tracked_files_meet_the_rule(self):
        self.assertEqual(POLICY.problems(REPO), [])

    # Verifies: QA-090, NUC-069
    def test_a_listed_file_is_refused_when_large_changed_or_without_provenance(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            data = b"\x89HDF\r\n\x1a\n" + bytes(100)
            (root / "H1.h5").write_bytes(data)
            self.assertEqual(POLICY.problems(root, registry(root, complete("H1.h5", data))), [])
            self.assertIn("over the 64 byte limit",
                          POLICY.problems(root, registry(root, complete("H1.h5", data), limit=64))[0])
            changed = complete("H1.h5", data + b"x")
            self.assertIn("differs from its recorded SHA-256", POLICY.problems(root, registry(root, changed))[0])
            for field in ("terms", "terms_evidence", "attribution", "provider", "source"):
                entry = complete("H1.h5", data)
                entry[field] = " "
                found = POLICY.problems(root, registry(root, entry))
                self.assertEqual(len(found), 1)
                self.assertIn(field, found[0])

    def test_a_listed_file_that_is_gone_is_reported(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            found = POLICY.problems(root, registry(root, complete("gone.h5", b"")))
            self.assertIn("missing", found[0])


if __name__ == "__main__":
    unittest.main()
