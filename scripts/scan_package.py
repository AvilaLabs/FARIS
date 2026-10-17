#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Package scan for restricted tools and nuclear data (requirements SEC-072, LEG-016).

    python3 scripts/scan_package.py FOLDER_OR_ARCHIVE [...]

Looks at every file in a release folder (the one a platform build collects, or a recorded-demo package) or archive
(.zip, .tar, .tar.gz, .tgz) and fails on
- a program of a tool FARIS must not bundle (OpenMC, Avila Core, ACTINV, MCNP, Serpent, SCALE, Cubit and the like),
  by file name;
- a nuclear data file (HDF5 or ACE libraries, ENDF evaluations, an OpenMC cross_sections.xml, xsdir files) that is not
  in the data manifest scripts/nuclear_data_in_git.json with its SHA-256, provider, source, terms, evidence for the
  terms and attribution (the fields scripts/nuclear_data_policy.py demands). Data fetched by hash is never in a package.
The manifest holds only data whose provider allows redistribution, so a listed file is one that may ship.
"""
from __future__ import annotations

import argparse
import hashlib
import re
import sys
import tarfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import nuclear_data_policy  # noqa: E402

RESTRICTED_TOOLS = re.compile(
    r"^(openmc(-[\w.]+)?|avila[-_]core(-[\w.]+)?|actinv(-[\w.]+)?|mcnp\d*[\w.]*|serpent\d*|scale\d*|cubit|trelis|"
    r"fluka|geant4|tripoli[\w-]*|ansys[\w-]*|comsol[\w-]*)(\.exe|\.bin)?$", re.I)
DATA_FILE = re.compile(r"(\.(h5|hdf5|ace|acer|endf|endf6|xsdir|xsdata)$|^cross_sections(_[\w-]+)?\.xml$|^xsdir$|\.endf\.[\w]+$)", re.I)
INDEX_FILE = re.compile(r"^cross_sections(_[\w-]+)?\.xml$", re.I)


def walk_folder(folder: Path):
    for path in sorted(folder.rglob("*")):
        if path.is_file():
            yield path.relative_to(folder).as_posix(), path.read_bytes
    return


def walk_archive(archive: Path):
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as z:
            for info in z.infolist():
                if not info.is_dir():
                    yield info.filename, (lambda i=info: z.open(i).read())
        return
    with tarfile.open(archive) as t:
        for member in t.getmembers():
            if member.isfile():
                yield member.name, (lambda m=member: t.extractfile(m).read())


def manifest_digests(registry: Path = nuclear_data_policy.REGISTRY) -> dict[str, str]:
    """sha256 -> path, for every listed data file that names its terms and credit in full."""
    out = {}
    for entry in nuclear_data_policy.load(registry)["files"]:
        if entry.get("kind") == "data" and all(entry.get(f) for f in nuclear_data_policy.DATA_FIELDS):
            out[entry["sha256"]] = entry["path"]
    return out


def scan_entries(entries, label: str, allowed: dict[str, str]) -> list[str]:
    problems = []
    for name, read in entries:
        base = name.rsplit("/", 1)[-1]
        if RESTRICTED_TOOLS.match(base):
            problems.append(f"{label}: {name} is a program of a restricted or separately licensed tool and is not bundled")
        elif DATA_FILE.search(base):
            if INDEX_FILE.match(base):
                problems.append(f"{label}: {name} is a nuclear data library index; no library ships in a package")
            elif hashlib.sha256(read()).hexdigest() not in allowed:
                problems.append(f"{label}: {name} is a nuclear data file that is not in the data manifest "
                                f"({nuclear_data_policy.REGISTRY.name}) with its terms, so it may not ship")
    return problems


def scan(target: Path, allowed: dict[str, str] | None = None) -> list[str]:
    allowed = manifest_digests() if allowed is None else allowed
    if target.is_dir():
        return scan_entries(walk_folder(target), str(target), allowed)
    if target.is_file():
        return scan_entries(walk_archive(target), str(target), allowed)
    return [f"{target} does not exist"]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("targets", nargs="+", type=Path)
    args = ap.parse_args(argv)
    problems = []
    for target in args.targets:
        problems += scan(target)
    for p in problems:
        print(p)
    print(f"package scan: {len(args.targets)} target(s), {len(problems)} problem(s)", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
