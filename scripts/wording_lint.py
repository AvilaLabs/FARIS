#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Wording lint over user-facing text (requirements LEG-041..044, VAL-084, VAL-090, QA-052, DSN-085).

Scans Rust string literals in the crate sources, the string literals of the
package script, the browser page, and the Markdown documents users read. It
does not scan comments, tests or docs/requirements/, which name forbidden words
in order to forbid them. A document listed in ADVISORY_MARKDOWN (none now) is scanned
as advisory: its findings are printed, and fail only with --strict. Website copy lives in another repository; run
`wording_lint.py --extra DIR` from there (or CI) to scan its .html and .md files.

Rules (word lists are versioned in wording_lint_terms.json):
  LEG-041  claims of licensing, qualification, certification or safety approval
  LEG-042  "digital twin" and "twin"
  LEG-043  variant spellings of verdict words and kind labels
  LEG-044  internal codenames in text and in script or package file names
  QA-052   "compliant", "certified" or "qualified" tied to NQA-1 or a regulator
           (VAL-090 and DSN-085 are the same claims in recommendations and exports)

A forbidden-claim hit is allowed when a negation word (not, no, never, nor,
without, cannot) appears within a few words before it in the same sentence, or
NOT_EVALUATED / "not evaluated" follows it closely. Everything else needs an
exact-sentence entry in the allow list, with a reason. An allow-list entry that
no longer matches anything is itself an error, so the list cannot rot.

Exit status 1 on any finding. Output: file:line: RULE: text.
"""
from __future__ import annotations

import argparse
import html
import io
import json
import re
import sys
import tokenize
from dataclasses import dataclass, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TERMS = Path(__file__).with_name("wording_lint_terms.json")
RUST_DIRS = ["crates/*/src"]
MARKDOWN = ["README.md", "CHANGELOG.md", "CONTRIBUTING.md", "docs/*.md", "docs/guide/*.md", "integrations/*/README.md"]
# Documents whose findings are printed but do not fail (--strict makes them fail). Empty: the design documents
# that used "qualification" and "licence" in their technical senses were reworded (VAL-090).
ADVISORY_MARKDOWN: set[str] = set()
# Third-party licence texts: they say "licensed" because that is what they are.
SKIPPED_MARKDOWN = {"docs/THIRD_PARTY_NOTICES.md", "docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md"}
HTML = ["crates/faris-app/index.html"]
PYTHON_TEXT = ["scripts/package_recorded_demo.py"]
NAME_DIRS = ["scripts"]


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str
    text: str
    sentence: str
    advisory: bool = False

    def render(self) -> str:
        return f"{self.path}:{self.line}: {'advisory ' if self.advisory else ''}{self.rule}: {self.text!r} in: {self.sentence[:160]}"


def load_terms(path: Path = TERMS) -> dict:
    terms = json.loads(path.read_text(encoding="utf-8"))
    validate_terms(terms)
    return terms


def validate_terms(terms: dict) -> None:
    """Validate optional allow-list path scopes and basic entry shape."""
    for index, entry in enumerate(terms.get("allow_list", [])):
        if not isinstance(entry, dict) or not isinstance(entry.get("rule"), str) or not isinstance(entry.get("text"), str):
            raise ValueError(f"allow_list[{index}] must contain string rule and text fields")
        if "paths" in entry:
            paths = entry["paths"]
            if (not isinstance(paths, list) or not paths or
                    any(not isinstance(path, str) or not path or Path(path).is_absolute() or ".." in Path(path).parts
                        for path in paths)):
                raise ValueError(f"allow_list[{index}].paths must be a non-empty list of relative repository paths")


# --- text extraction -------------------------------------------------------


def rust_strings(source: str) -> list[tuple[str, int]]:
    """String literals of non-test Rust code as (raw text, first line)."""
    tokens: list[tuple[str, str, int]] = []  # (kind, value, line)
    i, n, line = 0, len(source), 1
    while i < n:
        c = source[i]
        if c == "\n":
            line += 1
            i += 1
        elif c.isspace():
            i += 1
        elif source.startswith("//", i):
            while i < n and source[i] != "\n":
                i += 1
        elif source.startswith("/*", i):
            depth = 0
            while i < n:
                if source.startswith("/*", i):
                    depth += 1
                    i += 2
                elif source.startswith("*/", i):
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    line += source[i] == "\n"
                    i += 1
        elif c == "r" and re.match(r'r#*"', source[i:i + 40]) or (
                c == "b" and re.match(r'br#*"', source[i:i + 40])):
            m = re.match(r'b?r(#*)"', source[i:i + 40])
            hashes = m.group(1)
            start = i + m.end()
            end = source.index('"' + hashes, start)
            body = source[start:end]
            tokens.append(("str", body, line))
            line += body.count("\n")
            i = end + 1 + len(hashes)
        elif c == '"' or (c == "b" and source.startswith('b"', i)):
            i += 1 if c == '"' else 2
            start = i
            while source[i] != '"':
                i += 2 if source[i] == "\\" else 1
            body = source[start:i]
            tokens.append(("str", body, line))
            line += body.count("\n")
            i += 1
        elif c == "'":
            m = re.match(r"'(?:\\.[^']*|[^\\'])'", source[i:i + 12])
            if m:
                i += m.end()
            else:
                i += 1
        elif c.isalnum() or c == "_":
            m = re.match(r"[A-Za-z0-9_]+", source[i:])
            tokens.append(("id", m.group(0), line))
            i += m.end()
        else:
            tokens.append(("p", c, line))
            i += 1
    # Drop #[cfg(test)] items.
    out: list[tuple[str, int]] = []
    k = 0
    while k < len(tokens):
        if _is_cfg_test(tokens, k):
            k = _skip_item(tokens, k)
            continue
        if tokens[k][0] == "str":
            out.append((tokens[k][1], tokens[k][2]))
        k += 1
    return out


def _is_cfg_test(tokens, k: int) -> bool:
    shape = ["#", "[", "cfg", "(", "test", ")", "]"]
    if k + len(shape) > len(tokens):
        return False
    return [t[1] for t in tokens[k:k + len(shape)]] == shape


def _skip_item(tokens, k: int) -> int:
    k += 7
    while k < len(tokens) and tokens[k][1] == "#":  # further attributes
        depth = 0
        while k < len(tokens):
            depth += tokens[k][1] == "["
            depth -= tokens[k][1] == "]"
            k += 1
            if depth == 0:
                break
    depth = 0
    while k < len(tokens):
        v = tokens[k][1] if tokens[k][0] != "str" else ""
        if v == ";" and depth == 0:
            return k + 1
        if v == "{":
            depth += 1
        elif v == "}":
            depth -= 1
            if depth == 0:
                return k + 1
        k += 1
    return k


def python_strings(source: str) -> list[tuple[str, int]]:
    """Non-docstring string literals of a Python file as (text, first line)."""
    out: list[tuple[str, int]] = []
    previous_significant = None
    for tok in tokenize.generate_tokens(io.StringIO(source).readline):
        if tok.type in (tokenize.NL, tokenize.COMMENT, tokenize.INDENT, tokenize.DEDENT):
            continue
        is_text = tok.type == tokenize.STRING or tok.type == getattr(tokenize, "FSTRING_MIDDLE", -1)
        if is_text:
            docstring = tok.type == tokenize.STRING and previous_significant in (
                None, tokenize.NEWLINE, tokenize.INDENT) and _statement_is_bare_string(tok)
            if not docstring:
                out.append((tok.string, tok.start[0]))
        previous_significant = tok.type
    return out


def _statement_is_bare_string(tok) -> bool:
    line = tok.line.strip()
    return line.startswith(('"""', "'''", 'r"""', '"', "'")) and line.endswith(('"""', "'''", '"', "'")) \
        and line == tok.string.strip() or line.startswith(('"""', "'''"))


