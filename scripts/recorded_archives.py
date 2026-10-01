#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Bounded archives for byte-preserving Core case/workspace distribution."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import tarfile
import zlib

SCHEMA = "faris-recorded-tree-archive/v0.1"
MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_MEMBER_BYTES = 64 * 1024 * 1024
MAX_TREE_BYTES = 512 * 1024 * 1024
MAX_TREE_FILES = 2048
MAX_TREE_MEMBERS = 4096
MAX_PATH_COMPONENTS = 64
MAX_TREE_DIRECTORIES = 1024
MAX_EXPANDED_BYTES = 1536 * 1024 * 1024
MAX_MEMBERS = 8192
MAX_EXPANDED_DIRECTORIES = 8192
MAX_NAME_BYTES = 4096
COPY_CHUNK = 1024 * 1024


class _HashingReader:
    def __init__(self, stream):
        self.stream = stream
        self.digest = hashlib.sha256()
        self.bytes = 0

    def read(self, size: int = -1) -> bytes:
        block = self.stream.read(size)
        self.digest.update(block)
        self.bytes += len(block)
        return block


class _SingleMemberGzipStream:
    """Small read-only stream validating exactly one bounded gzip member."""
    def __init__(self, path: Path, max_output: int):
        self.source = path.open("rb")
        self.decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
        self.max_output = max_output
        self.output_bytes = 0
        self.pending = b""
        self.buffer = bytearray()
        self.done = False
        self.closed = False

    def readable(self) -> bool:
        return True

    def read(self, size: int = -1) -> bytes:
        if self.closed:
            raise ValueError("read from closed gzip stream")
        if size == 0:
            return b""
        if size < 0:
            while not self.done:
                self._fill()
            size = len(self.buffer)
        while len(self.buffer) < size and not self.done:
            self._fill()
        result = bytes(self.buffer[:size])
        del self.buffer[:size]
        return result

    def _fill(self) -> None:
        if self.done:
            return
        if not self.pending:
            compressed = self.source.read(COPY_CHUNK)
            if not compressed:
                if not self.decompressor.eof:
                    raise ValueError("gzip stream is truncated")
                self.done = True
                return
        else:
            compressed = self.pending
        output_limit = min(COPY_CHUNK, self.max_output - self.output_bytes + 1)
        if output_limit <= 0:
            raise ValueError("gzip archive exceeds the expanded-byte bound")
        output = self.decompressor.decompress(compressed, output_limit)
        self.pending = self.decompressor.unconsumed_tail
        self.output_bytes += len(output)
        if self.output_bytes > self.max_output:
            raise ValueError("gzip archive exceeds the expanded-byte bound")
        self.buffer.extend(output)
        if self.decompressor.unused_data:
            raise ValueError("archive has concatenated gzip members or trailing bytes")
        if self.decompressor.eof:
            if self.pending or self.source.read(1):
                raise ValueError("archive has concatenated gzip members or trailing bytes")
            self.done = True

    def close(self) -> None:
        if not self.closed:
            self.source.close()
            self.closed = True

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(COPY_CHUNK), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


def canonical_member_name(name: str) -> str:
    if (not isinstance(name, str) or not name or "\\" in name or "\x00" in name
            or len(name.encode("utf-8")) > MAX_NAME_BYTES or name.startswith("/")):
        raise ValueError(f"noncanonical archive path: {name!r}")
    path = PurePosixPath(name)
    if (path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts)
            or ":" in path.parts[0] or path.as_posix() != name):
        raise ValueError(f"noncanonical archive path: {name!r}")
    if len(path.parts) > MAX_PATH_COMPONENTS:
        raise ValueError(f"archive path has more than {MAX_PATH_COMPONENTS} components")
    return name


def _directory_paths(names) -> set[str]:
    directories: set[str] = set()
    for name in names:
        parts = PurePosixPath(name).parts
        for index in range(1, len(parts)):
            directories.add(PurePosixPath(*parts[:index]).as_posix())
    return directories


def count_implicit_directories(records) -> int:
    names = []
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("malformed archive member record")
        names.append(canonical_member_name(record.get("path")))
    count = len(_directory_paths(names))
    if count > MAX_TREE_DIRECTORIES:
        raise ValueError("archive has too many implicit directories")
    return count


