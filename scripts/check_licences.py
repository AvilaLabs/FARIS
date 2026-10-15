#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Licence policy check for the locked dependency graph (requirements LEG-003, SEC-039, SEC-031).

    cargo metadata --locked --offline --format-version 1 > metadata.json
    python3 scripts/check_licences.py --metadata metadata.json [--policy scripts/licence_policy.json]

Every package's declared SPDX licence expression must be acceptable under scripts/licence_policy.json: an OR is
acceptable when any branch is, an AND when every part is, and a bare identifier when it is on the allow list.
A package with no declared licence, an expression this script cannot parse, or an identifier off the list fails,
unless the policy has an exception naming that package and exactly that expression. An exception that no package
uses any more also fails, so the list cannot rot. Exit status 1 on any failure.

This covers what cargo-deny's `licenses` check does for this graph. cargo-deny would add banned-crate and
duplicate-version rules, a registry/source allow list (`sources`), and the advisory check in one config; those
are covered here only by scripts/check_audit.py (advisories) and by Cargo.lock being committed and used with
--locked (sources).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POLICY = Path(__file__).with_name("licence_policy.json")
TOKEN = re.compile(r"\(|\)|[^\s()]+")


class ParseError(ValueError):
    pass


def parse(expression: str):
    """Parse an SPDX expression into ("id", name) / ("and"|"or", [parts]). "A/B" is read as "A OR B"."""
    text = re.sub(r"\s*/\s*", " OR ", expression.strip())
    tokens = TOKEN.findall(text)
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        pos += 1
        return tokens[pos - 1]

    def atom():
        tok = peek()
        if tok is None:
            raise ParseError("expression ends early")
        if tok == "(":
            take()
            node = disjunction()
            if peek() != ")":
                raise ParseError("missing )")
            take()
            return node
        if tok in (")", "AND", "OR", "WITH"):
            raise ParseError(f"unexpected {tok}")
        take()
        name = tok
        if peek() == "WITH":
            take()
            if peek() is None:
                raise ParseError("WITH without an exception")
            name += " WITH " + take()
        return ("id", name)

    def conjunction():
        parts = [atom()]
        while peek() == "AND":
            take()
            parts.append(atom())
        return parts[0] if len(parts) == 1 else ("and", parts)

    def disjunction():
        parts = [conjunction()]
        while peek() == "OR":
            take()
            parts.append(conjunction())
        return parts[0] if len(parts) == 1 else ("or", parts)

    node = disjunction()
    if pos != len(tokens):
        raise ParseError(f"unexpected {tokens[pos]}")
    return node


def acceptable(node, allowed: set[str]) -> bool:
    kind, value = node
    if kind == "id":
        return value in allowed
    results = [acceptable(part, allowed) for part in value]
    return any(results) if kind == "or" else all(results)


def check(packages: list[dict], policy: dict) -> tuple[list[str], int]:
    """Problems in `packages` (cargo metadata entries) against `policy`, and how many used an exception."""
    allowed = set(policy["allowed"])
    exceptions = {(e["package"], e["licence"]): e for e in policy.get("exceptions", [])}
    for e in exceptions.values():
        if not str(e.get("reason", "")).strip():
            raise ValueError(f"exception for {e['package']} has no reason")
    used, problems = set(), []
    for p in sorted(packages, key=lambda p: (p["name"], p["version"])):
        label = f"{p['name']} {p['version']}"
        expression = p.get("license")
        if not expression or not expression.strip():
            problems.append(f"{label}: no licence declared (license_file {p.get('license_file')!r}); add the package to the policy exceptions only after reading its licence text")
            continue
        key = (p["name"], expression)
        if key in exceptions:
            used.add(key)
            continue
        try:
            ok = acceptable(parse(expression), allowed)
        except ParseError as e:
            problems.append(f"{label}: cannot parse licence {expression!r} ({e})")
            continue
        if not ok:
            problems.append(f"{label}: licence {expression!r} is not acceptable with AGPL-3.0 under the policy")
    for key in sorted(set(exceptions) - used):
        problems.append(f"policy exception for {key[0]} with licence {key[1]!r} matches no package; remove or update it")
    return problems, len(used)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--metadata", required=True, type=Path)
    ap.add_argument("--policy", type=Path, default=POLICY)
    args = ap.parse_args(argv)
    metadata = json.loads(args.metadata.read_text(encoding="utf-8"))
    policy = json.loads(args.policy.read_text(encoding="utf-8"))
    problems, used = check(metadata["packages"], policy)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        print(f"licence policy: FAIL, {len(problems)} problems", file=sys.stderr)
        return 1
    print(f"licence policy: PASS, {len(metadata['packages'])} packages, {used} by exception")
    return 0


if __name__ == "__main__":
    sys.exit(main())
