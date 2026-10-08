import copy
import hashlib
import json
import lzma
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import evidence_store as store_module
from evidence_store import StoreError


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def blob(root: Path, data: bytes) -> Path:
    return root / "blobs" / sha(data)[:2] / (sha(data) + ".xz")


def write_tree(root: Path, files: dict[str, bytes]) -> Path:
    for relative, data in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    root.mkdir(parents=True, exist_ok=True)
    return root


def snapshot(root: Path) -> dict[str, bytes]:
    return {path.relative_to(root).as_posix(): path.read_bytes()
            for path in sorted(root.rglob("*")) if path.is_file()}


class StoreFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.case_files = {"package.json": b'{"case": 1}\n', "expected/history.json": b"h" * 5000,
                           "inputs/recorded.json": b"recorded\n" * 400}
        self.workspace_files = {"history/inputs/upstream.json": b"h" * 5000,
                                "history/receipt.json": b'{"r": 1}\n'}
        self.case = write_tree(self.root / "src-case", self.case_files)
        self.workspace = write_tree(self.root / "src-workspace", self.workspace_files)
        self.store = self.root / "store"
        self.report = store_module.pack_store(
            self.store, {"b-workspace": self.workspace, "a-case": self.case})

    def tearDown(self):
        self.temporary.cleanup()

    def index(self) -> dict:
        return json.loads((self.store / "store.json").read_text())

    def write_index(self, index: dict) -> None:
        (self.store / "store.json").write_text(json.dumps(index, indent=2) + "\n")

    def assert_refused(self, fragment: str):
        report = store_module.verify_store(self.store)
        self.assertEqual(report["status"], "failed", report)
        self.assertIn(fragment, json.dumps(report["findings"]))
        with self.assertRaises(StoreError):
            store_module.check_store(self.store)


class RoundTripTests(StoreFixture):
    def test_pack_verify_unpack_is_byte_identical(self):
        report = store_module.verify_store(self.store)
        self.assertEqual(report["status"], "verified", report)
        self.assertEqual(report["trees"], 2)
        self.assertEqual(report["files"], 5)
        out = self.root / "out"
        store_module.unpack_store(self.store, out)
        self.assertEqual(snapshot(out / "a-case"), self.case_files)
        self.assertEqual(snapshot(out / "b-workspace"), self.workspace_files)

    def test_identical_contents_share_one_blob_and_the_index_is_sorted(self):
        blobs = [p for p in (self.store / "blobs").rglob("*.xz")]
        self.assertEqual(len(blobs), 4)  # history bytes appear in both trees
        self.assertEqual(self.report["blob_count"], 4)
        self.assertEqual(self.report["files"], 5)
        self.assertLess(self.report["distinct_bytes"], self.report["uncompressed_bytes"])
        self.assertEqual([t["name"] for t in self.index()["trees"]], ["a-case", "b-workspace"])
        paths = [f["path"] for f in self.index()["trees"][0]["files"]]
        self.assertEqual(paths, sorted(paths))

    def test_tree_summary_counts_implicit_directories(self):
        summary = {t["name"]: t for t in store_module.tree_summary(self.index())}
        self.assertEqual(summary["a-case"]["file_count"], 3)
        self.assertEqual(summary["a-case"]["directory_count"], 2)
        self.assertEqual(summary["b-workspace"]["directory_count"], 2)
        self.assertEqual(summary["a-case"]["bytes"], sum(len(v) for v in self.case_files.values()))

    def test_one_file_reads_verified_and_one_tree_unpacks_alone(self):
        index = store_module.load_index(self.store)
        self.assertEqual(store_module.read_file(self.store, index, "a-case", "package.json"),
                         self.case_files["package.json"])
        with self.assertRaises(StoreError):
            store_module.read_file(self.store, index, "a-case", "nope.json")
        with self.assertRaises(StoreError):
            store_module.read_file(self.store, index, "no-tree", "package.json")
        out = self.root / "one"
        result = store_module.unpack_store(self.store, out, ["b-workspace"])
        self.assertEqual(result["trees"], 1)
        self.assertEqual([p.name for p in out.iterdir()], ["b-workspace"])

    def test_unpack_refuses_an_existing_target_and_unknown_trees(self):
        existing = self.root / "exists"
        existing.mkdir()
        with self.assertRaises(StoreError):
            store_module.unpack_store(self.store, existing)
        with self.assertRaises(StoreError):
            store_module.unpack_store(self.store, self.root / "new", ["missing"])
        self.assertFalse((self.root / "new").exists())

    def test_empty_file_round_trips(self):
        tree = write_tree(self.root / "src-empty", {"empty": b""})
        target = self.root / "empty-store"
        store_module.pack_store(target, {"e": tree})
        self.assertEqual(store_module.verify_store(target)["status"], "verified")

    def test_pack_refuses_symlinks_existing_output_and_bad_names(self):
        link = write_tree(self.root / "src-link", {"real": b"x"})
        os.symlink(link / "real", link / "alias")
        with self.assertRaisesRegex(StoreError, "symbolic link"):
            store_module.pack_store(self.root / "s1", {"t": link})
        self.assertFalse((self.root / "s1").exists())
        with self.assertRaises(StoreError):
            store_module.pack_store(self.store, {"t": self.case})
        for name in ("", ".", "..", "has space", "x" * 129):
            with self.assertRaises(StoreError, msg=name):
                store_module.pack_store(self.root / "s2", {name: self.case})

    def test_pack_refuses_backslash_names(self):
        tree = self.root / "src-bad"
        tree.mkdir()
        (tree / "a\\b").write_bytes(b"x")
        with self.assertRaises(StoreError):
            store_module.pack_store(self.root / "s3", {"t": tree})

    def test_cli_verify_and_unpack(self):
        script = str(Path(store_module.__file__))
        verified = subprocess.run([sys.executable, script, "verify", str(self.store)],
                                  capture_output=True, text=True)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        failed = subprocess.run([sys.executable, script, "unpack", str(self.store), str(self.root)],
                                capture_output=True, text=True)
        self.assertEqual(failed.returncode, 2)


