#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Source lint over the Rust crates (requirements PRV-008, CFG-054, SEC-011, PRV-011, UX-046, VAL-084, QA-052, DOC-062, DSN-085).

    python3 scripts/source_lint.py [--strict]

Looks at non-test code only (comments, strings, `#[cfg(test)]` items and test files are removed first, so a line number
is the line in the real file). Rules in ADVISORY are printed but fail only with --strict: the tree violates them today and
fixing a violation needs a Rust change. Move a rule out of ADVISORY once its violations are gone. Exit status 1 on any
failing finding.

  PRV-008  wall-clock time (SystemTime, Utc::now) in faris-engine, faris-model or faris-study; PRV-008-timing is the
           monotonic Instant::now, used for durations and timeouts, which are not inputs of a result
  CFG-054  an ambient random source (thread_rng, rand::random, from_entropy, OsRng, getrandom) in those crates
  SEC-011  a comparison against a truncated digest (a slice or take() of a sha, digest or hash used with ==, != or
           starts_with); SEC-011-slice lists every truncation of a digest, to be read, with the allow list below for
           the ones that are file names or seeds and not integrity checks
  PRV-011  a constant holding a kind label (const NAME: &str = "calculated"); PRV-011-literal is a kind label written
           as a string literal outside the modules that derive it from a receipt or an input
  UX-046   a badge call with an empty explanation; UX-046-na is a bare "N/A" literal, UX-046-dash a lone dash literal
  DSN-085  faris-report writes the research-screening statement from its one constant into csv, pdf and svg output
  DOC-062  the user guide and README carry the design-model-not-a-twin sentence; VAL-084 is the same wording in the
           strings of the crates (the plant outputs), and QA-052 a document that states the NQA-1 mapping
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CALCULATION_CRATES = ["faris-engine", "faris-model", "faris-study"]
ADVISORY = {"PRV-008-timing", "SEC-011-slice", "PRV-011-literal", "UX-046-na", "UX-046-dash", "DOC-062", "VAL-084", "QA-052"}
KIND_WORDS = ("calculated", "authored", "literature", "conditional")
# Modules that define the kind text of a label from the result it labels.
KIND_DEFINERS = {"crates/faris-engine/src/brief.rs", "crates/faris-model/src/remote_handling.rs", "crates/faris-app/src/badge.rs"}
# (file, snippet) -> why a digest slice there is not an integrity comparison.
DIGEST_SLICE_OK = {
    ("crates/faris-engine/src/evidence_store.rs", "[..2]"): "two-character fan-out directory of the content store; the blob is checked by its full digest",
    ("crates/faris-study/src/store_layer.rs", "[..2]"): "two-character fan-out directory of the content store; the blob is checked by its full digest",
    ("crates/faris-app/src/package.rs", "[..2]"): "fan-out directory name when a package is unpacked; each entry is checked by its full digest",
    ("crates/faris-engine/src/history_ensemble.rs", "[..8]"): "derives a 64-bit seed from a digest; a seed is not an integrity check",
    ("crates/faris-engine/src/maintenance/actinv.rs", "[..16]"): "scratch directory name",
    ("crates/faris-engine/src/reactor/windows.rs", "[..16]"): "cache directory name",
    ("crates/faris-model/src/design/checks.rs", "take(16)"): "display text in a check message",
}
# (file, snippet) -> why a prefix comparison there is not an integrity check.
DIGEST_COMPARE_OK = {
    ("crates/faris-model/src/settings.rs", "self.key.starts_with("): "a settings key name, not a digest",
    ("crates/faris-app/src/crash.rs", "def.key.starts_with("): "a settings key name, not a digest",
    ("crates/faris-study/src/store_layer.rs", "fanout == &digest[..2]"): "checks the fan-out directory name of a digest that is_sha256_hex has just accepted in full",
}
# (file, snippet) -> why a wall-clock read there is not an input of a result.
AMBIENT_TIME_OK = {
    ("crates/faris-engine/src/result_cache.rs", "SystemTime::now()"): "cache bookkeeping only (when an entry was made and last used, for eviction); "
    "the key never includes a time, which result_cache's tests check",
}
AMBIENT_TIME = re.compile(r"\b(SystemTime::now|Utc::now|Local::now|OffsetDateTime::now_utc|chrono::Utc)\b")
MONOTONIC = re.compile(r"\bInstant::now\b")
AMBIENT_RANDOM = re.compile(r"\b(thread_rng|rand::random|from_entropy|OsRng|getrandom::|rand::rng|SmallRng::from_os)\b")
DIGEST_NAME = r"(?:sha256|sha|digest|hash|checksum|key)\w*"
DIGEST_SLICE = re.compile(rf"\b({DIGEST_NAME})\s*(?:\.\w+\(\))?\s*(\[\s*\.\.\s*\d+\s*\]|\.chars\(\)\.take\(\s*\d+\s*\)|\.get\(\s*\.\.\s*\d+\s*\))")
DIGEST_COMPARE = re.compile(rf"(?:\b{DIGEST_NAME}\w*\s*\[\s*\.\.\s*\d+\s*\]\s*(?:==|!=)|(?:==|!=)\s*&?\s*{DIGEST_NAME}\w*\s*\[\s*\.\.\s*\d+\s*\]|"
                            rf"\b{DIGEST_NAME}\w*\.starts_with\(|\bstarts_with\(\s*&?\s*{DIGEST_NAME}\w*\s*\[)")
