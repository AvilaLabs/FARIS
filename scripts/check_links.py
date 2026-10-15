#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Link checker for the Markdown documents (requirement DOC-026).

    python3 scripts/check_links.py [--root DIR]

Reads README.md, CHANGELOG.md, CONTRIBUTING.md, SECURITY.md and every .md under docs/ (the handbook in
docs/guide included). Inline links and images [text](target), reference definitions [id]: target, and autolinks
<https://...> are checked:

- a relative link must name a file or folder that exists, resolved from the document's folder (a link that leaves
  the repository fails);
- an anchor (#section, or file.md#section) must match a heading of the target document, using GitHub's slug rule
  (mdbook's is the same for the headings used here), or an explicit <a id="..."> / name="..." in it;
- an external link (http, https, mailto) is only checked for syntax: no network is used, so a dead external
  address is not found here (DOC-026 asks for a weekly online check; that is a separate scheduled job);
- a root-relative link (/docs/...) names a page of the hosted site and cannot be resolved offline; it is counted
  and reported, not failed.

docs/requirements/ is not checked by default: its tables are owned by the requirement process and cite sources
as `[V]([W3C](url))`, which is not a link in any renderer (43 such lines when this check was written). Pass
--include-requirements to list them.

Code spans and fenced blocks are ignored. Exit status 1 on any broken link; every one is listed as file:line.
"""
from __future__ import annotations

import argparse
import re
import sys
import unicodedata
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent
SKIPPED_DIRS = ("docs/requirements",)
TOP = ["README.md", "CHANGELOG.md", "CONTRIBUTING.md", "SECURITY.md"]
INLINE = re.compile(r"(!?)\[(?:[^\]\[]|\[[^\]]*\])*\]\(\s*(<[^>]*>|[^)\s]*)(?:\s+(?:\"[^\"]*\"|'[^']*'))?\s*\)")
REFERENCE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*(\S+)", re.M)
AUTOLINK = re.compile(r"<((?:https?|mailto):[^>\s]*)>")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
HTML_ANCHOR = re.compile(r"""<a\s+[^>]*?(?:id|name)\s*=\s*["']([^"']+)["']""", re.I)
HOST = re.compile(r"^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)*(:\d+)?$")


def slug(heading: str) -> str:
    """GitHub's heading anchor: lower case, markup and punctuation removed, spaces to hyphens."""
    text = re.sub(r"`([^`]*)`", r"\1", heading)
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"[*~]+|(?<!\w)_+|_+(?!\w)", "", text)  # emphasis markers, not word-internal underscores
    out = []
    for ch in unicodedata.normalize("NFKC", text).lower():
        if ch.isalnum() or ch in "_-":
            out.append(ch)
        elif ch.isspace():
            out.append("-")
    return "".join(out)


def strip_code(text: str, spans: bool = True) -> str:
    """The text with fenced blocks and code spans blanked, keeping line numbers."""
    lines, fence, out = text.splitlines(), None, []
    for line in lines:
        marker = re.match(r"^\s{0,3}(`{3,}|~{3,})", line)
        if fence:
            if marker and marker.group(1)[0] == fence[0] and len(marker.group(1)) >= len(fence) and not line.strip(marker.group(1)[0]).strip():
                fence = None
            out.append("")
            continue
        if marker:
            fence = marker.group(1)
            out.append("")
            continue
        out.append(re.sub(r"(`+)(?:(?!\1).)+\1", lambda m: " " * len(m.group(0)), line) if spans else line)
    return "\n".join(out)


def anchors_of(text: str) -> set[str]:
    found: set[str] = set()
    counts: dict[str, int] = {}
    for line in strip_code(text, spans=False).splitlines():
        match = HEADING.match(line)
        if match:
            base = slug(match.group(2))
            n = counts.get(base, 0)
            counts[base] = n + 1
            found.add(base if n == 0 else f"{base}-{n}")
    found.update(HTML_ANCHOR.findall(text))
    return found


def documents(root: Path, include_requirements: bool = False) -> list[Path]:
    files = [root / name for name in TOP if (root / name).is_file()]
    files += sorted(p for p in (root / "docs").rglob("*.md") if "node_modules" not in p.parts)
    if not include_requirements:
        files = [p for p in files if not any(p.relative_to(root).as_posix().startswith(d + "/") for d in SKIPPED_DIRS)]
    return files


def links_of(text: str):
    """(line number, target) for each link in `text`, code ignored."""
    clean = strip_code(text)
    for match in INLINE.finditer(clean):
        target = match.group(2)
        yield clean.count("\n", 0, match.start()) + 1, target[1:-1] if target.startswith("<") else target
    for match in REFERENCE.finditer(clean):
        yield clean.count("\n", 0, match.start()) + 1, match.group(1).strip("<>")
    for match in AUTOLINK.finditer(clean):
        yield clean.count("\n", 0, match.start()) + 1, match.group(1)


def external_problem(target: str) -> str | None:
    if target.startswith("mailto:"):
        return None if re.fullmatch(r"mailto:[^@\s]+@[^@\s]+\.[^@\s]+(\?.*)?", target) else "malformed mailto address"
    parts = urlsplit(target)
    if not parts.netloc or not HOST.match(parts.netloc.rpartition("@")[2]):
        return "external link has no valid host"
    if re.search(r"[\s<>\"]", target):
        return "external link contains a space or quote"
    return None


def check(root: Path, include_requirements: bool = False) -> tuple[list[str], dict]:
    problems: list[str] = []
    stats = {"documents": 0, "internal": 0, "anchors": 0, "external": 0, "site_relative": 0}
    cache: dict[Path, set[str]] = {}

    def anchors(path: Path) -> set[str]:
        if path not in cache:
            cache[path] = anchors_of(path.read_text(encoding="utf-8", errors="replace"))
        return cache[path]

    for doc in documents(root, include_requirements):
        stats["documents"] += 1
        rel = doc.relative_to(root)
        for number, target in links_of(doc.read_text(encoding="utf-8", errors="replace")):
            where = f"{rel}:{number}: {target}"
            if not target:
                problems.append(f"{where}: empty link target")
            elif re.match(r"^(https?|mailto):", target, re.I):
                stats["external"] += 1
                why = external_problem(target)
                if why:
                    problems.append(f"{where}: {why}")
            elif re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", target):
                problems.append(f"{where}: unsupported link scheme")
            elif target.startswith("/"):
                stats["site_relative"] += 1
            else:
                path_part, _, fragment = target.partition("#")
                dest = doc if not path_part else (doc.parent / unquote(path_part))
                stats["internal"] += 1
                try:
                    resolved = dest.resolve()
                    resolved.relative_to(root.resolve())
                except ValueError:
                    problems.append(f"{where}: leaves the repository")
                    continue
                if not resolved.exists():
                    problems.append(f"{where}: no such file or folder")
                    continue
                if fragment:
                    stats["anchors"] += 1
                    if resolved.is_dir() or resolved.suffix != ".md":
                        continue  # an anchor into a source file or folder: nothing to check offline
                    if unquote(fragment).lower() not in {a.lower() for a in anchors(resolved)}:
                        problems.append(f"{where}: no heading or anchor '{fragment}' in {resolved.relative_to(root.resolve())}")
    return problems, stats


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--include-requirements", action="store_true", help="also check docs/requirements/")
    args = ap.parse_args(argv)
    problems, stats = check(args.root, args.include_requirements)
    summary = (f"{stats['documents']} documents, {stats['internal']} internal links ({stats['anchors']} with an anchor), "
               f"{stats['external']} external links syntax-checked, {stats['site_relative']} site-relative links not resolved")
    if problems:
        print("\n".join(problems), file=sys.stderr)
        print(f"links: FAIL, {len(problems)} broken; {summary}", file=sys.stderr)
        return 1
    print(f"links: PASS, {summary}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