def markdown_units(text: str) -> list[tuple[str, int]]:
    """Paragraph, list-item, heading and table-row units with their first line."""
    units: list[tuple[str, int]] = []
    current: list[str] = []
    first = 1

    def flush():
        if current:
            units.append((" ".join(current), first))
            current.clear()

    for number, raw in enumerate(text.splitlines(), 1):
        stripped = raw.strip()
        starts_block = bool(re.match(r"(#+ |[-*] |\d+\. |\||```)", stripped))
        if not stripped:
            flush()
            continue
        if starts_block:
            flush()
        if not current:
            first = number
        current.append(stripped)
        if stripped.startswith(("#", "|")):
            flush()
    flush()
    return units


# --- rules -----------------------------------------------------------------


def normalise(sentence: str) -> str:
    return re.sub(r"\s+", " ", sentence.replace("\\\n", " ").replace("\\n", " ")).strip()


def sentences(text: str) -> list[tuple[str, int]]:
    """Split text into (sentence, offset in text)."""
    out, start = [], 0
    for m in re.finditer(r"(?<=[.!?;])\s+|\\n|\n\s*\n", text):
        out.append((text[start:m.start()], start))
        start = m.end()
    out.append((text[start:], start))
    return out


def words(text: str) -> list[str]:
    return re.findall(r"[A-Za-z_'-]+", text)