KIND_CONST = re.compile(r'\b(?:const|static)\s+\w+\s*:\s*&(?:\'static\s+)?str\s*=\s*"(' + "|".join(KIND_WORDS) + r')"')
KIND_LITERAL = re.compile(r'"(' + "|".join(KIND_WORDS) + r')"')
BADGE_EMPTY = re.compile(r'\bbadge\s*\([^;]*?,\s*""\s*\)')
BARE_NA = re.compile(r'"\s*(?:N/A|n/a)\s*"')
DASH = re.compile(r'"[—–-]"')
TWIN_STATEMENT = re.compile(r"design model\b.{0,40}\bnot a (?:digital )?twin", re.I | re.S)


def strip_rust(source: str, keep_strings: bool = False) -> str:
    """Rust source with comments, string contents and `#[cfg(test)]` items blanked; newlines are kept."""
    out: list[str] = []
    i, n = 0, len(source)

    def blank(text: str) -> str:
        return re.sub(r"[^\n]", " ", text)

    while i < n:
        c = source[i]
        if source.startswith("//", i):
            j = source.find("\n", i)
            j = n if j < 0 else j
            out.append(blank(source[i:j]))
            i = j
        elif source.startswith("/*", i):
            depth, j = 0, i
            while j < n:
                if source.startswith("/*", j):
                    depth += 1
                    j += 2
                elif source.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            out.append(blank(source[i:j]))
            i = j
        elif re.match(r'b?r#*"', source[i:i + 4]):
            m = re.match(r'b?r(#*)"', source[i:i + 40])
            end = source.index('"' + m.group(1), i + m.end())
            j = end + 1 + len(m.group(1))
            out.append(source[i:j] if keep_strings else '"' + blank(source[i + m.end():end]) + '"')
            i = j
        elif c == '"':
            j = i + 1
            while source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            out.append(source[i:j + 1] if keep_strings else '"' + blank(source[i + 1:j]) + '"')
            i = j + 1
        elif c == "'":
            m = re.match(r"'(?:\\.[^']*|[^\\'])'", source[i:i + 12])
            out.append(m.group(0) if m else "'")
            i += m.end() if m else 1
        else:
            out.append(c)
            i += 1
    text = "".join(out)
    while True:
        m = re.search(r"#\[cfg\(test\)\]", text)
        if not m:
            return text
        j = m.end()
        while True:  # further attributes
            k = re.match(r"\s*#\[[^\]]*\]", text[j:])
            if not k:
                break
            j += k.end()
        depth, k = 0, j
        while k < len(text):
            if text[k] == ";" and depth == 0:
                k += 1
                break
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
                if depth == 0:
                    k += 1
                    break
            k += 1
        text = text[:m.start()] + blank(text[m.start():k]) + text[k:]


def rust_files(root: Path, crates: list[str] | None = None) -> list[Path]:
    out = []
    for src in sorted((root / "crates").glob("*/src")):
        if crates is not None and src.parent.name not in crates:
            continue
        for rs in sorted(src.rglob("*.rs")):
            if "tests" in rs.relative_to(src).parts or rs.name == "tests.rs" or rs.name.endswith("_tests.rs"):
                continue
            out.append(rs)
    return out


