#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Licence header check (requirement LEG-002), the same test as `reuse lint` for in-file headers.

Every tracked source or script file (Rust, Python, shell, PowerShell, JavaScript, WGSL) must carry an
`SPDX-License-Identifier: AGPL-3.0-only` line and an `SPDX-FileCopyrightText:` line in its first lines.
Exit status 1 and a list of file names when any file lacks one.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SUFFIXES = (".rs", ".py", ".sh", ".ps1", ".mjs", ".js", ".wgsl")
LICENCE = "SPDX-License-Identifier: AGPL-3.0-only"
COPYRIGHT = "SPDX-FileCopyrightText:"
HEADER_LINES = 8


def tracked_files(root: Path = ROOT) -> list[str]:
    out = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True).stdout
    return [f for f in out.splitlines() if f.endswith(SUFFIXES)]


def problems(files: list[str], root: Path = ROOT) -> list[str]:
    found = []
    for name in files:
        head = "\n".join((root / name).read_text(encoding="utf-8").splitlines()[:HEADER_LINES])
        if LICENCE not in head:
            found.append(f"{name}: no '{LICENCE}' line")
        if COPYRIGHT not in head:
            found.append(f"{name}: no '{COPYRIGHT}' line")
    return found


def main() -> int:
    files = tracked_files()
    found = problems(files)
    for line in found:
        print(line)
    print(f"{len(files)} source and script files checked, {len(found)} problems")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
