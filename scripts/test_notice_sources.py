# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Where each licence text in the committed third-party notices comes from (LEG-014)."""
import hashlib
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM = ROOT / "licenses/upstream-rust"
ITEM = re.compile(r"^(.*): \[SHA-256 ([0-9a-f]{12})\]\(#text-([0-9a-f]{64})\)$")


def rows(path: Path):
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("| ") and "SHA-256" in line:
            cells = [c.strip() for c in line.strip().strip("|").split(" | ")]
            yield cells[0], cells[3].split("<br>")


class NoticeSourceTests(unittest.TestCase):
    def check(self, notices: str):
        manifest = {r["url"]: r for r in json.loads((UPSTREAM / "manifest.json").read_text(encoding="utf-8"))}
        exceptions = json.loads((UPSTREAM / "exceptions.json").read_text(encoding="utf-8"))["packages"]
        fetched, total = set(), 0
        for package, items in rows(ROOT / "docs" / notices):
            total += 1
            self.assertTrue(items and all(items), f"{package}: no licence text")
            for item in items:
                match = ITEM.match(item)
                self.assertIsNotNone(match, f"{package}: entry does not state source and SHA-256: {item}")
                source, short, full = match.groups()
                self.assertTrue(full.startswith(short), f"{package}: {item}")
                if source.startswith("https://"):
                    fetched.add(package)
                    record = manifest.get(source)
                    self.assertIsNotNone(record, f"{package}: {source} is not a pinned upstream text")
                    self.assertEqual(record["sha256"], full, f"{package}: {source}")
                    data = (UPSTREAM / record["file"]).read_bytes()
                    self.assertEqual(hashlib.sha256(data).hexdigest(), full, f"{package}: stored text changed")
        return fetched, exceptions, total

    def test_fetched_texts_are_marked_pinned_and_hash_checked_and_each_package_is_an_accepted_exception(self):
        # Verifies: LEG-014
        fetched, exceptions, total = self.check("THIRD_PARTY_NOTICES.md")
        self.assertGreater(total, 250)
        self.assertEqual(fetched, set(exceptions), "URL-sourced entries differ from licenses/upstream-rust/exceptions.json")
        self.assertTrue(all(reason for reason in exceptions.values()))

    def test_the_core_runtime_notices_state_their_source_and_hash_for_every_text(self):
        # Verifies: LEG-014
        fetched, exceptions, total = self.check("CORE_RUNTIME_DEPENDENCY_NOTICES.md")
        self.assertGreater(total, 50)
        self.assertLessEqual(fetched, set(exceptions))


if __name__ == "__main__":
    unittest.main()