def negated(before: str, after: str, terms: dict) -> bool:
    window = words(before)[-terms["negation_window_words"]:]
    negations = set(terms["negation_words"])
    if any(w.lower() in negations or w.lower().endswith("n't") for w in window):
        return True
    ahead = " ".join(words(after)[:terms["limiting_following_window_words"]])
    return any(p in ahead for p in terms["limiting_following_phrases"]) or \
        any(p.replace("_", " ") in ahead.lower() for p in terms["limiting_following_phrases"])


def lint_text(path: str, units: list[tuple[str, int]], terms: dict, used: set | None = None,
              code_spans: bool = False) -> list[Finding]:
    """Findings in `units`. With `code_spans` (Markdown), a verdict-word variant or synonym inside a `code span`
    is a verbatim identifier (a status id, a solver status) and is not reported; LEG-043 still applies to prose."""
    allow = [a for a in terms["allow_list"] if "paths" not in a or path in a["paths"]]
    findings: list[Finding] = []
    claim = re.compile(r"\b(?:" + "|".join(terms["forbidden_claims"]) + r")\b", re.I)
    twin = re.compile(r"\b(?:" + "|".join(terms["twin_terms"]) + r")\b", re.I)
    vw = terms["verdict_words"]
    variant = re.compile(r"(?<![A-Za-z0-9])(?:" + "|".join(vw["variant_patterns"]) + r")(?![A-Za-z0-9])", re.I)
    synonym = re.compile(r"\b(?:" + "|".join(vw["synonyms"]) + r")\b")
    reg = terms["regulator_claims"]
    anchor = re.compile(r"(?<![A-Za-z0-9])(?:" + "|".join(reg["anchors"]) + r")(?![A-Za-z0-9])", re.I)
    reg_claim = re.compile(r"\b(?:" + "|".join(reg["claims"]) + r")\b", re.I)
    names_ci = re.compile(r"(?<![A-Za-z0-9])(?:" + "|".join(re.escape(x) for x in terms["codenames"]["case_insensitive"])
                          + r")(?![A-Za-z0-9])", re.I)
    names_cs = re.compile(r"(?<![A-Za-z0-9])(?:" + "|".join(re.escape(x) for x in terms["codenames"]["case_sensitive"])
                          + r")(?![A-Za-z0-9])")

    for text, first_line in units:
        for sentence, offset in sentences(text):
            norm = normalise(sentence)
            if not norm:
                continue

            def report(rule: str, match: re.Match, honour_allow: bool = True):
                if honour_allow and any((a["rule"], normalise(a["text"])) == (rule, norm) for a in allow):
                    if used is not None:
                        used.add((rule, norm))
                    return
                line = first_line + text.count("\n", 0, offset + match.start())
                findings.append(Finding(path, line, rule, match.group(0), norm))

            for m in claim.finditer(sentence):
                if not negated(sentence[:m.start()], sentence[m.end():], terms):
                    report("LEG-041", m)
            if anchor.search(sentence):
                for m in reg_claim.finditer(sentence):
                    if not negated(sentence[:m.start()], sentence[m.end():], terms):
                        report("QA-052", m)
            for m in twin.finditer(sentence):
                report("LEG-042", m)
            canonical = set(vw["canonical_forms"])
            at_start = set(vw["sentence_start_forms"])
            def in_code(m: re.Match) -> bool:
                return code_spans and sentence.count("`", 0, m.start()) % 2 == 1

            for m in variant.finditer(sentence):
                if in_code(m):
                    continue
                token = m.group(0)
                if token in canonical:
                    continue
                if token in at_start and re.match(r"\W*$", sentence[:m.start()].replace("\\", "")):
                    continue
                report("LEG-043", m)
            for m in synonym.finditer(sentence):
                if not in_code(m):
                    report("LEG-043", m)
            for m in list(names_ci.finditer(sentence)) + list(names_cs.finditer(sentence)):
                report("LEG-044", m)
    return findings


def html_units(text: str) -> list[tuple[str, int]]:
    """Visible text of an HTML page as (paragraph, first line); scripts, styles and tags are dropped."""
    text = re.sub(r"(?is)<(script|style)\b.*?</\1>", lambda m: "\n" * m.group(0).count("\n"), text)
    text = re.sub(r"(?s)<!--.*?-->", lambda m: "\n" * m.group(0).count("\n"), text)
    text = re.sub(r"(?is)<[^>]+>", lambda m: "\n\n" if re.match(r"<(p|h\d|li|div|br|tr|section)\b", m.group(0), re.I) else " ", text)
    return markdown_units(html.unescape(text))


def _name_patterns(terms: dict):
    names_ci = re.compile("|".join(re.escape(x).replace(r"\ ", "[-_ ]") for x in terms["codenames"]["case_insensitive"]), re.I)
    exact = [x.lower() for x in terms["codenames"]["case_sensitive"]]
    return names_ci, exact


