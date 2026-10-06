import importlib.util
import json
import os
import tarfile
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("make_release", Path(__file__).with_name("make_release.py"))
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


def package(root: Path) -> Path:
    pkg = root / "pkg"
    contents = {"bin/faris": ("#!/bin/sh\necho faris 0.1.0\n", 0o555), "bin/faris-app": ("app\n", 0o555),
                "README.md": ("package\n", 0o444), "control/scenario.json": ("{}\n", 0o444),
                "verify.sh": ("#!/bin/sh\n", 0o555), "scripts/verify.py": ("v\n", 0o444),
                "port/cases/reference.tar.gz": ("evidence\n", 0o444)}
    files = []
    for relative, (text, mode) in contents.items():
        (pkg / relative).parent.mkdir(parents=True, exist_ok=True)
        (pkg / relative).write_text(text)
        os.chmod(pkg / relative, mode)
        part = "app" if relative.split("/")[0] in {"bin", "control"} or relative == "README.md" else "evidence"
        files.append({"path": relative, "bytes": len(text), "sha256": "0" * 64, "part": part})
    index = {"schema_version": "faris-recorded-demo-package/v0.5", "files": files,
             "parts": {"evidence": {"archive_name": "FARIS-0.1.0-evidence.tar.gz"}},
             "local_runtime": {"platform": {"os": "linux", "arch": "x86_64"}}}
    (pkg / "package-index.json").write_text(json.dumps(index))
    (pkg / "package-index.sha256").write_text("sha256:x  package-index.json\n")
    return pkg


def tree(root: Path) -> dict:
    return {path.relative_to(root).as_posix(): (path.stat().st_mode & 0o7777,
                                                None if path.is_dir() else path.read_bytes())
            for path in sorted(root.rglob("*"))}


class MakeReleaseTests(unittest.TestCase):
    def test_same_package_gives_the_same_archive_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = package(root)
            app, evidence, name = RELEASE.split_package(pkg, "0.1.0")
            self.assertEqual(name, "linux-x86_64")
            a, b = root / "a.tar.gz", root / "b.tar.gz"
            RELEASE.write_archive(pkg, a, "FARIS-0.1.0", 1_790_000_000, app)
            os.utime(pkg / "README.md", (1, 1))  # a different mtime on disk must not matter
            RELEASE.write_archive(pkg, b, "FARIS-0.1.0", 1_790_000_000, app)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            self.assertEqual(a.read_bytes()[4:8], bytes(4))  # gzip header mtime 0
            with tarfile.open(a) as tar:
                names = tar.getnames()
                self.assertEqual(names, sorted(names))
                self.assertEqual(names[0], "FARIS-0.1.0")
                self.assertIn("FARIS-0.1.0/bin/faris", names)
                member = tar.getmember("FARIS-0.1.0/bin/faris")
                self.assertEqual((member.uid, member.gid, member.uname, member.gname, member.mtime),
                                 (0, 0, "", "", 1_790_000_000))
                self.assertEqual(member.mode, 0o555)

    def test_both_archives_rebuild_the_package_and_the_app_archive_has_no_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = package(root)
            app, evidence, _ = RELEASE.split_package(pkg, "0.1.0")
            app_archive, evidence_archive = root / "app.tar.gz", root / "evidence.tar.gz"
            RELEASE.write_archive(pkg, app_archive, "FARIS-0.1.0", 5, app)
            RELEASE.write_archive(pkg, evidence_archive, "FARIS-0.1.0", 5, evidence)
            with tarfile.open(app_archive) as tar:
                members = set(tar.getnames())
            self.assertFalse(members & {f"FARIS-0.1.0/{path}" for path in evidence})
            self.assertIn("FARIS-0.1.0/package-index.json", members)
            self.assertNotIn("FARIS-0.1.0/port", members)
            out = root / "out"
            out.mkdir()
            for archive in (app_archive, evidence_archive):
                with tarfile.open(archive) as tar:
                    tar.extractall(out, filter="fully_trusted")
            self.assertEqual(tree(out / "FARIS-0.1.0"), tree(pkg))

    def test_package_that_differs_from_its_index_or_names_another_archive_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            pkg = package(Path(directory))
            with self.assertRaisesRegex(SystemExit, "evidence.tar.gz"):
                RELEASE.split_package(pkg, "0.2.0")
            (pkg / "extra.txt").write_text("unindexed\n")
            with self.assertRaisesRegex(SystemExit, "differ from its index"):
                RELEASE.split_package(pkg, "0.1.0")

    def test_release_time_prefers_source_date_epoch(self):
        saved = os.environ.get("SOURCE_DATE_EPOCH")
        try:
            os.environ["SOURCE_DATE_EPOCH"] = "1790000000"
            self.assertEqual(RELEASE.release_mtime(), 1_790_000_000)
            os.environ["SOURCE_DATE_EPOCH"] = "soon"
            with self.assertRaises(SystemExit):
                RELEASE.release_mtime()
            del os.environ["SOURCE_DATE_EPOCH"]
            self.assertGreater(RELEASE.release_mtime(), 1_700_000_000)
        finally:
            if saved is None:
                os.environ.pop("SOURCE_DATE_EPOCH", None)
            else:
                os.environ["SOURCE_DATE_EPOCH"] = saved

    def test_symbolic_links_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = package(root)
            (pkg / "link").symlink_to(pkg / "README.md")
            with self.assertRaises(SystemExit):
                RELEASE.write_archive(pkg, root / "c.tar.gz", "top", 0, ["link"])

    def test_changelog_section_needs_a_release_date(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            saved = RELEASE.ROOT
            RELEASE.ROOT = root
            try:
                (root / "CHANGELOG.md").write_text(
                    "# Changelog\n\n## 0.2.0 — unreleased\n\nnext\n\n## 0.1.0 — 2026-10-06\n\nfirst\n")
                body, date = RELEASE.changelog_section("0.1.0")
                self.assertEqual((body, date), ("first\n", "2026-10-06"))
                with self.assertRaisesRegex(SystemExit, "unreleased"):
                    RELEASE.changelog_section("0.2.0")
                with self.assertRaisesRegex(SystemExit, "no section"):
                    RELEASE.changelog_section("0.3.0")
            finally:
                RELEASE.ROOT = saved


if __name__ == "__main__":
    unittest.main()
