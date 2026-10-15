# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for the SBOM generator (SEC-030). They use a small synthetic cargo-metadata document."""
import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import generate_sbom as sbom  # noqa: E402

REGISTRY = sbom.REGISTRY


def fixture():
    def pkg(name, version, licence, source=REGISTRY, targets=()):
        return {"name": name, "version": version, "id": f"{name} {version}", "license": licence, "source": source,
                "authors": ["A Author"] if source else [], "description": f"{name} crate",
                "targets": [{"kind": [k], "name": name} for k in targets]}
    packages = [
        pkg("faris-cli", "0.2.0", "AGPL-3.0-only", None, ("bin",)),
        pkg("serde", "1.0.1", "MIT OR Apache-2.0"),
        pkg("old", "0.1.0", "MIT/Apache-2.0"),
    ]
    resolve = {"nodes": [
        {"id": "faris-cli 0.2.0", "deps": [{"pkg": "serde 1.0.1"}, {"pkg": "old 0.1.0"}]},
        {"id": "serde 1.0.1", "deps": []},
        {"id": "old 0.1.0", "deps": []},
    ]}
    return {"packages": packages, "workspace_members": ["faris-cli 0.2.0"], "resolve": resolve}


LOCK = '[[package]]\nname = "serde"\nversion = "1.0.1"\nchecksum = "' + "ab" * 32 + '"\n'


class SbomTests(unittest.TestCase):
    def build(self, meta=None, root=None):
        return sbom.generate(meta or fixture(), root or Path(tempfile.mkdtemp()), LOCK, "0.2.0", "2026-10-15T00:00:00Z")

    def test_both_documents_validate_and_name_every_package(self):
        # Verifies: SEC-030
        spdx, cdx, problems = self.build()
        self.assertEqual(problems, [])
        self.assertEqual(spdx["spdxVersion"], "SPDX-2.3")
        self.assertEqual(cdx["specVersion"], "1.5")
        self.assertEqual({p["name"] for p in spdx["packages"]}, {"faris-cli", "serde", "old"})
        names = {c["name"] for c in cdx["components"]}
        self.assertEqual(names, {"faris-cli", "serde", "old"})

    def test_checksums_licences_and_dependencies_are_carried(self):
        # Verifies: SEC-030
        spdx, cdx, _ = self.build()
        serde = next(p for p in spdx["packages"] if p["name"] == "serde")
        self.assertEqual(serde["checksums"][0]["checksumValue"], "ab" * 32)
        old = next(p for p in spdx["packages"] if p["name"] == "old")
        self.assertEqual(old["licenseDeclared"], "MIT OR Apache-2.0")  # "A/B" written as an SPDX OR
        edges = {(r["spdxElementId"], r["relatedSpdxElement"]) for r in spdx["relationships"] if r["relationshipType"] == "DEPENDS_ON"}
        self.assertIn(("SPDXRef-Package-faris-cli-0.2.0", "SPDXRef-Package-serde-1.0.1"), edges)
        dep = next(d for d in cdx["dependencies"] if d["ref"] == "faris-cli@0.2.0")
        self.assertEqual(sorted(dep["dependsOn"]), ["old@0.1.0", "serde@1.0.1"])
        self.assertTrue(any(c["bom-ref"] == "bin:faris-cli" for c in cdx["components"]))

    def test_workspace_crates_have_no_download_location_and_unknown_supplier_is_noassertion(self):
        # Verifies: SEC-030
        spdx, _, _ = self.build()
        cli = next(p for p in spdx["packages"] if p["name"] == "faris-cli")
        self.assertEqual(cli["downloadLocation"], "NOASSERTION")
        self.assertEqual(cli["supplier"], "NOASSERTION")

    def test_bundled_fonts_are_listed_with_their_hashes(self):
        # Verifies: SEC-030
        root = Path(tempfile.mkdtemp())
        fonts = root / sbom.FONT_DIR
        fonts.mkdir(parents=True)
        (fonts / "Hack-Regular.ttf").write_bytes(b"font bytes")
        spdx, cdx, problems = self.build(root=root)
        self.assertEqual(problems, [])
        font = next(p for p in spdx["packages"] if p["name"] == "Hack-Regular.ttf")
        self.assertEqual(font["checksums"][0]["checksumValue"], sbom.hashlib.sha256(b"font bytes").hexdigest())
        self.assertTrue(any(c["bom-ref"] == "font:Hack-Regular.ttf" for c in cdx["components"]))

    def test_the_repository_fonts_are_all_listed(self):
        # Verifies: SEC-030
        root = Path(__file__).resolve().parent.parent
        on_disk = {p.name for p in (root / sbom.FONT_DIR).glob("*.ttf")}
        self.assertTrue(on_disk)
        self.assertEqual(on_disk, set(sbom.FONT_LICENCES))

    def test_validation_catches_a_missing_identifier_and_a_missing_dependency_section(self):
        # Verifies: SEC-030
        spdx, cdx, _ = self.build()
        broken = copy.deepcopy(spdx)
        del broken["packages"][1]["externalRefs"]
        self.assertTrue(any("unique identifier" in p for p in sbom.validate_spdx(broken)))
        broken = copy.deepcopy(spdx)
        broken["relationships"] = []
        self.assertTrue(any("dependency relationships" in p for p in sbom.validate_spdx(broken)))
        broken = copy.deepcopy(cdx)
        broken["dependencies"] = []
        self.assertTrue(any("dependency relationships" in p for p in sbom.validate_cyclonedx(broken)))
        broken = copy.deepcopy(cdx)
        del broken["metadata"]["timestamp"]
        self.assertTrue(any("timestamp" in p for p in sbom.validate_cyclonedx(broken)))

    def test_output_is_reproducible_for_a_fixed_timestamp_and_main_writes_both_files(self):
        # Verifies: SEC-030
        a = self.build()[:2]
        b = self.build()[:2]
        self.assertEqual(json.dumps(a), json.dumps(b))
        with tempfile.TemporaryDirectory() as d:
            meta = Path(d) / "m.json"
            meta.write_text(json.dumps(fixture()))
            root = Path(d) / "root"
            root.mkdir()
            (root / "Cargo.lock").write_text(LOCK)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.2.0"\n')
            self.assertEqual(sbom.main(["--metadata", str(meta), "--out", str(Path(d) / "out"), "--root", str(root)]), 0)
            self.assertTrue((Path(d) / "out" / "faris.spdx.json").is_file())
            self.assertTrue((Path(d) / "out" / "faris.cdx.json").is_file())


if __name__ == "__main__":
    unittest.main()
