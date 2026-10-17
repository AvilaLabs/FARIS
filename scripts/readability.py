#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Readability gate for the handbook prose (requirement DOC-053).

Tool: this script, `fk-grade` version 1. Flesch-Kincaid grade level = 0.39 * (words / sentences)
+ 11.8 * (syllables / words) - 15.59, with syllables counted by the vowel-group rule below. Only prose is
measured: fenced and inline code, tables, headings, link targets, HTML and URLs are removed first.

Gates, over the handbook pages under docs/guide (kind from the first-line label):
  per page      Flesch-Kincaid grade <= 12, tutorials <= 10 (pages with fewer than MIN_SENTENCES sentences
                are not scored)
  whole manual  mean sentence <= 20 words; passive voice at most PASSIVE_CEILING of sentences (the target
                is 15 %; the measured value is above it, so the ceiling is a ratchet that only goes down)
Passive voice is a form of "to be" followed by a past participle ("is stored", "was written"); the
heuristic over-counts a little.

Usage: readability.py [--report]. Exit status 1 when a gate fails.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOL = "fk-grade 1"
MIN_SENTENCES = 5
GRADE = {"tutorial": 10.0}
DEFAULT_GRADE = 12.0
MAX_MEAN_SENTENCE = 20.0
MAX_PASSIVE = 0.15
# The heuristic measures 0.179 on the manual today against the 0.15 target; the ceiling only ratchets down.
PASSIVE_CEILING = 0.185
BE = r"(?:is|are|was|were|be|been|being)"
PASSIVE = re.compile(r"\b" + BE + r"\s+(?:\w+ly\s+)?(?:\w+ed|written|made|shown|given|taken|chosen|run|kept|built|held|set|read|seen|known|found|lost|done|sent|left)\b", re.I)


def prose(text: str) -> str:
    """The sentences of a Markdown page, without code, tables, headings, links' targets and HTML."""
    out = []
    fence = None
    for line in text.splitlines():
        marker = re.match(r"\s*(`{3,}|~{3,})", line)
        if fence:
            if marker and marker.group(1)[0] == fence[0]:
                fence = None
            continue
        if marker:
            fence = marker.group(1)
            continue
        stripped = line.strip()
        if not stripped or stripped.startswith(("|", "#", "<!--", "![", "$$")) or line.startswith("    "):
            out.append("")
            continue
        stripped = re.sub(r"^([-*+]|\d+[.)])\s+", "", stripped)
        stripped = re.sub(r"`[^`]*`", "x", stripped)
        stripped = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", stripped)
        stripped = re.sub(r"<[^>]+>", " ", stripped)
        stripped = re.sub(r"https?://\S+", "x", stripped)
        stripped = re.sub(r"[*_]{1,3}", "", stripped)
        out.append(stripped)
    return "\n".join(out)


def sentences(text: str) -> list[str]:
    found = []
    for paragraph in re.split(r"\n\s*\n", text):
        flat = " ".join(paragraph.split())
        for sentence in re.split(r"(?<=[.!?])\s+(?=[A-Z0-9\"(])", flat):
            if len(re.findall(r"[A-Za-z]+", sentence)) >= 3:
                found.append(sentence)
    return found


def syllables(word: str) -> int:
    word = word.lower()
    word = re.sub(r"(?:[^laeiouy]es|[^laeiouy]ed|[^laeiouy]e)$", "", word)
    word = re.sub(r"^y", "", word)
    return max(len(re.findall(r"[aeiouy]{1,2}", word)), 1)


def measure(text: str) -> dict:
    sents = sentences(prose(text))
    words = [w for s in sents for w in re.findall(r"[A-Za-z]+(?:'[a-z]+)?", s)]
    if not sents or not words:
        return {"sentences": 0, "words": 0, "grade": 0.0, "mean_sentence": 0.0, "passive": 0.0}
    syl = sum(syllables(w) for w in words)
    grade = 0.39 * len(words) / len(sents) + 11.8 * syl / len(words) - 15.59
    passive = sum(1 for s in sents if PASSIVE.search(s)) / len(sents)
    return {"sentences": len(sents), "words": len(words), "grade": round(grade, 2),
            "mean_sentence": round(len(words) / len(sents), 2), "passive": round(passive, 3)}


def kind_of(text: str) -> str:
    match = re.match(r"<!-- kind: ([a-z-]+) -->", text)
    return match.group(1) if match else "reference"


def check(root: Path = ROOT) -> tuple[list[str], dict]:
    """Per-page grade gate and manual-wide sentence-length and passive-voice gates."""
    problems, report = [], {}
    for page in sorted((root / "docs/guide").glob("*.md")):
        name = page.name
        if name in ("SUMMARY.md", "cli-reference.md"):
            continue
        text = page.read_text(encoding="utf-8")
        m = measure(text)
        report[name] = m
        limit = GRADE.get(kind_of(text), DEFAULT_GRADE)
        if m["sentences"] >= MIN_SENTENCES and m["grade"] > limit:
            problems.append(f"{name}: grade {m['grade']} is above {limit}")
    sentences_total = sum(m["sentences"] for m in report.values())
    if sentences_total:
        mean = sum(m["words"] for m in report.values()) / sentences_total
        passive = sum(m["passive"] * m["sentences"] for m in report.values()) / sentences_total
        report["(manual)"] = {"sentences": sentences_total, "mean_sentence": round(mean, 2), "passive": round(passive, 3)}
        if mean > MAX_MEAN_SENTENCE:
            problems.append(f"manual: mean sentence {mean:.2f} words is above {MAX_MEAN_SENTENCE}")
        if passive > PASSIVE_CEILING:
            problems.append(f"manual: passive voice {passive:.3f} is above the recorded ceiling {PASSIVE_CEILING}")
    return problems, report


def main(argv: list[str]) -> int:
    problems, report = check()
    if "--report" in argv:
        for name, m in report.items():
            print(f"{name:32} {m}")
    for line in problems:
        print(line)
    print(f"readability ({TOOL}): {len(report)} pages, {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
