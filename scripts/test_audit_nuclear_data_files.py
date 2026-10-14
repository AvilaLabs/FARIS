import importlib.util
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("audit_nuclear_data_files", HERE / "audit_nuclear_data_files.py")
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)

HDF5 = AUDIT.HDF5_SIGNATURE + b"rest of a library file"


def git_repo(files):
    temp = tempfile.TemporaryDirectory()
    root = Path(temp.name)
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    for name, content in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    subprocess.run(["git", "-C", str(root), "add", "-A"], check=True)
    return temp, root


class NuclearDataAudit(unittest.TestCase):
    # Verifies: NUC-069
    def test_the_repository_tracks_no_data_file_beyond_the_documented_exceptions(self):
        findings, allowed = AUDIT.audit_git(AUDIT.REPO)
        self.assertEqual(findings, [])
        for line in allowed:
            self.assertIn(line.split(":")[0], AUDIT.ALLOWED_IN_GIT)

    # Verifies: NUC-069
    def test_a_tracked_library_file_is_found_by_name_and_by_signature(self):
        temp, root = git_repo({
            "src/main.rs": b"fn main() {}\n",
            "data/Fe56.h5": HDF5,
            "data/renamed.bin": HDF5,
            "data/cross_sections.xml": b"<cross_sections/>",
            "notes.txt": b"short",
        })
        with temp:
            findings, allowed = AUDIT.audit_git(root)
        self.assertEqual(allowed, [])
        text = "\n".join(findings)
        for name in ("data/Fe56.h5", "data/renamed.bin", "data/cross_sections.xml"):
            self.assertIn(name, text)
        self.assertNotIn("src/main.rs", text)
        self.assertNotIn("notes.txt", text)

    def test_a_clean_repository_has_no_findings(self):
        temp, root = git_repo({"README.md": b"text\n", "src/lib.rs": b"\n"})
        with temp:
            self.assertEqual(AUDIT.audit_git(root), ([], []))

    def make_package(self, directory, members, tar_members=None):
        path = Path(directory) / "study.faris"
        with zipfile.ZipFile(path, "w") as archive:
            for name, content in members.items():
                archive.writestr(name, content)
            if tar_members is not None:
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode="w") as tar:
                    for name, content in tar_members.items():
                        info = tarfile.TarInfo(name)
                        info.size = len(content)
                        tar.addfile(info, io.BytesIO(content))
                archive.writestr("evidence/case.tar", buffer.getvalue())
        return path

    # Verifies: NUC-069
    def test_a_study_package_without_data_passes_and_one_with_data_is_found(self):
        with tempfile.TemporaryDirectory() as temp:
            clean = self.make_package(temp, {"manifest.json": b"{}", "transport/run.json": b"{}"}, {"report.json": b"{}"})
            self.assertEqual(AUDIT.audit_package(clean), [])
        with tempfile.TemporaryDirectory() as temp:
            by_name = self.make_package(temp, {"manifest.json": b"{}", "data/Fe56.h5": b"x"})
            self.assertTrue(any("Fe56.h5" in f for f in AUDIT.audit_package(by_name)))
        with tempfile.TemporaryDirectory() as temp:
            by_signature = self.make_package(temp, {"blob": HDF5})
            self.assertTrue(any("blob" in f for f in AUDIT.audit_package(by_signature)))
        with tempfile.TemporaryDirectory() as temp:
            nested = self.make_package(temp, {"manifest.json": b"{}"}, {"inputs/H1.h5": HDF5})
            self.assertTrue(any("inputs/H1.h5" in f for f in AUDIT.audit_package(nested)))


if __name__ == "__main__":
    unittest.main()
