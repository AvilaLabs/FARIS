#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Scan a FARIS download for restricted or unlisted binaries (requirement LEG-016).

The handbook page docs/guide/external-tools.md says no restricted tool is bundled: no OpenMC, no DAGMC,
no nuclear data, no code from an access-controlled library. This scan makes that checkable on a package
folder (scripts/make_release.py runs it before any archive is written):

- a native executable or library (ELF, PE, Mach-O magic) must be one of the programs listed in ALLOWED_PROGRAMS;
- no file name may match the tools that must never be bundled (OpenMC, DAGMC, MOAB, double-down, OpenCASCADE
  shared libraries, nuclear-data libraries and HDF5 files).

Usage: package_scan.py PACKAGE_DIR. Exit status 1 and one line per finding.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

# Programs the download holds, by path relative to the package, without a Windows ".exe" suffix.
ALLOWED_PROGRAMS = {"bin/faris", "bin/faris-app", "bin/avila-core"}
MAGIC = {
    b"\x7fELF": "ELF",
    b"MZ": "PE",
    b"\xfe\xed\xfa\xce": "Mach-O",
    b"\xfe\xed\xfa\xcf": "Mach-O",
    b"\xce\xfa\xed\xfe": "Mach-O",
    b"\xcf\xfa\xed\xfe": "Mach-O",
    b"\xca\xfe\xba\xbe": "Mach-O universal",
    b"\x89HDF": "HDF5",
}
RESTRICTED_NAMES = re.compile(r"openmc|dagmc|moab|double[-_]?down|libTK|libopencascade|fendl|endf|\.h5$|\.h5m$|\.hdf5$", re.I)


def kind_of(path: Path) -> str | None:
    with path.open("rb") as handle:
        head = handle.read(4)
    for magic, kind in MAGIC.items():
        if head.startswith(magic):
            # "MZ" is two letters; a text file that starts with MZ is rare but possible, so also need a PE header offset.
            if kind == "PE" and not _is_pe(path):
                return None
            return kind
    return None


def _is_pe(path: Path) -> bool:
    with path.open("rb") as handle:
        data = handle.read(0x40)
        if len(data) < 0x40:
            return False
        offset = int.from_bytes(data[0x3C:0x40], "little")
        handle.seek(offset)
        return handle.read(4) == b"PE\0\0"


def scan(package: Path) -> list[str]:
    """One message per file that is not allowed in a download."""
    findings = []
    for path in sorted(package.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(package).as_posix()
        if RESTRICTED_NAMES.search(path.name):
            findings.append(f"{relative}: name matches a tool or data that is never bundled")
            continue
        kind = kind_of(path)
        if kind and re.sub(r"\.exe$", "", relative) not in ALLOWED_PROGRAMS:
            findings.append(f"{relative}: {kind} binary that is not a listed FARIS program")
    return findings


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    findings = scan(Path(argv[1]))
    for line in findings:
        print(line)
    print(f"package scan: {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