def _source_records(source_root: Path) -> list[dict[str, object]]:
    root = source_root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("archive source must be a directory")
    records: list[dict[str, object]] = []
    total = 0
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"archive source contains a symlink: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"archive source contains a nonregular entry: {path}")
        relative = path.relative_to(root).as_posix()
        canonical_member_name(relative)
        size = path.stat().st_size
        if size > MAX_MEMBER_BYTES:
            raise ValueError(f"archive source member exceeds 64 MiB: {relative}")
        total += size
        records.append({"path": relative, "bytes": size, "sha256": sha256(path)})
        if len(records) > MAX_TREE_FILES or total > MAX_TREE_BYTES:
            raise ValueError("archive source exceeds individual Core tree bounds")
    if not records:
        raise ValueError("refusing to archive an empty directory")
    if len(_directory_paths(record["path"] for record in records)) > MAX_TREE_DIRECTORIES:
        raise ValueError("archive source has too many implicit directories")
    return records


def create_archive(source_root: Path, archive_path: Path,
                   manifest_path: Path) -> dict[str, object]:
    """Write USTAR+gzip of regular files only, plus a per-file byte manifest."""
    source_root = source_root.resolve(strict=True)
    records = _source_records(source_root)
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    archive_path.parent.resolve(strict=True)
    with tarfile.open(archive_path, mode="w:gz", format=tarfile.USTAR_FORMAT,
                      compresslevel=9) as archive:
        for record in records:
            relative = str(record["path"])
            source = source_root / relative
            info = archive.gettarinfo(str(source), arcname=relative)
            if not info.isfile():
                raise ValueError(f"archive source changed to a nonregular file: {relative}")
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            info.mode = 0o444
            descriptor = os.open(source, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
            with os.fdopen(descriptor, "rb") as data:
                reader = _HashingReader(data)
                archive.addfile(info, reader)
            if (reader.bytes != record["bytes"]
                    or "sha256:" + reader.digest.hexdigest() != record["sha256"]):
                archive_path.unlink(missing_ok=True)
                raise ValueError(f"archive source changed while reading: {relative}")
    archive_bytes = archive_path.stat().st_size
    if archive_bytes > MAX_ARCHIVE_BYTES:
        archive_path.unlink(missing_ok=True)
        raise ValueError("compressed archive exceeds 64 MiB")
    manifest: dict[str, object] = {
        "schema_version": SCHEMA,
        "archive_sha256": sha256(archive_path),
        "archive_bytes": archive_bytes,
        "expanded_bytes": sum(int(record["bytes"]) for record in records),
        "file_count": len(records),
        "archive_member_count": len(records),
        "directory_count": len(_directory_paths(record["path"] for record in records)),
        "members": records,
    }
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n",
                             encoding="utf-8")
    return manifest


def _validate_manifest(manifest: object, archive_path: Path) -> list[dict[str, object]]:
    if not isinstance(manifest, dict) or manifest.get("schema_version") != SCHEMA:
        raise ValueError("unsupported recorded tree archive manifest")
    if archive_path.stat().st_size > MAX_ARCHIVE_BYTES:
        raise ValueError("compressed archive exceeds 64 MiB")
    if (manifest.get("archive_sha256") != sha256(archive_path)
            or manifest.get("archive_bytes") != archive_path.stat().st_size):
        raise ValueError("compressed archive hash/size mismatch")
    records = manifest.get("members")
    if not isinstance(records, list) or not records or len(records) > MAX_TREE_FILES:
        raise ValueError("archive member manifest is empty or over its bound")
    seen: set[str] = set()
    total = 0
    directories: set[str] = set()
    validated = []
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("malformed archive member record")
        name = canonical_member_name(record.get("path"))
        directories.update(_directory_paths([name]))
        size = record.get("bytes")
        digest = record.get("sha256")
        if (name in seen or not isinstance(size, int) or isinstance(size, bool)
                or size < 0 or size > MAX_MEMBER_BYTES or not isinstance(digest, str)
                or len(digest) != 71 or not digest.startswith("sha256:")):
            raise ValueError(f"malformed or duplicate archive member: {name}")
        try:
            bytes.fromhex(digest[7:])
        except ValueError as error:
            raise ValueError(f"malformed archive member digest: {name}") from error
        seen.add(name)
        total += size
        validated.append({"path": name, "bytes": size, "sha256": digest})
    if (total > MAX_TREE_BYTES or len(directories) > MAX_TREE_DIRECTORIES
            or manifest.get("directory_count") != len(directories)
            or manifest.get("expanded_bytes") != total
            or manifest.get("file_count") != len(validated)
            or not isinstance(manifest.get("archive_member_count"), int)
            or manifest["archive_member_count"] < len(validated)
            or manifest["archive_member_count"] > MAX_TREE_MEMBERS):
        raise ValueError("archive expanded totals do not match their bounded inventory")
    return validated


