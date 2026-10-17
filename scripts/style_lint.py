#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Style lint for the prose (requirement DOC-054): British spelling and one name per thing.

Scans the prose of README.md, CONTRIBUTING.md, CHANGELOG.md, the handbook (docs/guide) and the documents
under docs/ that users read. Code blocks, inline code, link targets, HTML tags and URLs are not prose and
are skipped. The word lists are in scripts/style_lint_terms.json (versioned):

  spelling     American spellings with their British form (color -> colour)
  terminology  a variant name with the one name to use (for example "cross-section" -> "cross section")

An exact-sentence allow list lets a quotation keep its original spelling; an entry that matches nothing is
itself an error, so the list cannot rot. Output: file:line: STYLE: word -> use. Exit status 1 on any finding.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TERMS = Path(__file__).with_name("style_lint_terms.json")
DOCUMENTS = ["README.md", "CONTRIBUTING.md", "CHANGELOG.md", "docs/*.md", "docs/guide/*.md"]
# Third-party licence texts and generated listings (the CLI reference, the known-issues page) are not our prose.
SKIPPED = {
    "docs/THIRD_PARTY_NOTICES.md", "docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md", "docs/guide/cli-reference.md",
    "docs/KNOWN_ISSUES.md",
}


def prose_lines(text: str) -> list[tuple[int, str]]:
    """(line number, text) of prose lines: fenced code, inline code, URLs, link targets and tags removed."""
    out = []
    fence = None
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.lstrip()
        marker = re.match(r"(`{3,}|~{3,})", stripped)
        if fence:
            if marker and marker.group(1)[0] == fence[0] and len(marker.group(1)) >= len(fence):
                fence = None
            continue
        if marker:
            fence = marker.group(1)
            continue
        if line.startswith("    ") and not stripped.startswith(("-", "*", "|")):
            continue
        line = re.sub(r"`[^`]*`", " ", line)
        line = re.sub(r"\]\([^)]*\)", "]", line)
        line = re.sub(r"<[^>]+>", " ", line)
        line = re.sub(r"https?://\S+", " ", line)
        line = re.sub(r"\[[^\]]*\]:\s*\S+", " ", line)
        out.append((number, line))
    return out


def sentence_of(line: str, start: int) -> str:
    found = line.rfind(". ", 0, start)
    left = found + 2 if found != -1 else 0
    right = line.find(". ", start)
    return re.sub(r"\s+", " ", line[left:right + 1 if right != -1 else None]).strip()


def lint_text(path: str, text: str, terms: dict, used: set) -> list[str]:
    allowed = {re.sub(r"\s+", " ", e["sentence"]).strip(): e for e in terms.get("allow_list", [])}
    patterns = [(re.compile(r"(?<![A-Za-z0-9_-])" + w + r"(?![A-Za-z0-9_-])", re.I), f"{w.replace('|', '/')}", use)
                for group in ("spelling", "terminology") for w, use in terms[group].items()]
    findings = []
    for number, line in prose_lines(text):
        for regex, word, use in patterns:
            for match in regex.finditer(line):
                sentence = sentence_of(line, match.start())
                if sentence in allowed:
                    used.add(sentence)
                    continue
                findings.append(f"{path}:{number}: STYLE: {match.group(0)} -> use {use}")
    return findings


def run(root: Path = ROOT, terms: dict | None = None) -> list[str]:
    terms = terms or json.loads(TERMS.read_text(encoding="utf-8"))
    used: set = set()
    findings = []
    seen = set()
    for pattern in DOCUMENTS:
        for path in sorted(root.glob(pattern)):
            name = path.relative_to(root).as_posix()
            if name in seen or name in SKIPPED:
                continue
            seen.add(name)
            findings += lint_text(name, path.read_text(encoding="utf-8"), terms, used)
    for entry in terms.get("allow_list", []):
        if re.sub(r"\s+", " ", entry["sentence"]).strip() not in used:
            findings.append(f"scripts/style_lint_terms.json:1: STYLE: unused allow-list entry: {entry['sentence'][:80]}")
    return findings


def main() -> int:
    findings = run()
    for line in findings:
        print(line)
    print(f"style lint: {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
