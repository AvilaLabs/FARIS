#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""One command rebuilds the whole handbook, and proves the build is reproducible (requirement DOC-036).

    python3 scripts/build_docs.py [--out DIR]

It checks that every file the book reads (the pages under docs/guide and the scripts named in
book.toml) is tracked in version control, builds the book with `mdbook build` twice into two fresh
folders, and requires both outputs to have the same tree hash (SHA-256 over sorted relative paths and
file hashes). With --out DIR the verified build is also copied to DIR. Prints the tree hash.
Needs mdbook (CI pins 0.5.4); exit status 1 on any difference or untracked source, 2 without mdbook.
"""
from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def tree_hash(directory: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        digest.update(path.relative_to(directory).as_posix().encode() + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def book_sources(root: Path = ROOT) -> list[str]:
    """Files the book reads, relative to the repository: every file under `src` and the additional scripts."""
    book = (root / "book.toml").read_text(encoding="utf-8")
    src = re.search(r'^src\s*=\s*"([^"]+)"', book, re.M).group(1)
    files = [p.relative_to(root).as_posix() for p in sorted((root / src).rglob("*")) if p.is_file()]
    for match in re.finditer(r'additional-(?:js|css)\s*=\s*\[(.*?)\]', book, re.S):
        files += re.findall(r'"([^"]+)"', match.group(1))
    return files


def untracked(files: list[str], root: Path = ROOT) -> list[str]:
    out = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True).stdout.splitlines()
    tracked = set(out)
    return [f for f in files if f not in tracked]


def build(destination: Path) -> None:
    subprocess.run(["mdbook", "build", "-d", str(destination)], cwd=ROOT, check=True, capture_output=True, text=True)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="rebuild the handbook and check the build is reproducible")
    ap.add_argument("--out", help="copy the verified build to this folder")
    args = ap.parse_args(argv)
    if shutil.which("mdbook") is None:
        print("build_docs: mdbook is not installed (cargo install mdbook --version 0.5.4 --locked)", file=sys.stderr)
        return 2
    missing = untracked(book_sources())
    if missing:
        print("build_docs: book content outside version control: " + ", ".join(missing), file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory() as tmp:
        first, second = Path(tmp) / "first", Path(tmp) / "second"
        build(first)
        build(second)
        a, b = tree_hash(first), tree_hash(second)
        if a != b:
            print(f"build_docs: two builds of the same input differ: {a} != {b}", file=sys.stderr)
            return 1
        if args.out:
            out = Path(args.out)
            if out.exists():
                shutil.rmtree(out)
            shutil.copytree(first, out)
    print(f"handbook rebuilt twice, identical tree hash {a}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