def _check_one_gzip_member(archive_path: Path, max_uncompressed: int) -> None:
    """Bound gzip inflation and reject concatenated members or trailing bytes."""
    decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
    total = 0
    with archive_path.open("rb") as source:
        while block := source.read(COPY_CHUNK):
            pending = block
            while pending:
                output = decompressor.decompress(pending, COPY_CHUNK)
                pending = decompressor.unconsumed_tail
                total += len(output)
                if total > max_uncompressed:
                    raise ValueError("gzip archive exceeds the expanded-byte bound")
                if decompressor.unused_data:
                    raise ValueError("archive has concatenated gzip members or trailing bytes")
                if not pending:
                    break
    if not decompressor.eof or decompressor.unused_data:
        raise ValueError("archive is truncated or has trailing gzip data")


def _cancelled(cancel_event) -> None:
    if cancel_event is not None and cancel_event.is_set():
        raise InterruptedError("saved Core evidence materialization was cancelled")


def _preflight_tar(archive_path: Path, records: list[dict[str, object]], cancel_event=None) -> int:
    expected = {str(record["path"]): int(record["bytes"]) for record in records}
    observed: dict[str, int] = {}
    count = 0
    try:
        max_raw = int(sum(int(item["bytes"]) for item in records)) + MAX_TREE_MEMBERS * 1024 + 2 * 512 + 10240
        with _SingleMemberGzipStream(archive_path, max_raw) as gzip_stream:
            with tarfile.open(fileobj=gzip_stream, mode="r|") as archive:
                for member in archive:
                    _cancelled(cancel_event)
                    count += 1
                    if count > MAX_TREE_MEMBERS:
                        raise ValueError("archive has too many members")
                    name = canonical_member_name(member.name)
                    if name in observed:
                        raise ValueError(f"archive repeats a normalized path: {name}")
                    if member.pax_headers or member.sparse is not None:
                        raise ValueError(f"archive uses unsupported extension or sparse metadata: {name}")
                    if member.isdir():
                        if member.size != 0:
                            raise ValueError(f"archive directory has a nonzero size: {name}")
                        observed[name] = -1
                    elif member.isfile():
                        if member.size < 0 or member.size > MAX_MEMBER_BYTES:
                            raise ValueError(f"archive member exceeds per-file bound: {name}")
                        observed[name] = member.size
                    else:
                        raise ValueError(f"archive contains a link or special file: {name}")
                trailer = bytearray()
                while block := archive.fileobj.read(COPY_CHUNK):
                    trailer.extend(block)
                    if len(trailer) > MAX_TREE_MEMBERS * 1024 + 12288:
                        raise ValueError("tar archive has excessive trailing padding")
                if (len(trailer) < 1024 or len(trailer) % 512 != 0
                        or any(trailer)):
                    raise ValueError("tar archive has invalid end markers or nonzero trailing data")
    except (tarfile.TarError, OSError, EOFError) as error:
        raise ValueError(f"invalid compressed tar archive: {error}") from error
    files = {name: size for name, size in observed.items() if size >= 0}
    if files != expected:
        raise ValueError("archive members differ from the declared per-file inventory")
    for name in expected:
        parts = PurePosixPath(name).parts
        for index in range(1, len(parts)):
            ancestor = PurePosixPath(*parts[:index]).as_posix()
            if ancestor in expected:
                raise ValueError(f"archive file/directory prefix conflict: {ancestor} and {name}")
            if observed.get(ancestor, -1) >= 0:
                raise ValueError(f"archive member conflicts with a parent file: {ancestor} and {name}")
    actual_directories = set(name for name, size in observed.items() if size == -1)
    actual_directories.update(_directory_paths(expected))
    if len(actual_directories) > MAX_TREE_DIRECTORIES:
        raise ValueError("archive has too many explicit and implicit directories")
    for name, size in observed.items():
        if size == -1 and not any(item.startswith(name + "/") for item in expected):
            raise ValueError(f"archive contains an unneeded directory entry: {name}")
    return count


