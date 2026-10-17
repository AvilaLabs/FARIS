#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint against fission accuracy figures used as FARIS acceptance numbers (requirement VAL-081).

    python3 scripts/fission_figure_lint.py [--root DIR]

Reactivity in pcm, critical boron in ppm and nodal power rms are fission figures measured against decades of
plant data; fusion has no such data, so they are never a target for a fusion quantity. The lint scans

- the Target/acceptance column of every requirement table (docs/requirements/*.md), where such a figure is
  allowed only in a sentence that labels it an example (VAL-080 quotes the CMS5 statistics that way);
- the Markdown documents users read and the Rust string literals of the crates (the wording lint's scope),
  where such a figure is allowed only in a sentence that labels it as fission context ("fission" or "context").

docs/requirements/ prose and docs/requirements/research/ quote the figures in order to forbid them and are not
scanned. Exit status 1 on any finding. Output: file:line: VAL-081: text.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import requirement_trace as rt  # noqa: E402
import wording_lint  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FIGURE = re.compile(
    r"\bpcm\b|\bppm\s?B\b|\bppm\b[^.;|]{0,24}\bboron\b|\bboron\b[^.;|]{0,24}\bppm\b|\bnodal\s+power\b",
    re.IGNORECASE)
CONTEXT = re.compile(r"\bfission\b|\bcontext\b|\bPWR\b|\bexample\b", re.IGNORECASE)


def acceptance_findings(root: Path) -> list[str]:
    out = []
    for path in sorted((root / "docs" / "requirements").glob("[0-9][0-9]-*.md")):
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if not rt.ROW.match(line):
                continue
            cells = [c.strip() for c in rt.CELL_SPLIT.split(line.strip())[1:-1]]
            if len(cells) >= 3 and cells[0] != "VAL-081":
                for sentence in wording_lint.sentences(cells[2]):
                    if (m := FIGURE.search(sentence[0])) and not CONTEXT.search(sentence[0]):
                        out.append(f"{path.relative_to(root)}:{number}: VAL-081: {m.group(0)!r} in the acceptance of {cells[0]}")
    return out


def text_findings(name: str, units: list[tuple[str, int]]) -> list[str]:
    out = []
    for text, first_line in units:
        for sentence, offset in wording_lint.sentences(text):
            for m in FIGURE.finditer(sentence):
                if not CONTEXT.search(sentence):
                    line = first_line + text.count("\n", 0, offset + m.start())
                    out.append(f"{name}:{line}: VAL-081: {m.group(0)!r} outside a sentence that labels it fission context")
    return out


def run(root: Path = ROOT) -> list[str]:
    findings = acceptance_findings(root)
    seen: set[Path] = set()
    for pattern in wording_lint.MARKDOWN:
        for path in sorted(root.glob(pattern)):
            if path in seen or path.name == "THIRD_PARTY_NOTICES.md":
                continue
            seen.add(path)
            findings += text_findings(str(path.relative_to(root)), wording_lint.markdown_units(path.read_text(encoding="utf-8")))
    for pattern in wording_lint.RUST_DIRS:
        for src in sorted(root.glob(pattern)):
            for rs in sorted(src.rglob("*.rs")):
                findings += text_findings(str(rs.relative_to(root)), wording_lint.rust_strings(rs.read_text(encoding="utf-8")))
    return findings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="lint against fission accuracy figures as acceptance numbers")
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    findings = run(args.root)
    for f in findings:
        print(f)
    print(f"fission figure lint: {'clean' if not findings else str(len(findings)) + ' finding(s)'}", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
