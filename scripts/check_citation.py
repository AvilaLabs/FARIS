#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Validate CITATION.cff against Citation File Format 1.2.0 (requirements PRV-036, DOC-060).

    python3 scripts/check_citation.py

cffconvert is the reference validator but needs network access to install, so this checks, with the standard
library only, the rules of the CFF 1.2.0 schema that apply to a software citation: required keys, value types,
the date and version forms, an SPDX-looking licence, authors with a name, a DOI of the right shape when given.
It also checks what the schema cannot know: version equals the workspace version in Cargo.toml, and the
"How to cite" page (docs/guide/cite.md) shows the same title, version, author and licence as the file.
The file is a small flat YAML subset; the reader here parses only that subset and refuses anything else.
Exit status 1 on any problem.
"""
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CFF = Path("CITATION.cff")
PAGE = Path("docs") / "guide" / "cite.md"
TYPES = {"software", "dataset"}
DOI = re.compile(r"^10\.\d{4,9}/[^\s]+$")
SEMVER = re.compile(r"^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$")
SPDX = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.+-]*$")
URL = re.compile(r"^https?://[^\s]+$")
REQUIRED = ("cff-version", "message", "title", "authors")


def unquote(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1]
    return value


def read_yaml(text: str) -> dict:
    """Top-level scalars, lists of scalars, and lists of flat mappings; nothing deeper."""
    data: dict = {}
    key = None
    for number, raw in enumerate(text.splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip())
        line = raw.strip()
        if indent == 0:
            match = re.fullmatch(r"([A-Za-z-]+):\s*(.*)", line)
            if not match:
                raise ValueError(f"line {number}: cannot read {line!r}")
            key, value = match.groups()
            if key in data:
                raise ValueError(f"line {number}: duplicate key {key}")
            data[key] = unquote(value) if value else []
        elif isinstance(data.get(key), list):
            if line.startswith("- "):
                item = line[2:].strip()
                pair = re.fullmatch(r"([A-Za-z-]+):\s*(.*)", item)
                data[key].append({pair.group(1): unquote(pair.group(2))} if pair and not item.startswith(("\"", "'")) else unquote(item))
            else:
                pair = re.fullmatch(r"([A-Za-z-]+):\s*(.*)", line)
                if not pair or not data[key] or not isinstance(data[key][-1], dict):
                    raise ValueError(f"line {number}: cannot read {line!r}")
                data[key][-1][pair.group(1)] = unquote(pair.group(2))
        else:
            raise ValueError(f"line {number}: unexpected indentation")
    return data


def problems_in(cff: dict, workspace_version: str, page: str | None) -> list[str]:
    problems = [f"missing required key {k}" for k in REQUIRED if not cff.get(k)]
    if cff.get("cff-version") != "1.2.0":
        problems.append("cff-version must be 1.2.0")
    if cff.get("type", "software") not in TYPES:
        problems.append("type must be software or dataset")
    authors = cff.get("authors")
    if isinstance(authors, list):
        for a in authors:
            if not isinstance(a, dict) or not (a.get("name") or (a.get("family-names") and a.get("given-names"))):
                problems.append(f"author {a!r} needs name, or family-names and given-names")
    elif authors:
        problems.append("authors must be a list")
    version = str(cff.get("version", ""))
    if version and not SEMVER.match(version):
        problems.append(f"version {version!r} is not a semantic version")
    if version and version != workspace_version:
        problems.append(f"version {version} differs from the workspace version {workspace_version}; update CITATION.cff with the release")
    if not version:
        problems.append("version is required for a software citation here (a release identifier suitable for a DOI)")
    released = str(cff.get("date-released", ""))
    if released and not re.fullmatch(r"\d{4}-\d{2}-\d{2}", released):
        problems.append("date-released must be YYYY-MM-DD")
    if not released:
        problems.append("date-released is required")
    licence = str(cff.get("license", ""))
    if licence and not SPDX.match(licence):
        problems.append(f"license {licence!r} is not an SPDX identifier")
    for key in ("repository-code", "url"):
        if cff.get(key) and not URL.match(str(cff[key])):
            problems.append(f"{key} is not an http(s) URL")
    if cff.get("doi") and not DOI.match(str(cff["doi"])):
        problems.append("doi has the wrong shape (expected 10.NNNN/suffix)")
    if page is not None:
        a = authors[0] if isinstance(authors, list) and authors and isinstance(authors[0], dict) else {}
        author = a.get("name") or f"{a.get('given-names', '')} {a.get('family-names', '')}".strip()
        for what, text in (("title", cff.get("title")), ("version", version), ("author", author), ("licence", licence)):
            if text and str(text) not in page:
                problems.append(f"the How to cite page does not show the CITATION.cff {what} ({text})")
    return problems


def workspace_version(root: Path) -> str:
    return tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]


def main(argv: list[str] | None = None) -> int:
    root = Path(argv[0]) if argv else ROOT
    try:
        cff = read_yaml((root / CFF).read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"CITATION.cff: {e}", file=sys.stderr)
        return 1
    page = (root / PAGE).read_text(encoding="utf-8") if (root / PAGE).is_file() else None
    problems = problems_in(cff, workspace_version(root), page)
    if page is None:
        problems.append(f"{PAGE} does not exist")
    if problems:
        print("\n".join("CITATION.cff: " + p for p in problems), file=sys.stderr)
        return 1
    print(f"CITATION.cff: valid CFF 1.2.0, version {cff['version']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
