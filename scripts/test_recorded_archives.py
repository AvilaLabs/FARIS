import gzip
import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from recorded_archives import (SCHEMA, MAX_TREE_DIRECTORIES, MAX_PATH_COMPONENTS,
                               create_archive, extract_archive, extract_indexed_trees,
                               count_implicit_directories, canonical_member_name)


def digest_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def crafted_archive(path: Path, members: list[tuple[str, bytes, bytes]]) -> dict:
    """members are (archive name, payload, tar type); manifest names are trusted test inputs."""
    records = []
    with tarfile.open(path, "w:gz", format=tarfile.USTAR_FORMAT) as archive:
        for name, payload, kind in members:
            info = tarfile.TarInfo(name)
            info.type = kind
            info.size = len(payload) if kind == tarfile.REGTYPE else 0
            import io
            archive.addfile(info, io.BytesIO(payload) if info.size else None)
            if kind == tarfile.REGTYPE and name not in {item["path"] for item in records}:
                records.append({"path": name, "bytes": len(payload), "sha256": digest_bytes(payload)})
    data = path.read_bytes()
    return {"schema_version": SCHEMA, "archive_sha256": digest_bytes(data),
            "archive_bytes": len(data), "expanded_bytes": sum(item["bytes"] for item in records),
            "file_count": len(records), "archive_member_count": len(members), "members": records}


class RecordedArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def test_round_trip_byte_identity_and_long_ustar_path(self):
        source = self.root / "source"
        relative = Path("nested") / ("n" * 90) / ("f" * 70 + ".json")
        content = (b'{"history":"same bytes"}\n' * 400)
        (source / relative).parent.mkdir(parents=True)
        (source / relative).write_bytes(content)
        archive, manifest = self.root / "case.tar.gz", self.root / "case.manifest.json"
        create_archive(source, archive, manifest)
        extracted = self.root / "extracted"
        extract_archive(archive, manifest, extracted)
        self.assertEqual((extracted / relative).read_bytes(), content)

    # Verifies: SEC-002
    def test_rejects_unsafe_and_noncanonical_paths_before_writing(self):
        for name in ("../escape", "/absolute", "a//b", "a/./b", "a/../b", "C:/drive", "a\\b"):
            with self.subTest(name=name):
                with self.assertRaises(ValueError):
                    canonical_member_name(name)
        with self.assertRaises(ValueError):
            canonical_member_name("/".join(["a"] * (MAX_PATH_COMPONENTS + 1)))
        deeply_nested = [{"path": f"d{index:04d}/file", "bytes": 0,
                          "sha256": digest_bytes(b"")}
                         for index in range(MAX_TREE_DIRECTORIES + 1)]
        with self.assertRaises(ValueError):
            count_implicit_directories(deeply_nested)
        archive = self.root / "bad.tar.gz"
        manifest = crafted_archive(archive, [("../escape", b"x", tarfile.REGTYPE)])
        manifest_path = self.root / "bad.json"
        manifest_path.write_text(json.dumps(manifest))
        target = self.root / "out"
        with self.assertRaises(ValueError):
            extract_archive(archive, manifest_path, target)
        self.assertFalse(target.exists())
        self.assertFalse((self.root.parent / "escape").exists())

    def test_rejects_duplicate_paths_and_file_prefix_conflicts_in_preflight(self):
        cases = (
            [("same", b"one", tarfile.REGTYPE), ("same", b"two", tarfile.REGTYPE)],
            [("a", b"file", tarfile.REGTYPE), ("a/b", b"child", tarfile.REGTYPE)],
        )
        for index, members in enumerate(cases):
            with self.subTest(index=index):
                archive = self.root / f"bad-{index}.tar.gz"
                manifest = crafted_archive(archive, members)
                manifest_path = self.root / f"bad-{index}.json"
                manifest_path.write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    extract_archive(archive, manifest_path, self.root / f"out-{index}")
                self.assertFalse((self.root / f"out-{index}").exists())

    # Verifies: PRV-044
    def test_rejects_links_special_files_and_extension_metadata(self):
        cases = (
            [("ok", b"valid", tarfile.REGTYPE), ("link", b"target", tarfile.SYMTYPE)],
            [("ok", b"valid", tarfile.REGTYPE), ("hardlink", b"target", tarfile.LNKTYPE)],
            [("ok", b"valid", tarfile.REGTYPE), ("fifo", b"", tarfile.FIFOTYPE)],
        )
        for index, members in enumerate(cases):
            with self.subTest(index=index):
                archive = self.root / f"special-{index}.tar.gz"
                manifest = crafted_archive(archive, members)
                manifest_path = self.root / f"special-{index}.json"
                manifest_path.write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    extract_archive(archive, manifest_path, self.root / f"special-out-{index}")

    def test_rejects_truncated_concatenated_and_trailing_gzip_data(self):
        source = self.root / "src"
        source.mkdir()
        (source / "x").write_bytes(b"x" * 100)
        original, original_manifest = self.root / "valid.tar.gz", self.root / "valid.json"
        create_archive(source, original, original_manifest)
        original_bytes = original.read_bytes()
        nonzero_tar_tail = gzip.compress(gzip.decompress(original_bytes) + b"X" * 512)
        for suffix, mutation in (("truncated", original_bytes[:-3]),
                                 ("concatenated", original_bytes + gzip.compress(b"extra")),
                                 ("trailing", original_bytes + b"trailing"),
                                 ("nonzero-tar-tail", nonzero_tar_tail)):
            with self.subTest(suffix=suffix):
                archive = self.root / f"{suffix}.tar.gz"
                archive.write_bytes(mutation)
                manifest = json.loads(original_manifest.read_text())
                manifest["archive_sha256"] = digest_bytes(mutation)
                manifest["archive_bytes"] = len(mutation)
                manifest_path = self.root / f"{suffix}.json"
                manifest_path.write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    extract_archive(archive, manifest_path, self.root / f"{suffix}-out")

    def test_tar_end_is_judged_from_member_end_not_from_unread_bytes(self):
        # tarfile consumes the first end-of-archive block itself, so a valid
        # archive ending in exactly two zero blocks once failed the check.
        import io
        payload = b"y" * 700
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            info = tarfile.TarInfo("x")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        member_end = 512 + 1024
        body = raw.getvalue()[:member_end]
        record = {"path": "x", "bytes": len(payload), "sha256": digest_bytes(payload)}
        cases = (("two-zero-blocks", b"\0" * 1024, True),
                 ("three-zero-blocks", b"\0" * 1536, True),
                 ("one-zero-block", b"\0" * 512, False),
                 ("nonzero-block-before-end", b"G" * 512 + b"\0" * 1024, False),
                 ("partial-block", b"\0" * 1100, False))
        for name, trailer, accepted in cases:
            with self.subTest(name=name):
                data = gzip.compress(body + trailer)
                archive_path = self.root / f"{name}.tar.gz"
                archive_path.write_bytes(data)
                manifest_path = self.root / f"{name}.json"
                manifest_path.write_text(json.dumps({
                    "schema_version": SCHEMA, "archive_sha256": digest_bytes(data),
                    "archive_bytes": len(data), "expanded_bytes": len(payload), "file_count": 1,
                    "archive_member_count": 1, "directory_count": 0, "members": [record]}))
                target = self.root / f"{name}-out"
                if accepted:
                    extract_archive(archive_path, manifest_path, target)
                    self.assertEqual((target / "x").read_bytes(), payload)
                else:
                    with self.assertRaises(ValueError):
                        extract_archive(archive_path, manifest_path, target)

    def test_refuses_source_mutated_during_archive_write(self):
        source = self.root / "source-mutation"
        source.mkdir()
        payload = source / "x"
        payload.write_bytes(b"original")
        archive, manifest = self.root / "mutation.tar.gz", self.root / "mutation.json"
        import recorded_archives
        original_addfile = tarfile.TarFile.addfile
        changed = False
        def mutate_then_add(tar, info, fileobj=None):
            nonlocal changed
            if not changed and info.name == "x":
                payload.write_bytes(b"mutated!")
                changed = True
            return original_addfile(tar, info, fileobj)
        tarfile.TarFile.addfile = mutate_then_add
        try:
            with self.assertRaisesRegex(ValueError, "changed while reading"):
                recorded_archives.create_archive(source, archive, manifest)
        finally:
            tarfile.TarFile.addfile = original_addfile
        self.assertFalse(archive.exists())

    # Verifies: SEC-002
    def test_index_pair_and_variant_ids_cannot_escape_extraction_root(self):
        package = self.root / "package"
        package.mkdir()
        for scenario_path, variant in (("../escape/scenario.json", "reference"),
                                       ("control/scenario.json", "../escape")):
            target = self.root / "extract" / str(len(list(self.root.glob("extract*"))))
            index = {"scenario_pairs": [{"scenario_path": scenario_path,
                                          "arrangements": [{"variant_id": variant}]}]}
            with self.subTest(scenario_path=scenario_path, variant=variant):
                with self.assertRaises(ValueError):
                    extract_indexed_trees(package, index, target)
                self.assertFalse((self.root / "escape").exists())


if __name__ == "__main__":
    unittest.main()