class BlobRefusalTests(StoreFixture):
    # Verifies: SEC-002
    def test_a_flipped_byte_in_a_blob_is_detected(self):
        victim = blob(self.store, self.case_files["inputs/recorded.json"])
        data = bytearray(victim.read_bytes())
        data[len(data) // 2] ^= 0x01
        victim.write_bytes(bytes(data))
        self.assert_refused("bad_blob")

    def test_content_that_differs_but_decodes_cleanly_is_detected(self):
        victim = blob(self.store, self.case_files["package.json"])
        victim.write_bytes(lzma.compress(b'{"case": 2}\n', format=lzma.FORMAT_XZ))
        self.assert_refused("hashes to")

    def test_a_decompression_bomb_is_bounded(self):
        victim = blob(self.store, self.case_files["package.json"])
        victim.write_bytes(lzma.compress(b"\0" * (64 * 1024 * 1024), format=lzma.FORMAT_XZ, preset=9))
        self.assert_refused("more than the")

    def test_a_short_blob_is_detected(self):
        victim = blob(self.store, self.case_files["inputs/recorded.json"])
        victim.write_bytes(lzma.compress(b"short", format=lzma.FORMAT_XZ))
        self.assert_refused("decompressed to 5 bytes")

    def test_a_truncated_blob_is_detected(self):
        victim = blob(self.store, self.case_files["inputs/recorded.json"])
        victim.write_bytes(victim.read_bytes()[:-8])
        self.assert_refused("bad_blob")

    def test_trailing_data_after_the_stream_is_detected(self):
        victim = blob(self.store, self.case_files["package.json"])
        victim.write_bytes(victim.read_bytes() + b"trailing")
        self.assert_refused("data follows the end of the xz stream")

    def test_a_second_xz_stream_is_trailing_data(self):
        victim = blob(self.store, self.case_files["package.json"])
        victim.write_bytes(victim.read_bytes() + lzma.compress(b"", format=lzma.FORMAT_XZ))
        self.assert_refused("data follows the end of the xz stream")

    # Verifies: SEC-002
    def test_a_symlink_blob_is_refused_and_never_read(self):
        victim = blob(self.store, self.case_files["package.json"])
        copy = self.root / "outside.xz"
        copy.write_bytes(victim.read_bytes())
        victim.unlink()
        os.symlink(copy, victim)
        index = store_module.load_index(self.store)
        with self.assertRaisesRegex(StoreError, "not a regular file"):
            store_module.read_file(self.store, index, "a-case", "package.json")
        self.assert_refused("not a regular file")

    def test_a_symlinked_blob_directory_is_refused(self):
        moved = self.root / "moved-blobs"
        (self.store / "blobs").rename(moved)
        os.symlink(moved, self.store / "blobs")
        index = store_module.load_index(self.store)
        with self.assertRaises(StoreError):
            store_module.read_file(self.store, index, "a-case", "package.json")

    def test_a_missing_blob_is_reported(self):
        blob(self.store, self.case_files["package.json"]).unlink()
        self.assert_refused("missing_blob")

    def test_unreferenced_misnamed_and_extra_entries_are_refused(self):
        extra = self.store / "blobs" / "ab"
        extra.mkdir(exist_ok=True)
        (extra / ("ab" + "0" * 62 + ".xz")).write_bytes(lzma.compress(b"x", format=lzma.FORMAT_XZ))
        self.assert_refused("unreferenced_blob")
        (extra / ("ab" + "0" * 62 + ".xz")).unlink()
        (extra / "readme.txt").write_text("hi")
        self.assert_refused("misnamed_blob")
        (extra / "readme.txt").unlink()
        (self.store / "notes.txt").write_text("hi")
        self.assert_refused("unexpected_entry")


class IndexRefusalTests(StoreFixture):
    def mutate(self, edit, fragment: str):
        if not hasattr(self, "pristine"):
            self.pristine = self.index()
        index = copy.deepcopy(self.pristine)
        edit(index)
        self.write_index(index)
        with self.assertRaisesRegex(StoreError, fragment):
            store_module.load_index(self.store)
        self.assertEqual(store_module.verify_store(self.store)["findings"][0]["kind"], "invalid_index")

    # Verifies: SEC-002
    def test_bad_paths_are_refused(self):
        for path in ("/abs", "a/../b", "./a", "a//b", "", "a\\b", "a\nb", "/".join(["d"] * 65)):
            self.mutate(lambda i, p=path: i["trees"][0]["files"][0].__setitem__("path", p),
                        "path|empty|component")

    def test_unsorted_and_duplicate_entries_are_refused(self):
        self.mutate(lambda i: i["trees"][0]["files"].reverse(), "not sorted")
        self.mutate(lambda i: i["trees"][0]["files"].append(dict(i["trees"][0]["files"][-1])),
                    "more than once")
        self.mutate(lambda i: i["trees"].reverse(), "not sorted by name")
        self.mutate(lambda i: i["trees"].append(dict(i["trees"][-1])), "duplicate tree")

    def test_a_file_that_is_also_a_directory_is_refused(self):
        def edit(index):
            files = index["trees"][0]["files"]
            files.append({"path": "package.json/x", "sha256": files[0]["sha256"], "bytes": files[0]["bytes"]})
            files.sort(key=lambda f: f["path"].encode())
        self.mutate(edit, "also a directory")

    def test_bad_digests_lengths_names_and_schema_are_refused(self):
        self.mutate(lambda i: i["trees"][0]["files"][0].__setitem__("sha256", "A" * 64), "sha256")
        self.mutate(lambda i: i["trees"][0]["files"][0].__setitem__("bytes", -1), "bytes")
        self.mutate(lambda i: i["trees"][0]["files"][0].__setitem__("bytes", True), "bytes")
        self.mutate(lambda i: i["trees"][0].__setitem__("name", "bad name"), "tree name")
        self.mutate(lambda i: i.__setitem__("schema_version", "x"), "schema_version")
        self.mutate(lambda i: i.__setitem__("codec", "zstd"), "codec")
        self.mutate(lambda i: i.__setitem__("extra", 1), "unknown field")
        self.mutate(lambda i: i["trees"][0]["files"][0].__setitem__("mode", 0o644), "unknown field")

    def test_one_digest_with_two_lengths_is_refused(self):
        def edit(index):
            files = index["trees"][1]["files"]
            files[0]["sha256"] = index["trees"][0]["files"][0]["sha256"]
            files[0]["bytes"] = index["trees"][0]["files"][0]["bytes"] + 1
        self.mutate(edit, "two different lengths")

    def test_duplicate_json_keys_and_an_oversized_index_are_refused(self):
        (self.store / "store.json").write_text('{"schema_version": "a", "schema_version": "b"}')
        with self.assertRaisesRegex(StoreError, "duplicate key"):
            store_module.load_index(self.store)
        (self.store / "store.json").write_bytes(b" " * (store_module.MAX_INDEX_BYTES + 1))
        with self.assertRaisesRegex(StoreError, "larger than"):
            store_module.load_index(self.store)

    def test_a_symlinked_index_and_a_missing_store_are_refused(self):
        real = self.root / "real-index.json"
        (self.store / "store.json").rename(real)
        os.symlink(real, self.store / "store.json")
        with self.assertRaisesRegex(StoreError, "regular file"):
            store_module.load_index(self.store)
        with self.assertRaises(StoreError):
            store_module.load_index(self.root / "nowhere")


@unittest.skipUnless(os.environ.get("FARIS_CORE_STORE_VERIFY"),
                     "set FARIS_CORE_STORE_VERIFY to Core's verifier/store_verify.py to cross-check")
class CoreCrossCheckTests(StoreFixture):
    def test_core_python_verifier_accepts_a_faris_written_store_and_unpacks_it_identically(self):
        verifier = os.environ["FARIS_CORE_STORE_VERIFY"]
        verified = subprocess.run([sys.executable, verifier, "store-verify", str(self.store)],
                                  capture_output=True, text=True)
        self.assertEqual(verified.returncode, 0, verified.stdout + verified.stderr)
        out = self.root / "core-out"
        unpacked = subprocess.run([sys.executable, verifier, "store-unpack", str(self.store), str(out)],
                                  capture_output=True, text=True)
        self.assertEqual(unpacked.returncode, 0, unpacked.stderr)
        self.assertEqual(snapshot(out / "a-case"), self.case_files)


if __name__ == "__main__":
    unittest.main()
