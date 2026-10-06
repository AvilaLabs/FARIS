import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).parent
SCRIPT = HERE / "collect_dependency_notices.py"
ROOT = HERE.parent


def metadata(root: Path) -> Path:
    """Two workspace crates depending on one registry crate that ships a licence file."""
    crate = root / "dep-1.0.0"
    crate.mkdir()
    (crate / "LICENSE").write_text("dep licence text\n")
    packages = [
        {"id": "cli", "name": "faris-cli", "version": "0.1.0", "source": None, "license": None,
         "license_file": None, "manifest_path": str(root / "cli/Cargo.toml")},
        {"id": "app", "name": "faris-app", "version": "0.1.0", "source": None, "license": None,
         "license_file": None, "manifest_path": str(root / "app/Cargo.toml")},
        {"id": "dep", "name": "dep", "version": "1.0.0", "source": "registry+crates", "license": "MIT",
         "license_file": None, "manifest_path": str(crate / "Cargo.toml")},
    ]
    nodes = [{"id": "cli", "deps": [{"pkg": "dep"}]}, {"id": "app", "deps": [{"pkg": "dep"}]},
             {"id": "dep", "deps": []}]
    path = root / "metadata.json"
    path.write_text(json.dumps({"packages": packages, "resolve": {"nodes": nodes}}))
    return path


def run(root: Path, name: str, *extra: str) -> str:
    output = root / name
    subprocess.run([sys.executable, str(SCRIPT), "--metadata", str(metadata_path(root)),
                    "--output", str(output), *extra], check=True, capture_output=True)
    return output.read_text()


def metadata_path(root: Path) -> Path:
    return root / "metadata.json"


class CollectNoticesTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        metadata(self.root)

    def tearDown(self):
        self.temporary.cleanup()

    def test_default_output_keeps_the_linux_wording_of_the_checked_in_notices(self):
        text = run(self.root, "default.md")
        committed = (ROOT / "docs/THIRD_PARTY_NOTICES.md").read_text().splitlines()
        # Everything between the title and the dependency count is the header.
        count = next(i for i, line in enumerate(committed) if line.endswith("exact license texts."))
        expected = "\n".join(committed[1:count]) + "\n"
        self.assertTrue(text.startswith("# FARIS native dependency notices\n" + expected))
        self.assertEqual(text, run(self.root, "explicit.md", "--platform-label", "Linux",
                                   "--target-triple", "x86_64-unknown-linux-gnu"))

    def test_platform_options_change_only_the_header_wording(self):
        linux = run(self.root, "linux.md")
        windows = run(self.root, "windows.md", "--platform-label", "Windows",
                      "--target-triple", "x86_64-pc-windows-msvc")
        self.assertIn("locked Windows Cargo dependency graph", windows)
        self.assertIn("--filter-platform x86_64-pc-windows-msvc`", windows)
        self.assertNotIn("Linux", windows)
        self.assertNotIn("linux", windows)
        self.assertEqual(linux.replace("Linux", "Windows").replace("x86_64-unknown-linux-gnu",
                                                                   "x86_64-pc-windows-msvc"), windows)


class DeclaredOnlyTests(unittest.TestCase):
    """A crate with no license file in its package and no upstream supplement."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        metadata(self.root)
        (self.root / "dep-1.0.0/LICENSE").unlink()
        self.supplements = self.root / "supplements"
        self.supplements.mkdir()
        (self.supplements / "manifest.json").write_text("[]")
        self.standard = self.root / "standard"
        self.standard.mkdir()
        self.mit = b"standard MIT text\n"
        (self.standard / "MIT.txt").write_bytes(self.mit)
        self.write_standard()
        self.write_declared({"dep 1.0.0": {"ids": ["MIT"], "reason": "upstream publishes no licence file"}})

    def tearDown(self):
        self.temporary.cleanup()

    def write_standard(self, sha=None):
        sha = sha or hashlib.sha256(self.mit).hexdigest()
        (self.standard / "manifest.json").write_text(json.dumps(
            {"tag": "v9.9", "licenses": {"MIT": {"file": "MIT.txt", "sha256": sha}}}))

    def write_declared(self, declared):
        (self.standard / "declared-only.json").write_text(json.dumps(declared))

    def collect(self):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--metadata", str(metadata_path(self.root)),
             "--supplements", str(self.supplements), "--standard", str(self.standard),
             "--output", str(self.root / "out.md")], capture_output=True, text=True)

    def test_declared_only_crate_gets_the_standard_text_with_its_label(self):
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        text = (self.root / "out.md").read_text()
        self.assertIn("standard MIT text (SPDX license-list-data v9.9); "
                      "upstream publishes no licence file: [SHA-256 ", text)
        self.assertIn("standard MIT text\n", text)

    def test_declared_ids_must_equal_the_crates_license_field(self):
        self.write_declared({"dep 1.0.0": {"ids": ["Zlib"], "reason": "x"}})
        result = self.collect()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("differ from license", result.stderr)

    def test_unlisted_crate_without_text_still_fails(self):
        self.write_declared({})
        result = self.collect()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no license text for dep 1.0.0", result.stderr)

    def test_changed_standard_text_fails(self):
        (self.standard / "MIT.txt").write_bytes(self.mit + b"tampered\n")
        result = self.collect()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SPDX license text changed", result.stderr)

    def test_spdx_expression_parsing(self):
        spec = importlib.util.spec_from_file_location("collect", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertEqual(module.spdx_ids("Zlib OR (Apache-2.0 AND MIT)"), {"Zlib", "Apache-2.0", "MIT"})
        self.assertEqual(module.spdx_ids("MIT/Apache-2.0"), {"MIT", "Apache-2.0"})


if __name__ == "__main__":
    unittest.main()