def lint_rust(root: Path = ROOT) -> list[tuple[str, str, int, str]]:
    """(rule, path, line, text) findings over the crates."""
    findings = []
    for rs in rust_files(root):
        rel = str(rs.relative_to(root))
        crate = rs.relative_to(root).parts[1]
        raw = rs.read_text(encoding="utf-8")
        code = strip_rust(raw)
        with_strings = strip_rust(raw, keep_strings=True)
        literal_lines = with_strings.splitlines()
        for number, line in enumerate(code.splitlines(), 1):
            literal_line = literal_lines[number - 1]
            if crate in CALCULATION_CRATES:
                if AMBIENT_TIME.search(line) and not any(rel == f and snippet in line for (f, snippet) in AMBIENT_TIME_OK):
                    findings.append(("PRV-008", rel, number, line.strip()))
                if MONOTONIC.search(line):
                    findings.append(("PRV-008-timing", rel, number, line.strip()))
                if AMBIENT_RANDOM.search(line):
                    findings.append(("CFG-054", rel, number, line.strip()))
            if DIGEST_COMPARE.search(line) and not any(rel == f and snippet in line for (f, snippet) in DIGEST_COMPARE_OK):
                findings.append(("SEC-011", rel, number, line.strip()))
            for m in DIGEST_SLICE.finditer(line):
                snippet = m.group(2).replace(" ", "")
                snippet = re.sub(r"\.chars\(\)\.take\((\d+)\)", r"take(\1)", snippet)
                if not any(rel == f and s in snippet for (f, s) in DIGEST_SLICE_OK):
                    findings.append(("SEC-011-slice", rel, number, line.strip()))
            if KIND_CONST.search(literal_line) and rel not in KIND_DEFINERS:
                findings.append(("PRV-011", rel, number, literal_line.strip()))
            elif rel not in KIND_DEFINERS and KIND_LITERAL.search(literal_line):
                findings.append(("PRV-011-literal", rel, number, literal_line.strip()))
            if BADGE_EMPTY.search(literal_line):
                findings.append(("UX-046", rel, number, literal_line.strip()))
            elif BARE_NA.search(literal_line):
                findings.append(("UX-046-na", rel, number, literal_line.strip()))
            elif DASH.search(literal_line):
                findings.append(("UX-046-dash", rel, number, literal_line.strip()))
    return findings


def lint_statements(root: Path = ROOT) -> list[tuple[str, str, int, str]]:
    findings = []
    report_src = root / "crates" / "faris-report" / "src"
    sources = {p.name: p.read_text(encoding="utf-8") for p in report_src.glob("*.rs") if p.name != "tests.rs"}
    for kind, module in (("csv", "tables.rs"), ("pdf", "pdf.rs"), ("svg", "charts.rs")):
        if "RESEARCH_SCREENING_STATEMENT" not in sources.get(module, ""):
            findings.append(("DSN-085", f"crates/faris-report/src/{module}", 1, f"the {kind} writer does not use RESEARCH_SCREENING_STATEMENT"))
    for name in ("docs/USER_GUIDE.md", "README.md"):
        text = (root / name).read_text(encoding="utf-8")
        if not TWIN_STATEMENT.search(text):
            findings.append(("DOC-062", name, 1, "no design-model-not-a-twin sentence"))
    strings = ""
    for rs in rust_files(root):
        strings += strip_rust(rs.read_text(encoding="utf-8"), keep_strings=True)
    if not re.search(r'"[^"\n]*design model[^"\n]*"', strings, re.I):
        findings.append(("VAL-084", "crates", 1, "no crate string says that a plant output comes from a design model"))
    docs = [root / "README.md", root / "docs" / "USER_GUIDE.md", *sorted((root / "docs" / "guide").glob("*.md"))]
    if not any(re.search(r"NQA-1", p.read_text(encoding="utf-8")) and re.search(r"NQA-1[^.]*\b(not|does not|cannot)\b", p.read_text(encoding="utf-8")) for p in docs):
        findings.append(("QA-052", "docs", 1, "no user document states that FARIS maps to NQA-1 and does not comply with it"))
    return findings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="source lint over the Rust crates")
    ap.add_argument("--strict", action="store_true", help="advisory findings fail too")
    args = ap.parse_args(argv)
    findings = lint_rust() + lint_statements()
    failing = 0
    for rule, path, line, text in findings:
        advisory = rule in ADVISORY and not args.strict
        failing += not advisory
        print(f"{path}:{line}: {'advisory ' if advisory else ''}{rule}: {text[:140]}")
    print(f"source lint: {len(findings)} finding(s), {failing} failing", file=sys.stderr)
    return 1 if failing else 0


if __name__ == "__main__":
    sys.exit(main())
