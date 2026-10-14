#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Audit that no nuclear-data file is in Git or inside a `.faris` study package (NUC-069).

Git: every tracked file is checked by name (HDF5, ENDF and ACE suffixes, `cross_sections.xml`) and,
for files up to 64 MiB, by the HDF5 signature in its first bytes, so a renamed library file is
still found. A tracked file listed in ALLOWED_IN_GIT is reported as a documented exception, not
as a finding.

Packages: each path given with `--package` is opened as a zip (a `.faris` study file is one) and
every member, and every member of a tar archive stored inside it, is checked the same way.

Exit status 1 when anything outside the exceptions is found. Reads files only.

    audit_nuclear_data_files.py                      # the repository this script is in
    audit_nuclear_data_files.py --package a.faris    # also audit study packages
"""
from __future__ import annotations

import argparse
import io
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DATA_SUFFIXES = {".h5", ".hdf5", ".endf", ".ace", ".endf6"}
DATA_NAMES = {"cross_sections.xml", "chain.xml"}
HDF5_SIGNATURE = b"\x89HDF\r\n\x1a\n"
# HDF5 containers that hold geometry, not nuclear data (MOAB meshes for DAGMC).
GEOMETRY_SUFFIXES = {".h5m"}
MAX_SCANNED_BYTES = 64 * 1024 * 1024
# Tracked on purpose, each with its reason. Anything else is a finding.
ALLOWED_IN_GIT = {
    "integrations/windows-spike/data/H1.h5": (
        "0.3 MB public-domain ENDF/B-VII.1 H1 evaluation kept as the Windows native spike's only data fixture "
        "(docs/notes/WINDOWS_NATIVE_SPIKE.md); the spike's cross_sections.xml points at it"
    ),
    "integrations/windows-spike/data/cross_sections.xml": "index of that single fixture",
}


def is_data_name(name: str) -> bool:
    path = Path(name)
    return path.suffix.lower() in DATA_SUFFIXES or path.name in DATA_NAMES


def audit_git(root: Path) -> tuple[list[str], list[str]]:
    """(findings, documented exceptions) for the files tracked in the repository at `root`."""
    listing = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=True
    ).stdout.decode("utf-8", "surrogateescape")
    findings, allowed = [], []
    for name in filter(None, listing.split("\0")):
        path = root / name
        reason = None
        if is_data_name(name):
            reason = "data file name"
        elif (
            path.suffix.lower() not in GEOMETRY_SUFFIXES
            and path.is_file()
            and not path.is_symlink()
            and 8 <= path.stat().st_size <= MAX_SCANNED_BYTES
        ):
            with path.open("rb") as stream:
                if stream.read(8) == HDF5_SIGNATURE:
                    reason = "HDF5 signature under another name"
        if reason is None:
            continue
        if name in ALLOWED_IN_GIT:
            allowed.append(f"{name}: {ALLOWED_IN_GIT[name]}")
        else:
            findings.append(f"{name}: {reason} is tracked in Git")
    return findings, allowed


def audit_members(label: str, names_and_heads) -> list[str]:
    return [
        f"{label}: {name} is a nuclear-data file"
        for name, head in names_and_heads
        if is_data_name(name) or head.startswith(HDF5_SIGNATURE)
    ]


def audit_package(path: Path) -> list[str]:
    """Findings inside a zip-format study package, including tar archives stored in it."""
    findings = []
    with zipfile.ZipFile(path) as archive:
        members = []
        for info in archive.infolist():
            if info.is_dir():
                continue
            with archive.open(info) as stream:
                members.append((info.filename, stream.read(8)))
            if info.filename.endswith((".tar", ".tar.xz", ".tar.gz", ".txz", ".tgz")) and info.file_size <= MAX_SCANNED_BYTES:
                try:
                    inner = tarfile.open(fileobj=io.BytesIO(archive.read(info)))
                except tarfile.TarError:
                    findings.append(f"{path.name}: {info.filename} is a tar archive that cannot be read, so it is not audited")
                    continue
                with inner:
                    entries = []
                    for entry in inner:
                        if entry.isfile():
                            handle = inner.extractfile(entry)
                            entries.append((entry.name, handle.read(8) if handle else b""))
                    findings += audit_members(f"{path.name}:{info.filename}", entries)
        findings += audit_members(path.name, members)
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=REPO, help="repository to audit")
    parser.add_argument("--package", type=Path, action="append", default=[], help="a .faris study package (zip)")
    args = parser.parse_args()
    findings, allowed = audit_git(args.root)
    for package in args.package:
        findings += audit_package(package)
    for line in allowed:
        print(f"documented exception: {line}")
    for line in findings:
        print(f"FOUND {line}")
    if findings:
        print(f"{len(findings)} finding(s)", file=sys.stderr)
        return 1
    print("nuclear-data audit: no findings")
    return 0


if __name__ == "__main__":
    sys.exit(main())
