#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The installed application stays small (requirements PERF-045, PLAT-026).

    python3 scripts/check_install_size.py out [--limit-mb 150]

Adds up the size of every file under the folder a platform build collects (the programs and the licence
notices; nuclear data and sample results are never in it) and fails when the total is over the limit.
The desktop workflow runs it on each release-profile build.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

LIMIT_MB = 150


def total_bytes(root: Path) -> int:
    if not root.is_dir():
        raise SystemExit(f"{root} is not a folder")
    return sum(path.stat().st_size for path in root.rglob("*") if path.is_file())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--limit-mb", type=float, default=LIMIT_MB)
    args = parser.parse_args()
    size = total_bytes(args.folder)
    megabytes = size / 1e6
    print(f"{args.folder}: {megabytes:.1f} MB (limit {args.limit_mb:g} MB)")
    if megabytes > args.limit_mb:
        print("The installed application is over the size limit.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