def extract_archive(archive_path: Path, manifest_path: Path,
                    target: Path, cancel_event=None) -> dict[str, object]:
    """Preflight and extract only verified regular members below a fresh directory."""
    if target.exists():
        raise ValueError(f"archive extraction target must be new: {target}")
    manifest_bytes = manifest_path.stat().st_size
    if manifest_bytes > MAX_ARCHIVE_BYTES:
        raise ValueError("archive manifest exceeds 64 MiB")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    records = _validate_manifest(manifest, archive_path)
    declared_total = int(manifest["expanded_bytes"])
    tar_overhead_bound = MAX_TREE_MEMBERS * 1024 + 2 * 512 + 10240
    archive_member_count = _preflight_tar(archive_path, records, cancel_event)
    if archive_member_count != manifest.get("archive_member_count"):
        raise ValueError("archive member count differs from its manifest")

    target.mkdir(parents=True, mode=0o700)
    root = target.resolve(strict=True)
    expected = {str(record["path"]): record for record in records}
    observed: set[str] = set()
    actual_total = 0
    try:
        max_raw = declared_total + tar_overhead_bound
        with _SingleMemberGzipStream(archive_path, max_raw) as gzip_stream:
            with tarfile.open(fileobj=gzip_stream, mode="r|") as archive:
                for member in archive:
                    _cancelled(cancel_event)
                    name = canonical_member_name(member.name)
                    if member.isdir():
                        destination = root.joinpath(*PurePosixPath(name).parts)
                        destination.mkdir(parents=True, exist_ok=True, mode=0o700)
                        if not destination.resolve(strict=True).is_relative_to(root):
                            raise ValueError(f"archive directory escapes extraction root: {name}")
                        continue
                    record = expected.get(name)
                    if record is None or name in observed:
                        raise ValueError(f"unexpected or duplicate archive member: {name}")
                    destination = root.joinpath(*PurePosixPath(name).parts)
                    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                    if not destination.parent.resolve(strict=True).is_relative_to(root):
                        raise ValueError(f"archive member escapes extraction root: {name}")
                    digest = hashlib.sha256()
                    actual = 0
                    stream = archive.extractfile(member)
                    if stream is None:
                        raise ValueError(f"cannot read archive member: {name}")
                    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
                    if hasattr(os, "O_NOFOLLOW"):
                        flags |= os.O_NOFOLLOW
                    descriptor = os.open(destination, flags, 0o444)
                    with os.fdopen(descriptor, "wb") as output:
                        while block := stream.read(COPY_CHUNK):
                            _cancelled(cancel_event)
                            actual += len(block)
                            actual_total += len(block)
                            if (actual > int(record["bytes"]) or actual > MAX_MEMBER_BYTES
                                    or actual_total > declared_total
                                    or actual_total > MAX_TREE_BYTES):
                                raise ValueError(f"archive member exceeded its declared size: {name}")
                            digest.update(block)
                            output.write(block)
                    if actual != record["bytes"] or "sha256:" + digest.hexdigest() != record["sha256"]:
                        raise ValueError(f"archive member hash/size mismatch: {name}")
                    observed.add(name)
                trailer = bytearray()
                while block := archive.fileobj.read(COPY_CHUNK):
                    trailer.extend(block)
                    if len(trailer) > MAX_TREE_MEMBERS * 1024 + 12288:
                        raise ValueError("tar archive has excessive trailing padding")
                if (len(trailer) < 1024 or len(trailer) % 512 != 0
                        or any(trailer)):
                    raise ValueError("tar archive has invalid end markers or nonzero trailing data")
        if observed != set(expected) or actual_total != declared_total:
            raise ValueError("archive extraction did not match the declared inventory")
    except Exception:
        import shutil
        shutil.rmtree(target, ignore_errors=True)
        raise
    return manifest


