#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Assemble the faris.avilalabs.org site: the built handbook at /docs/.

Until the browser version is published, the site root redirects to the
handbook. Deploy with `npx wrangler@4 deploy --config wrangler.jsonc`.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]


def prepare(destination: Path, docs: Path) -> None:
    if not (docs / "index.html").is_file():
        raise ValueError(f"Handbook is not built at {docs}; run mdbook build first")
    source = docs.resolve()
    target = (destination / "docs").resolve()
    if source.is_relative_to(target) or target.is_relative_to(source):
        raise ValueError("Handbook source and packaged destination must not overlap")
    destination.mkdir(parents=True, exist_ok=True)
    # Replacing the generated directory removes chapters retired since the
    # previous build, including their old search index and assets.
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(source, target)
    (destination / "_redirects").write_text("/ /docs/ 302\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, nargs="?", default=ROOT / "dist/web")
    parser.add_argument("--docs", type=Path, default=ROOT / "dist/docs")
    args = parser.parse_args()
    prepare(args.destination, args.docs)
