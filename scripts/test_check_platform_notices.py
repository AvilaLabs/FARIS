# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for the per-platform notices comparison."""
import unittest

import check_platform_notices as chk

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def metadata(extra_dep=True):
    def pkg(name, version, source=REGISTRY):
        return {"id": f"{name} {version}", "name": name, "version": version, "source": source}
    packages = [pkg("faris-cli", "0.1.0", None), pkg("serde", "1.0.0"), pkg("winapi", "0.3.9"), pkg("unrelated", "1.0.0")]
    deps = [{"pkg": "serde 1.0.0"}] + ([{"pkg": "winapi 0.3.9"}] if extra_dep else [])
    nodes = [{"id": "faris-cli 0.1.0", "deps": deps}, {"id": "serde 1.0.0", "deps": []},
             {"id": "winapi 0.3.9", "deps": []}, {"id": "unrelated 1.0.0", "deps": []}]
    return {"packages": packages, "resolve": {"nodes": nodes}}


def notices(*rows, count=None):
    body = "\n".join(f"| {name} | MIT | [{name}](https://crates.io/x) | LICENSE-MIT: [SHA-256 abc](#t) |" for name in rows)
    return f"# Notices\n\n{count if count is not None else len(rows)} dependency versions; 1 distinct exact license texts.\n\n" \
           f"| Package | Declared license | Source | License texts |\n| --- | --- | --- | --- |\n{body}\n"


class PlatformNoticesTests(unittest.TestCase):
    # Verifies: LEG-013
    def test_the_graph_is_the_registry_packages_reachable_from_the_roots(self):
        self.assertEqual(chk.graph_packages(metadata(), {"faris-cli"}), {"serde 1.0.0", "winapi 0.3.9"})
        self.assertEqual(chk.graph_packages(metadata(False), {"faris-cli"}), {"serde 1.0.0"})
        with self.assertRaises(ValueError):
            chk.graph_packages(metadata(), {"not-a-crate"})

    # Verifies: LEG-013
    def test_matching_notices_have_no_difference(self):
        self.assertEqual(chk.compare(metadata(), {"faris-cli"}, notices("serde 1.0.0", "winapi 0.3.9")), [])

    # Verifies: LEG-013
    def test_a_missing_extra_or_stale_entry_is_reported(self):
        linux_only = chk.compare(metadata(), {"faris-cli"}, notices("serde 1.0.0"))
        self.assertIn("in the graph, not in the notices: winapi 0.3.9", linux_only)
        extra = chk.compare(metadata(False), {"faris-cli"}, notices("serde 1.0.0", "winapi 0.3.9"))
        self.assertIn("in the notices, not in the graph: winapi 0.3.9", extra)
        stale = chk.compare(metadata(), {"faris-cli"}, notices("serde 1.0.0", "winapi 0.3.9", count=7))
        self.assertTrue(any("header counts 7" in p for p in stale))

    # Verifies: LEG-013
    def test_an_entry_without_a_licence_text_is_reported(self):
        text = notices("serde 1.0.0", "winapi 0.3.9").replace("LICENSE-MIT: [SHA-256 abc](#t) |\n", " |\n", 1)
        self.assertIn("no licence text: serde 1.0.0", chk.compare(metadata(), {"faris-cli"}, text))


if __name__ == "__main__":
    unittest.main()
