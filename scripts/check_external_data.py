#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Checks that data fetched from outside the repository is fetched by hash (requirement QA-090).

    python3 scripts/check_external_data.py [--root DIR]

scripts/external_data.json lists every dataset too large for the repository: its URL, a SHA-256 or MD5 (or a
per-file SHA-256 for an archive), and the files that verify it. The check fails when
- an entry has no https URL, no well-formed hash, or no verifying file that exists and contains each hash;
- a curl or wget command, `urlretrieve` call or `urlopen` of a literal URL in a workflow, script, document or
  integration names a URL that is neither an entry nor a listed tool installer;
- an entry's URL appears nowhere it could be fetched from (a stale entry).
Reads files only.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = Path("scripts") / "external_data.json"
SCANNED = (".github", "scripts", "docs", "integrations", "validation", "controls", "README.md", "CONTRIBUTING.md")
SUFFIXES = (".yml", ".yaml", ".py", ".sh", ".md")
FETCH = re.compile(r"(?:\bcurl\b|\bwget\b|urlretrieve\(|urlopen\()[^\n|]*?(https?://[^\s\"'\\)]+)")
HASHES = {"sha256": re.compile(r"[0-9a-f]{64}"), "md5": re.compile(r"[0-9a-f]{32}")}


def scanned_files(root: Path):
    for name in SCANNED:
        base = root / name
        paths = [base] if base.is_file() else sorted(base.rglob("*")) if base.is_dir() else []
        for path in paths:
            if path.is_file() and path.suffix in SUFFIXES and "target" not in path.relative_to(root).parts:
                yield path


def fetched_urls(root: Path) -> dict[str, list[str]]:
    """{url: [files that fetch it]} for every literal URL a fetch command or call names."""
    found: dict[str, list[str]] = {}
    for path in scanned_files(root):
        if path.name == "check_external_data.py" or path.name.startswith("test_"):  # tests quote URLs as fixtures
            continue
        for match in FETCH.finditer(path.read_text(encoding="utf-8", errors="replace")):
            found.setdefault(match.group(1).rstrip(".,;"), []).append(path.relative_to(root).as_posix())
    return found


def entry_hashes(entry: dict) -> list[tuple[str, str]]:
    out = [(kind, entry[kind]) for kind in HASHES if kind in entry]
    out += [("sha256", digest) for digest in (entry.get("file_sha256") or {}).values()]
    return out


def problems(root: Path, manifest: dict | None = None) -> list[str]:
    manifest = manifest if manifest is not None else json.loads((root / MANIFEST).read_text(encoding="utf-8"))
    found: list[str] = []
    known = set()
    for entry in manifest.get("data", []):
        name = entry.get("name") or "(no name)"
        url = str(entry.get("url") or "")
        known.add(url)
        if not url.startswith("https://"):
            found.append(f"{name}: url must be https")
        hashes = entry_hashes(entry)
        if not hashes:
            found.append(f"{name}: no sha256, md5 or file_sha256 hash recorded")
        for kind, digest in hashes:
            if not HASHES[kind].fullmatch(str(digest)):
                found.append(f"{name}: {digest!r} is not a {kind} digest")
        files = entry.get("verified_in") or []
        if not files:
            found.append(f"{name}: verified_in names no file that checks the hash")
        for relative in files:
            path = root / relative
            if not path.is_file():
                found.append(f"{name}: verifying file {relative} does not exist")
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            found += [f"{name}: {relative} does not contain the {kind} {digest}" for kind, digest in hashes if digest not in text]
    tools = {t.get("url") for t in manifest.get("tools", [])}
    fetched = fetched_urls(root)
    for url, files in sorted(fetched.items()):
        if url not in known and url not in tools:
            found.append(f"{url} is fetched in {', '.join(sorted(set(files)))} but is not in {MANIFEST}; record its hash there")
    for url in sorted(known - set(fetched)):
        found.append(f"{url} is listed in {MANIFEST} but no workflow, script or document fetches it; remove the entry")
    return found


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    found = problems(args.root)
    for line in found:
        print(f"check_external_data: {line}", file=sys.stderr)
    if not found:
        print("check_external_data: every fetched dataset has a recorded hash")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