def _safe_package_file(package: Path, relative: object) -> Path:
    if not isinstance(relative, str):
        raise ValueError("package archive path is missing")
    canonical_member_name(relative)
    cursor = package.resolve(strict=True)
    for part in PurePosixPath(relative).parts:
        cursor = cursor / part
        if cursor.is_symlink():
            raise ValueError(f"package archive path traverses a symlink: {relative}")
    resolved = cursor.resolve(strict=True)
    if not resolved.is_file() or not resolved.is_relative_to(package.resolve(strict=True)):
        raise ValueError(f"package archive path is missing or escapes package: {relative}")
    return resolved


def extract_indexed_trees(package: Path, index: dict[str, object], target: Path,
                          cancel_event=None) -> dict[tuple[str, str], tuple[Path, Path]]:
    """Extract all eight named trees and enforce aggregate expanded limits."""
    if target.exists():
        raise ValueError(f"archive extraction root must be new: {target}")
    target.mkdir(parents=True, mode=0o700)
    total_bytes = 0
    total_files = 0
    total_members = 0
    total_directories = 0
    result: dict[tuple[str, str], tuple[Path, Path]] = {}
    try:
        for pair in index.get("scenario_pairs", []):
            pair_id = str(pair["scenario_path"]).split("/", 1)[0]
            canonical_member_name(pair_id)
            if "/" in pair_id:
                raise ValueError("scenario pair ID must be one path component")
            for arrangement in pair["arrangements"]:
                variant = arrangement["variant_id"]
                canonical_member_name(variant)
                if "/" in variant:
                    raise ValueError("variant ID must be one path component")
                outputs = []
                for kind in ("case", "workspace"):
                    descriptor = arrangement.get(f"{kind}_archive")
                    if not isinstance(descriptor, dict):
                        raise ValueError(f"missing {kind} archive descriptor for {pair_id}/{variant}")
                    archive_path = _safe_package_file(package, descriptor.get("path"))
                    manifest_path = _safe_package_file(package, descriptor.get("manifest_path"))
                    if (sha256(archive_path) != descriptor.get("sha256")
                            or sha256(manifest_path) != descriptor.get("manifest_sha256")):
                        raise ValueError(f"{kind} archive identity mismatch for {pair_id}/{variant}")
                    tree_parent = "cases" if kind == "case" else "core-workspaces"
                    destination = target / pair_id / tree_parent / variant
                    manifest = extract_archive(archive_path, manifest_path, destination, cancel_event)
                    if (manifest.get("expanded_bytes") != descriptor.get("expanded_bytes")
                            or manifest.get("file_count") != descriptor.get("file_count")):
                        raise ValueError(f"{kind} archive totals differ from package index")
                    total_bytes += int(manifest["expanded_bytes"])
                    total_files += int(manifest["file_count"])
                    total_members += int(manifest["archive_member_count"])
                    total_directories += int(manifest["directory_count"])
                    if (total_bytes > MAX_EXPANDED_BYTES or total_files > MAX_MEMBERS
                            or total_members > MAX_MEMBERS
                            or total_directories > MAX_EXPANDED_DIRECTORIES):
                        raise ValueError("combined case/workspace expansion exceeds package limits")
                    outputs.append(destination)
                scenario_source = _safe_package_file(package, pair.get("scenario_path"))
                scenario_target = target / pair_id / "scenario.json"
                scenario_target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                import shutil
                shutil.copyfile(scenario_source, scenario_target)
                descriptor_path = target / f"saved-study-{pair_id}-{variant}.json"
                descriptor_path.write_text(json.dumps({
                    "case_directory": outputs[0].relative_to(target).as_posix(),
                    "execution_report": (outputs[0] / "execution-report.json").relative_to(target).as_posix(),
                    "execution_workspace": outputs[1].relative_to(target).as_posix(),
                }, indent=2) + "\n", encoding="utf-8")
                result[(pair_id, variant)] = (outputs[0], outputs[1])
    except Exception:
        import shutil
        shutil.rmtree(target, ignore_errors=True)
        raise
    return result
