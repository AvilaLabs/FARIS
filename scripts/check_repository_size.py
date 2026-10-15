#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Checks that the files tracked by version control stay small and hold no nuclear data or archives (QA-090).

Fails when the tracked files add up to more than 50 MB, when one file is larger than 10 MB, or when a
tracked file has a suffix that belongs to downloaded nuclear data or to an archive, unless it is small
nuclear data listed with its provenance in nuclear_data_in_git.json (see nuclear_data_policy.py). Larger
data is fetched by hash and kept out of the repository (AGENTS.md).
"""
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

import nuclear_data_policy

ROOT = Path(__file__).resolve().parent.parent
MAX_TOTAL_BYTES = 50 * 1000 * 1000
MAX_FILE_BYTES = 10 * 1000 * 1000
FORBIDDEN_SUFFIXES = (".h5", ".hdf5", ".endf", ".zip", ".tar", ".gz", ".xz", ".tgz", ".7z")
# Small nuclear data whose provider allows redistribution, each with its provenance (QA-090).
ALLOWED = nuclear_data_policy.allowed()


def check(sizes: dict[str, int]) -> list[str]:
    """Returns one sentence per problem for a {tracked path: bytes} map."""
    problems = []
    for path, size in sorted(sizes.items()):
        if path.lower().endswith(FORBIDDEN_SUFFIXES) and path not in ALLOWED:
            problems.append(f"{path} looks like downloaded data or an archive; keep it out of the repository "
                            f"and fetch it by hash")
        if size > MAX_FILE_BYTES:
            problems.append(f"{path} is {size} bytes, over the {MAX_FILE_BYTES} byte limit for one file; "
                            f"store it outside the repository and fetch it by hash")
    total = sum(sizes.values())
    if total > MAX_TOTAL_BYTES:
        problems.append(f"tracked files add up to {total} bytes, over the {MAX_TOTAL_BYTES} byte limit; "
                        f"move the largest fixtures out of the repository")
    return problems


def tracked_sizes(root: Path) -> dict[str, int]:
    listing = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], check=True, capture_output=True).stdout
    sizes = {}
    for name in listing.decode("utf-8").split("\0"):
        path = root / name
        if name and path.is_file() and not path.is_symlink():
            sizes[name] = path.stat().st_size
    return sizes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    sizes = tracked_sizes(args.root)
    problems = check(sizes) + nuclear_data_policy.problems(args.root)
    for problem in problems:
        print(f"check_repository_size: {problem}", file=sys.stderr)
    print(f"{len(sizes)} tracked files, {sum(sizes.values())} bytes")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
