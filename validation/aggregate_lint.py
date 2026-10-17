# SPDX-License-Identifier: AGPL-3.0-only
"""Cross-document lint for C/E aggregates (requirement VAL-035).

    python3 -m validation.aggregate_lint [--root DIR] [FILE...]

No document may carry one C/E number across response classes, libraries or code versions. Three checks:

- text: in the Markdown documents users read and in any FILE (a report.md, an export, a page), a sentence that
  speaks of C/E with an aggregating word (overall, headline, combined, pooled, grand, global, aggregate, average
  across, "across all/every class, library or code version") is a finding unless a negation word comes before it;
- report Markdown: a C/E distribution table sits only under a "### Response class NAME" heading;
- report JSON (`lint_suite`): the report lint of validation.report, and every distribution stands on rows of
  one response class, one library and one code version, with no aggregate key anywhere in the suite.

The harness refuses such an aggregate at the source (scoring.aggregate_ce); this lint finds one written or
generated somewhere else. Exit status 1 on any finding.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from . import report as report_mod

ROOT = Path(__file__).resolve().parent.parent
DOC_GLOBS = ["README.md", "CHANGELOG.md", "docs/*.md", "docs/guide/*.md", "integrations/*/README.md"]
AGGREGATE = re.compile(
    r"\b(?:overall|headline|combined|pooled|grand|global|aggregate[ds]?|total|single)\b[^.;|]{0,20}\bC/E\b"
    r"|\bC/E\b[^.;|]{0,30}\b(?:across|over)\s+(?:all|every|the)\s+(?:response\s+)?(?:classes|libraries|code versions|benchmarks|cases)\b"
    r"|\b(?:average|mean|median)\s+(?:of\s+)?C/E\b[^.;|]{0,30}\b(?:across|over)\s+(?:all|every|the)\b"
    r"|\bC/E\b[^.;|]{0,12}\b(?:averaged|pooled|aggregated)\b[^.;|]{0,20}\b(?:across|over)\b",
    re.IGNORECASE)
NEGATION = re.compile(r"\b(?:no|not|never|nor|without|cannot|refuses?|refused|none)\b", re.IGNORECASE)
TABLE_HEADER = "| count | mean | median | std dev | 5th | 95th | min | max |"
CLASS_HEADING = re.compile(r"^### Response class \S+")


def sentences(text: str) -> list[tuple[str, int]]:
    """(sentence, offset) pairs split at sentence ends, semicolons and blank lines."""
    out, start = [], 0
    for m in re.finditer(r"(?<=[.!?;])\s+|\n\s*\n", text):
        out.append((text[start:m.start()], start))
        start = m.end()
    out.append((text[start:], start))
    return out


def lint_text(name: str, text: str) -> list[str]:
    findings = []
    for sentence, offset in sentences(text):
        flat = re.sub(r"\s+", " ", sentence)
        for m in AGGREGATE.finditer(flat):
            if not NEGATION.search(flat[:m.start()]):
                line = text.count("\n", 0, offset) + 1
                findings.append(f"{name}:{line}: VAL-035: {m.group(0)!r} reads as a C/E aggregate across classes, libraries or code versions")
    return findings


def lint_report_markdown(name: str, text: str) -> list[str]:
    """A distribution table belongs under a response class heading of one case."""
    findings = []
    heading = None
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("#"):
            heading = line if CLASS_HEADING.match(line) else None
        elif line.strip() == TABLE_HEADER and heading is None:
            findings.append(f"{name}:{number}: VAL-035: a C/E distribution table outside a '### Response class' section")
    return findings


def lint_suite(suite: dict) -> list[str]:
    problems = [f"VAL-035: {p}" for p in report_mod.lint_report(suite)]
    for key in ("mean", "headline", "overall", "total", "aggregate", "ce"):
        if key in suite or key in suite.get("coverage", {}):
            problems.append(f"VAL-035: the suite carries a top-level C/E key {key!r}")
    for case in suite.get("cases", []):
        for name, cls in case.get("classes", {}).items():
            rows = [r for r in case.get("rows", []) if r.get("response_class") == name and r.get("ce") is not None]
            count = (cls.get("distribution") or {}).get("count")
            if count is not None and count > len(rows):
                problems.append(f"VAL-035: {case['case_id']}/{name}: distribution of {count} values stands on {len(rows)} rows of that class")
            for field in ("library_sha256", "code_version"):
                seen = sorted({str(r.get(field)) for r in rows if field in r})
                if len(seen) > 1:
                    problems.append(f"VAL-035: {case['case_id']}/{name}: distribution mixes {field} {seen}")
    return problems


def run(root: Path = ROOT, files: list[Path] | None = None) -> list[str]:
    findings = []
    paths = [p for g in DOC_GLOBS for p in sorted(root.glob(g))] + list(files or [])
    for path in dict.fromkeys(paths):
        text = path.read_text(encoding="utf-8", errors="replace")
        name = str(path.relative_to(root)) if path.is_relative_to(root) else str(path)
        findings += lint_text(name, text)
        if path.name.startswith("report") and path.suffix == ".md":
            findings += lint_report_markdown(name, text)
    return findings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="lint documents for C/E aggregates across classes, libraries or code versions")
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("files", type=Path, nargs="*")
    args = ap.parse_args(argv)
    findings = run(args.root, args.files)
    for f in findings:
        print(f)
    print(f"aggregate lint: {'clean' if not findings else str(len(findings)) + ' finding(s)'}", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
