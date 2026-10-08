#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""FARIS reader, writer and verifier for the content-addressed evidence store.

The format is `avila.core/evidence-store/v0.1` (Avila Core ADR-0028): one
directory holding `store.json` and `blobs/<h0h1>/<h>.xz`, where `<h>` is the
SHA-256 of the file content before compression. Several named file trees share
one blob per distinct content. This module is FARIS's own implementation, as
the Rust reader in faris-engine is; the standard library (`lzma`, `hashlib`,
`json`) is all it needs.

Nothing read from a store is believed. The index is checked against the format
rules, every blob is decompressed with its output bounded to the indexed
length plus one byte, and content is returned only when its length and digest
match. A blob that is a symlink or not a regular file, has data after the end
of the xz stream, or sits where the index does not name it is an error.
"""
from __future__ import annotations

import hashlib
import json
import lzma
import os
from pathlib import Path
import re
import shutil
import stat
import unicodedata
from typing import Any, Iterator

SCHEMA_VERSION = "avila.core/evidence-store/v0.1"
CODEC = "xz"
XZ_PRESET = 9
DECODER_MEMORY_LIMIT = 1 << 30
MAX_INDEX_BYTES = 64 * 1024 * 1024
MAX_TREES = 1_024
MAX_FILES_PER_TREE = 65_536
MAX_FILES_TOTAL = 262_144
MAX_FILE_BYTES = 4 * 1024 * 1024 * 1024
MAX_TREE_NAME_CHARS = 128
MAX_PATH_COMPONENTS = 64
MAX_REPORTED_FINDINGS = 1_000
CHUNK = 64 * 1024

_TREE_NAME = re.compile(r"[A-Za-z0-9._-]+\Z")
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
_FANOUT = re.compile(r"[0-9a-f]{2}\Z")


class StoreError(ValueError):
    """A store, its index, or one of its blobs does not meet the format."""


# ---- index ------------------------------------------------------------------


def _no_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise StoreError(f"duplicate key `{key}` in store.json")
        result[key] = value
    return result


def _check_keys(value: Any, keys: set[str], what: str) -> None:
    if not isinstance(value, dict):
        raise StoreError(f"{what} must be an object")
    unknown, missing = set(value) - keys, keys - set(value)
    if unknown:
        raise StoreError(f"{what} has unknown field `{sorted(unknown)[0]}`")
    if missing:
        raise StoreError(f"{what} is missing field `{sorted(missing)[0]}`")


def validate_tree_name(name: Any) -> None:
    if not isinstance(name, str) or not 1 <= len(name) <= MAX_TREE_NAME_CHARS:
        raise StoreError(f"tree name must be 1 to {MAX_TREE_NAME_CHARS} characters")
    if name in (".", "..") or not _TREE_NAME.match(name):
        raise StoreError(f"tree name `{name}` is not allowed (A-Z a-z 0-9 . _ - only)")


def validate_path(path: Any) -> None:
    if not isinstance(path, str) or path == "":
        raise StoreError("path is empty or not a string")
    try:
        path.encode("utf-8")
    except UnicodeEncodeError:
        raise StoreError(f"path {path!r} is not valid UTF-8") from None
    if path.startswith("/"):
        raise StoreError(f"path `{path}` is absolute")
    if any(char == "\\" or unicodedata.category(char) == "Cc" for char in path):
        raise StoreError(f"path {path!r} contains a backslash or control character")
    parts = path.split("/")
    if len(parts) > MAX_PATH_COMPONENTS:
        raise StoreError(f"path `{path}` has more than {MAX_PATH_COMPONENTS} components")
    if any(part in ("", ".", "..") for part in parts):
        raise StoreError(f"path `{path}` has an empty, `.` or `..` component")


def implicit_directories(paths) -> set[str]:
    """Every proper directory prefix of the given file paths."""
    directories: set[str] = set()
    for path in paths:
        parts = path.split("/")
        for end in range(1, len(parts)):
            directories.add("/".join(parts[:end]))
    return directories


def validate_index(index: Any) -> None:
    """Rules 1 to 3 and 6 of ADR-0028."""
    _check_keys(index, {"schema_version", "codec", "trees"}, "store index")
    if index["schema_version"] != SCHEMA_VERSION:
        raise StoreError(f"schema_version must be `{SCHEMA_VERSION}`")
    if index["codec"] != CODEC:
        raise StoreError(f"codec must be `{CODEC}`")
    trees = index["trees"]
    if not isinstance(trees, list):
        raise StoreError("trees must be an array")
    if len(trees) > MAX_TREES:
        raise StoreError(f"more than {MAX_TREES} trees")
    total = 0
    lengths: dict[str, int] = {}
    previous_name = None
    for tree in trees:
        _check_keys(tree, {"name", "files"}, "tree")
        name = tree["name"]
        validate_tree_name(name)
        if previous_name is not None:
            if name == previous_name:
                raise StoreError(f"duplicate tree name `{name}`")
            if name < previous_name:
                raise StoreError(f"trees are not sorted by name (`{name}` after `{previous_name}`)")
        previous_name = name
        files = tree["files"]
        if not isinstance(files, list):
            raise StoreError(f"tree `{name}` files must be an array")
        if len(files) > MAX_FILES_PER_TREE:
            raise StoreError(f"tree `{name}` has more than {MAX_FILES_PER_TREE} files")
        total += len(files)
        if total > MAX_FILES_TOTAL:
            raise StoreError(f"more than {MAX_FILES_TOTAL} files in total")
        paths: set[str] = set()
        previous_key = None
        for entry in files:
            _check_keys(entry, {"path", "sha256", "bytes"}, "file entry")
            path = entry["path"]
            try:
                validate_path(path)
            except StoreError as error:
                raise StoreError(f"tree `{name}`: {error}") from None
            key = path.encode("utf-8")
            if previous_key is not None:
                if key == previous_key:
                    raise StoreError(f"tree `{name}` lists `{path}` more than once")
                if key < previous_key:
                    raise StoreError(f"tree `{name}` files are not sorted by path (at `{path}`)")
            previous_key = key
            paths.add(path)
            digest, size = entry["sha256"], entry["bytes"]
            if not isinstance(digest, str) or not _SHA256.match(digest):
                raise StoreError(f"tree `{name}` file `{path}`: sha256 must be 64 lowercase hex characters")
            if type(size) is not int or not 0 <= size <= MAX_FILE_BYTES:
                raise StoreError(f"tree `{name}` file `{path}`: bytes must be an integer 0..{MAX_FILE_BYTES}")
            if lengths.setdefault(digest, size) != size:
                raise StoreError(f"digest {digest} is recorded with two different lengths")
        for directory in implicit_directories(paths):
            if directory in paths:
                raise StoreError(f"tree `{name}`: `{directory}` is a file and also a directory prefix")


def load_index(store: str | os.PathLike) -> dict[str, Any]:
    store = os.fspath(store)
    if not os.path.isdir(store):
        raise StoreError(f"`{store}` is not a directory")
    path = os.path.join(store, "store.json")
    try:
        info = os.lstat(path)
    except OSError as error:
        raise StoreError(f"cannot read store.json: {error}") from None
    if not stat.S_ISREG(info.st_mode):
        raise StoreError("store.json is not a regular file")
    with open(path, "rb") as handle:
        raw = handle.read(MAX_INDEX_BYTES + 1)
    if len(raw) > MAX_INDEX_BYTES:
        raise StoreError(f"store.json is larger than {MAX_INDEX_BYTES} bytes")
    try:
        index = json.loads(raw.decode("utf-8"), object_pairs_hook=_no_duplicate_keys)
    except (UnicodeDecodeError, ValueError) as error:
        if isinstance(error, StoreError):
            raise
        raise StoreError(f"store.json is not valid JSON: {error}") from None
    validate_index(index)
    return index


def tree_entries(index: dict[str, Any], tree: str) -> list[dict[str, Any]]:
    for candidate in index["trees"]:
        if candidate["name"] == tree:
            return candidate["files"]
    raise StoreError(f"store has no tree `{tree}`")


# ---- blobs ------------------------------------------------------------------


def blob_relpath(digest: str) -> str:
    return os.path.join("blobs", digest[:2], digest + ".xz")


def _blob_path(store: str, digest: str) -> str:
    """The blob's path, after checking `blobs/` and its fan-out directory are
    real directories and the blob is a regular file, not a symlink."""
    for directory in (os.path.join(store, "blobs"), os.path.join(store, "blobs", digest[:2])):
        try:
            info = os.lstat(directory)
        except FileNotFoundError:
            raise StoreError(f"blob {digest}: the blob file is missing") from None
        if not stat.S_ISDIR(info.st_mode):
            raise StoreError(f"blob {digest}: `{directory}` is not a directory")
    path = os.path.join(store, blob_relpath(digest))
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        raise StoreError(f"blob {digest}: the blob file is missing") from None
    if not stat.S_ISREG(info.st_mode):
        raise StoreError(f"blob {digest}: the blob is not a regular file")
    return path


def iter_blob(store: str | os.PathLike, digest: str, nbytes: int) -> Iterator[bytes]:
    """Yield a blob's decompressed content in chunks.

    Output is bounded to `nbytes` + 1. The length and SHA-256 are checked when
    the stream ends, so the chunks are provisional until the generator is
    exhausted without raising `StoreError`.
    """
    path = _blob_path(os.fspath(store), digest)
    decoder = lzma.LZMADecompressor(format=lzma.FORMAT_XZ, memlimit=DECODER_MEMORY_LIMIT)
    hasher = hashlib.sha256()
    produced = 0
    with open(path, "rb") as handle:
        try:
            while not decoder.eof:
                data = handle.read(CHUNK)
                if not data:
                    raise StoreError(f"blob {digest}: the xz stream is truncated")
                while True:
                    out = decoder.decompress(data, max_length=min(CHUNK, nbytes + 1 - produced))
                    data = b""
                    if out:
                        produced += len(out)
                        if produced > nbytes:
                            raise StoreError(
                                f"blob {digest}: decompresses to more than the {nbytes} bytes the index records")
                        hasher.update(out)
                        yield out
                    if decoder.eof or decoder.needs_input:
                        break
        except lzma.LZMAError as error:
            raise StoreError(f"blob {digest}: xz decoding failed: {error}") from None
        if decoder.unused_data or handle.read(1):
            raise StoreError(f"blob {digest}: data follows the end of the xz stream")
    if produced != nbytes:
        raise StoreError(f"blob {digest}: decompressed to {produced} bytes, index records {nbytes}")
    if hasher.hexdigest() != digest:
        raise StoreError(f"blob {digest}: decompressed content hashes to {hasher.hexdigest()}")


def read_file(store: str | os.PathLike, index: dict[str, Any], tree: str, path: str) -> bytes:
    """One whole file, returned only after its length and digest match."""
    for entry in tree_entries(index, tree):
        if entry["path"] == path:
            return b"".join(iter_blob(store, entry["sha256"], entry["bytes"]))
    raise StoreError(f"tree `{tree}` has no file `{path}`")


# ---- verification -----------------------------------------------------------


def _check_layout(store: str, referenced: set[str], findings: list[dict[str, str]]) -> None:
    def add(kind: str, path: str, detail: str) -> None:
        findings.append({"kind": kind, "path": path, "detail": detail})

    for name in sorted(os.listdir(store)):
        if name not in ("store.json", "blobs"):
            add("unexpected_entry", name, "only store.json and blobs/ may appear in a store")
    blobs = os.path.join(store, "blobs")
    try:
        info = os.lstat(blobs)
    except FileNotFoundError:
        return
    if not stat.S_ISDIR(info.st_mode):
        add("layout", "blobs", "blobs is not a directory")
        return
    for fanout in sorted(os.listdir(blobs)):
        relative = os.path.join("blobs", fanout)
        fanout_path = os.path.join(blobs, fanout)
        if not _FANOUT.match(fanout) or not stat.S_ISDIR(os.lstat(fanout_path).st_mode):
            add("misnamed_blob", relative, "blobs/ may contain only two-character lowercase hex directories")
            continue
        for name in sorted(os.listdir(fanout_path)):
            blob_relative = os.path.join(relative, name)
            digest = name[:-3] if name.endswith(".xz") else ""
            if not _SHA256.match(digest) or digest[:2] != fanout:
                add("misnamed_blob", blob_relative, "a blob must be named <h0h1>/<sha256>.xz")
            elif digest not in referenced:
                add("unreferenced_blob", blob_relative, "no entry in the index references this blob")
            elif not stat.S_ISREG(os.lstat(os.path.join(fanout_path, name)).st_mode):
                add("not_regular_blob", blob_relative, "the blob is not a regular file")


def verify_store(store: str | os.PathLike) -> dict[str, Any]:
    """Verify a whole store; `status` is `verified` only if the index, every
    blob and the directory layout all pass."""
    store = os.fspath(store)
    report: dict[str, Any] = {
        "store": store, "status": "failed", "trees": 0, "files": 0, "distinct_blobs": 0,
        "uncompressed_bytes": 0, "stored_bytes": 0, "finding_count": 0, "findings": [],
    }
    try:
        index = load_index(store)
    except StoreError as error:
        report["finding_count"] = 1
        report["findings"] = [{"kind": "invalid_index", "path": "store.json", "detail": str(error)}]
        return report
    findings: list[dict[str, str]] = []
    referenced: dict[str, int] = {}
    for tree in index["trees"]:
        report["files"] += len(tree["files"])
        for entry in tree["files"]:
            referenced[entry["sha256"]] = entry["bytes"]
            report["uncompressed_bytes"] += entry["bytes"]
    _check_layout(store, set(referenced), findings)
    for digest, nbytes in sorted(referenced.items()):
        try:
            for _ in iter_blob(store, digest, nbytes):
                pass
        except StoreError as error:
            kind = "missing_blob" if "is missing" in str(error) else "bad_blob"
            findings.append({"kind": kind, "path": blob_relpath(digest), "detail": str(error)})
        else:
            report["stored_bytes"] += os.stat(os.path.join(store, blob_relpath(digest))).st_size
    report.update(trees=len(index["trees"]), distinct_blobs=len(referenced),
                  finding_count=len(findings), findings=findings[:MAX_REPORTED_FINDINGS],
                  status="verified" if not findings else "failed")
    return report


def check_store(store: str | os.PathLike) -> dict[str, Any]:
    """`verify_store`, raising `StoreError` with the first findings when it fails."""
    report = verify_store(store)
    if report["status"] != "verified":
        shown = "; ".join(f"{item['kind']} {item['path']}: {item['detail']}"
                          for item in report["findings"][:3])
        raise StoreError(f"evidence store failed verification ({report['finding_count']} findings): {shown}")
    return report


# ---- unpacking --------------------------------------------------------------


def _destination(root: str, relative: str) -> str:
    destination = root
    for component in relative.split("/"):
        if os.path.splitdrive(component)[0] or os.sep in component or (
                os.altsep and os.altsep in component):
            raise StoreError(f"path `{relative}` has a component this platform cannot confine: `{component}`")
        destination = os.path.join(destination, component)
    return destination


def unpack_store(store: str | os.PathLike, out: str | os.PathLike,
                 trees: list[str] | None = None) -> dict[str, Any]:
    """Write trees byte-identically under a new directory `out` (as
    `out/<name>/<path>`), verifying each file as it is written. `out` must not
    exist; on failure it is removed."""
    store, out = os.fspath(store), os.fspath(out)
    index = load_index(store)
    by_name = {tree["name"]: tree for tree in index["trees"]}
    selected = []
    for name in trees or [tree["name"] for tree in index["trees"]]:
        if name not in by_name:
            raise StoreError(f"store has no tree `{name}`")
        if by_name[name] not in selected:
            selected.append(by_name[name])
    if os.path.lexists(out):
        raise StoreError(f"`{out}` already exists; unpacking only writes to a new directory")
    os.makedirs(out)
    files = total = 0
    written: dict[str, str] = {}
    try:
        for tree in selected:
            tree_root = os.path.join(out, tree["name"])
            os.mkdir(tree_root)
            for entry in tree["files"]:
                destination = _destination(tree_root, entry["path"])
                os.makedirs(os.path.dirname(destination), exist_ok=True)
                first = written.get(entry["sha256"])
                if first is not None:
                    shutil.copyfile(first, destination)
                else:
                    with open(destination, "xb") as handle:
                        for chunk in iter_blob(store, entry["sha256"], entry["bytes"]):
                            handle.write(chunk)
                    written[entry["sha256"]] = destination
                files += 1
                total += entry["bytes"]
    except BaseException:
        shutil.rmtree(out, ignore_errors=True)
        raise
    return {"out": out, "trees": len(selected), "files": files, "bytes": total}


# ---- packing ----------------------------------------------------------------


def _hash_file(path: str) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with open(path, "rb") as handle:
        while block := handle.read(1 << 20):
            hasher.update(block)
            size += len(block)
    return hasher.hexdigest(), size


def _scan_tree(name: str, root: str) -> list[tuple[str, dict[str, Any]]]:
    """The regular files under `root` as (source path, index entry). Symlinks,
    special files and names that are not valid store paths are refused."""
    found: list[tuple[str, dict[str, Any]]] = []
    pending = [(root, [])]
    while pending:
        directory, prefix = pending.pop()
        with os.scandir(directory) as entries:
            listing = list(entries)
        for item in listing:
            components = prefix + [item.name]
            info = os.lstat(item.path)
            if stat.S_ISLNK(info.st_mode):
                raise StoreError(f"tree `{name}`: `{item.path}` is a symbolic link")
            if stat.S_ISDIR(info.st_mode):
                if len(components) >= MAX_PATH_COMPONENTS:
                    raise StoreError(f"tree `{name}`: `{item.path}` is nested deeper than the format allows")
                pending.append((item.path, components))
            elif stat.S_ISREG(info.st_mode):
                relative = "/".join(components)
                try:
                    validate_path(relative)
                except StoreError as error:
                    raise StoreError(f"tree `{name}`: `{item.path}` is not a valid store path: {error}") from None
                digest, size = _hash_file(item.path)
                if size != info.st_size:
                    raise StoreError(f"tree `{name}`: `{item.path}` changed size while it was being read")
                found.append((item.path, {"path": relative, "sha256": digest, "bytes": size}))
            else:
                raise StoreError(f"tree `{name}`: `{item.path}` is not a regular file or directory")
    found.sort(key=lambda pair: pair[1]["path"].encode("utf-8"))
    return found


def _write_blob(source: str, entry: dict[str, Any], destination: str) -> int:
    compressor = lzma.LZMACompressor(format=lzma.FORMAT_XZ, check=lzma.CHECK_CRC64, preset=XZ_PRESET)
    hasher = hashlib.sha256()
    length = 0
    with open(source, "rb") as handle, open(destination, "xb") as output:
        while block := handle.read(CHUNK):
            hasher.update(block)
            length += len(block)
            output.write(compressor.compress(block))
        output.write(compressor.flush())
    if length != entry["bytes"] or hasher.hexdigest() != entry["sha256"]:
        raise StoreError(f"`{source}` changed while the store was being written")
    return os.stat(destination).st_size


def pack_store(out: str | os.PathLike, trees: dict[str, str | os.PathLike]) -> dict[str, Any]:
    """Create a new store at `out` from named directory trees.

    Blobs are written first and `store.json` last, so a directory without an
    index is an unfinished store, never a valid one. On failure `out` is
    removed. Returns the totals the package index records.
    """
    out = os.fspath(out)
    names = sorted(trees)
    for name in names:
        validate_tree_name(name)
    if os.path.lexists(out):
        raise StoreError(f"`{out}` already exists; a store is only written to a new location")
    index_trees: list[dict[str, Any]] = []
    scanned: list[list[tuple[str, dict[str, Any]]]] = []
    for name in names:
        directory = os.fspath(trees[name])
        if not os.path.isdir(directory):
            raise StoreError(f"tree `{name}`: `{directory}` is not a directory")
        files = _scan_tree(name, directory)
        index_trees.append({"name": name, "files": [entry for _, entry in files]})
        scanned.append(files)
    index = {"schema_version": SCHEMA_VERSION, "codec": CODEC, "trees": index_trees}
    validate_index(index)
    os.makedirs(os.path.join(out, "blobs"))
    try:
        done: set[str] = set()
        stored_bytes = distinct_bytes = uncompressed = file_count = 0
        for source, entry in (pair for files in scanned for pair in files):
            file_count += 1
            uncompressed += entry["bytes"]
            if entry["sha256"] in done:
                continue
            done.add(entry["sha256"])
            destination = os.path.join(out, blob_relpath(entry["sha256"]))
            os.makedirs(os.path.dirname(destination), exist_ok=True)
            stored_bytes += _write_blob(source, entry, destination)
            distinct_bytes += entry["bytes"]
        with open(os.path.join(out, "store.json"), "xb") as handle:
            handle.write((json.dumps(index, indent=2) + "\n").encode("utf-8"))
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        shutil.rmtree(out, ignore_errors=True)
        raise
    return {
        "out": out, "files": file_count, "blob_count": len(done),
        "uncompressed_bytes": uncompressed, "distinct_bytes": distinct_bytes,
        "stored_bytes": stored_bytes,
        "trees": [{"name": tree["name"], "file_count": len(tree["files"]),
                   "bytes": sum(entry["bytes"] for entry in tree["files"]),
                   "directory_count": len(implicit_directories(e["path"] for e in tree["files"]))}
                  for tree in index_trees],
    }


def tree_summary(index: dict[str, Any]) -> list[dict[str, Any]]:
    """The per-tree totals the package index records, from a store's own index."""
    return [{"name": tree["name"], "file_count": len(tree["files"]),
             "bytes": sum(entry["bytes"] for entry in tree["files"]),
             "directory_count": len(implicit_directories(e["path"] for e in tree["files"]))}
            for tree in index["trees"]]


def main(argv: list[str] | None = None) -> int:
    """`evidence_store.py verify|unpack|pack ...`, for scripts and cross-checks."""
    import argparse
    import sys
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("store")
    unpack = commands.add_parser("unpack")
    unpack.add_argument("store")
    unpack.add_argument("out")
    unpack.add_argument("--tree", action="append")
    pack = commands.add_parser("pack")
    pack.add_argument("out")
    pack.add_argument("tree", nargs="+", metavar="NAME=DIR")
    args = parser.parse_args(argv)
    try:
        if args.command == "verify":
            report = verify_store(args.store)
            print(json.dumps(report, indent=2))
            return 0 if report["status"] == "verified" else 1
        if args.command == "unpack":
            print(json.dumps(unpack_store(args.store, args.out, args.tree), indent=2))
            return 0
        pairs = dict(item.split("=", 1) for item in args.tree)
        print(json.dumps(pack_store(args.out, pairs), indent=2))
        return 0
    except StoreError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