def _codename_in(name: str, patterns) -> str | None:
    names_ci, exact = patterns
    hit = names_ci.search(name)
    if hit:
        return hit.group(0)
    return next((w for w in re.split(r"[^A-Za-z0-9]+", name) if w.lower() in exact), None)


def lint_listing(names: list[str], terms: dict, label: str = "") -> list[Finding]:
    """Codenames in any component of the given relative file names (a package or website file listing)."""
    patterns = _name_patterns(terms)
    findings = []
    for name in names:
        for part in name.replace("\\", "/").split("/"):
            hit = _codename_in(part, patterns)
            if hit:
                findings.append(Finding(label + name, 1, "LEG-044", hit, "file name"))
                break
    return findings


def listing_of(directory: Path) -> list[str]:
    return sorted(p.relative_to(directory).as_posix() for p in directory.rglob("*")
                  if p.is_file() and "node_modules" not in p.parts and "__pycache__" not in p.parts)


def lint_names(root: Path, terms: dict) -> list[Finding]:
    patterns = _name_patterns(terms)
    findings = []
    for pattern in NAME_DIRS:
        for path in sorted((root / pattern).rglob("*")):
            if "__pycache__" in path.parts or not path.is_file():
                continue
            hit = _codename_in(path.name, patterns)
            if hit:
                findings.append(Finding(str(path.relative_to(root)), 1, "LEG-044", hit, "file name"))
    return findings


def run(root: Path = ROOT, terms: dict | None = None, extra_dirs: list[str] | None = None,
        listings: list[str] | None = None) -> list[Finding]:
    terms = terms or load_terms()
    validate_terms(terms)
    used: set = set()
    findings: list[Finding] = []
    for pattern in RUST_DIRS:
        for path in sorted(root.glob(pattern)):
            for rs in sorted(path.rglob("*.rs")):
                rel = rs.relative_to(root)
                if "tests" in rel.parts or rs.name in ("tests.rs", "build.rs") or rs.name.endswith("_tests.rs"):
                    continue
                findings += lint_text(str(rel), rust_strings(rs.read_text(encoding="utf-8")), terms, used)
    for name in PYTHON_TEXT:
        path = root / name
        if path.exists():
            findings += lint_text(name, python_strings(path.read_text(encoding="utf-8")), terms, used)
    seen: set[Path] = set()
    for pattern in MARKDOWN:
        for path in sorted(root.glob(pattern)):
            name = str(path.relative_to(root))
            if path in seen or name in SKIPPED_MARKDOWN:
                continue
            seen.add(path)
            found = lint_text(name, markdown_units(path.read_text(encoding="utf-8")), terms, used, code_spans=True)
            if name in ADVISORY_MARKDOWN:
                found = [replace(f, advisory=True) for f in found]
            findings += found
    for name in HTML:
        path = root / name
        if path.exists():
            findings += lint_text(name, html_units(path.read_text(encoding="utf-8")), terms, used)
    for extra in extra_dirs or []:
        for path in sorted(Path(extra).rglob("*")):
            if path.suffix in (".md", ".html", ".htm") and path.is_file() and "node_modules" not in path.parts:
                text = path.read_text(encoding="utf-8")
                units = markdown_units(text) if path.suffix == ".md" else html_units(text)
                findings += lint_text(str(path), units, terms, used, code_spans=path.suffix == ".md")
    findings += lint_names(root, terms)
    for listing in listings or []:
        findings += lint_listing(listing_of(Path(listing)), terms, f"{listing}/")
    for extra in extra_dirs or []:
        findings += lint_listing(listing_of(Path(extra)), terms, f"{extra}/")
    for entry in terms["allow_list"]:
        if (entry["rule"], normalise(entry["text"])) not in used:
            findings.append(Finding("scripts/wording_lint_terms.json", 1, "ALLOW-LIST",
                                    "unused allow-list entry", normalise(entry["text"])))
    return findings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="wording lint over user-facing text")
    ap.add_argument("--strict", action="store_true", help="advisory findings fail too")
    ap.add_argument("--extra", action="append", default=[], metavar="DIR", help="also scan the .md and .html files under DIR (website copy)")
    ap.add_argument("--listing", action="append", default=[], metavar="DIR",
                    help="also check every file name under DIR for codenames (an unpacked package); --extra does too")
    args = ap.parse_args(argv)
    terms = load_terms()
    findings = run(ROOT, terms, args.extra, args.listing)
    failing = [f for f in findings if args.strict or not f.advisory]
    for finding in findings:
        print(finding.render())
    if failing:
        print(f"wording lint: {len(failing)} failing finding(s), {len(findings) - len(failing)} advisory (terms version {terms['version']})", file=sys.stderr)
        return 1
    print(f"wording lint: clean, {len(findings)} advisory (terms version {terms['version']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
