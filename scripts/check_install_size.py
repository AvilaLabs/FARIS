#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""The installed application stays small (requirements PERF-045, PLAT-026).

    python3 scripts/check_install_size.py out [--limit-mb 150] [--time-install] [--limit-s 60]

Adds up the size of every file under the folder a platform build collects (the programs and the licence
notices; nuclear data and sample results are never in it) and fails when the total is over the limit.
With --time-install it also installs: the folder is copied into a fresh folder of the temporary directory and every
copied file is flushed to disk, which is all that installing the portable build is; it fails when that takes more than
--limit-s seconds (PLAT-026, 60 s on an SSD). The desktop workflow runs it on each release-profile build.
"""
from __future__ import annotations

import argparse
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

LIMIT_MB = 150
LIMIT_S = 60


def total_bytes(root: Path) -> int:
    if not root.is_dir():
        raise SystemExit(f"{root} is not a folder")
    return sum(path.stat().st_size for path in root.rglob("*") if path.is_file())


def install_seconds(root: Path) -> float:
    """Seconds to copy the folder into a fresh temporary folder and flush every file to disk."""
    if not root.is_dir():
        raise SystemExit(f"{root} is not a folder")
    with tempfile.TemporaryDirectory(prefix="faris-install-") as target:
        start = time.perf_counter()
        installed = Path(target) / "faris"
        shutil.copytree(root, installed)
        for path in installed.rglob("*"):
            if path.is_file():
                fd = os.open(path, os.O_RDONLY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)
        return time.perf_counter() - start


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--limit-mb", type=float, default=LIMIT_MB)
    parser.add_argument("--time-install", action="store_true", help="also time the copy into a fresh folder")
    parser.add_argument("--limit-s", type=float, default=LIMIT_S)
    args = parser.parse_args()
    size = total_bytes(args.folder)
    megabytes = size / 1e6
    print(f"{args.folder}: {megabytes:.1f} MB (limit {args.limit_mb:g} MB)")
    if megabytes > args.limit_mb:
        print("The installed application is over the size limit.", file=sys.stderr)
        return 1
    if args.time_install:
        seconds = install_seconds(args.folder)
        print(f"{args.folder}: installed in {seconds:.2f} s (limit {args.limit_s:g} s)")
        if seconds > args.limit_s:
            print("Installing the application took longer than the limit.", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
