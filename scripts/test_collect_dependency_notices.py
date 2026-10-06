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


if __name__ == "__main__":
    unittest.main()
