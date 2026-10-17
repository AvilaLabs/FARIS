#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Compare a third-party notices file with the dependency graph of one platform (requirement LEG-013).

    cargo metadata --locked --offline --format-version 1 --filter-platform TARGET > metadata.json
    check_platform_notices.py --metadata metadata.json --root-crate faris-cli --root-crate faris-app \\
        --notices docs/THIRD_PARTY_NOTICES.md

The packages that matter are the registry (or git) packages reachable from the root crates in the graph
that `cargo metadata --filter-platform TARGET` resolves, the same set scripts/collect_dependency_notices.py
collects. The notices file must name each one as "name version" in its package table, name nothing else,
and give each one a licence text. Any difference is listed and exits with status 1. The desktop workflow runs
this for every shipped platform after it collects that platform's notices; CI runs it for the committed Linux
files.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROW = re.compile(r"^\| (\S+) (\S+) \| ([^|]*) \| [^|]* \| ([^|]*) \|\s*$")


def graph_packages(metadata: dict, root_names: set[str]) -> set[str]:
    """Packages with a source (not the workspace's own) reachable from the named root crates, as "name version"."""
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    roots = [p["id"] for p in metadata["packages"] if p["name"] in root_names]
    if len(roots) != len(root_names):
        raise ValueError("metadata must include each requested root crate exactly once")
    pending, used = roots[:], set()
    while pending:
        package_id = pending.pop()
        if package_id in used:
            continue
        used.add(package_id)
        pending.extend(dep["pkg"] for dep in nodes[package_id]["deps"])
    return {f"{packages[i]['name']} {packages[i]['version']}" for i in used if packages[i]["source"]}


def notice_packages(text: str) -> tuple[set[str], list[str]]:
    """The "name version" entries of the package table, and the entries whose licence-text cell is empty."""
    names, empty = set(), []
    for line in text.splitlines():
        match = ROW.match(line)
        if not match or match.group(1) in ("Package", "---"):
            continue
        name = f"{match.group(1)} {match.group(2)}"
        names.add(name)
        if not match.group(4).strip():
            empty.append(name)
    return names, empty


def compare(metadata: dict, root_names: set[str], notices: str) -> list[str]:
    graph = graph_packages(metadata, root_names)
    listed, empty = notice_packages(notices)
    problems = [f"in the graph, not in the notices: {name}" for name in sorted(graph - listed)]
    problems += [f"in the notices, not in the graph: {name}" for name in sorted(listed - graph)]
    problems += [f"no licence text: {name}" for name in sorted(empty)]
    claimed = re.search(r"^(\d+) dependency versions;", notices, re.M)
    if not claimed or int(claimed.group(1)) != len(graph):
        problems.append(f"the header counts {claimed.group(1) if claimed else 'nothing'} dependency versions, the graph has {len(graph)}")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="compare a notices file with a platform's dependency graph")
    ap.add_argument("--metadata", required=True, type=Path)
    ap.add_argument("--root-crate", action="append", required=True)
    ap.add_argument("--notices", required=True, type=Path)
    args = ap.parse_args(argv)
    metadata = json.loads(args.metadata.read_text(encoding="utf-8"))
    problems = compare(metadata, set(args.root_crate), args.notices.read_text(encoding="utf-8"))
    for line in problems:
        print(line)
    print(f"notices check: {args.notices}: {len(problems)} difference(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
