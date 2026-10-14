#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Check that Monte Carlo dependent Python tests record their seed, acceptance and source.

Applies QA-017 and QA-093 to the physics controls: `controls/` and the top-level
`integrations/openmc/test_*.py` (the recorded-data tests in `integrations/openmc/risk_tests`
assert stored numbers and are outside this scope; `--all` lists their findings too).

A test file is stochastic when it draws random numbers or sets a transport seed. Every test in
such a file must carry a marker comment (inside the test, or on its own line at module level):

    # Stochastic: seed 20261005; |z| < 3; expected: analytic torus volumes (GEO-024)
    # Stochastic: seed 11; structural; expected: none, the test checks stop behaviour only

so that the seed, the acceptance and the origin of the expected value live with the test.
Findings:

* a stochastic test without a well-formed marker, a non-numeric seed, or |z| above 3;
* unseeded randomness (`random.random()` without `random.seed`, `Random()`, `default_rng()`);
* a sigma multiple above 3 as the acceptance (`5 * sigma`, `Z_LIMIT = 5`);
* exact float equality with a non-zero float literal (`assertEqual(x, 1.5)`, `assert x == 1.5`),
  unless the line says `# exact: <reason>`.

Exit status 1 when anything is found. Reads files only.
"""
from __future__ import annotations

import argparse
import ast
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DEFAULT_ROOTS = [(REPO / "controls", True), (REPO / "integrations" / "openmc", False)]
MARKER = re.compile(
    r"#\s*Stochastic:\s*seed\s+(?P<seed>\S+?)\s*;\s*(?P<accept>\|z\|\s*<\s*(?P<k>\d+(?:\.\d+)?)|structural)\s*;"
    r"\s*expected:\s*(?P<source>\S.*)$"
)
MARKER_START = re.compile(r"#\s*Stochastic:")
STOCHASTIC = re.compile(
    r"\brandom\.(?:random|uniform|gauss|randint|choice|shuffle|sample|seed)\(|\bRandom\(|default_rng\(|"
    r"RandomState\(|np\.random|numpy\.random|\.seed\s*=|\bsettings\.seed\b"
)
UNSEEDED = re.compile(r"\bRandom\(\s*\)|default_rng\(\s*\)|RandomState\(\s*\)")
SIGMA_MULTIPLE = re.compile(r"\b(\d+(?:\.\d+)?)\s*\*\s*sigma\b")
Z_LIMIT = re.compile(r"\bZ_LIMIT\s*=\s*(\d+(?:\.\d+)?)")
EXACT_NOTE = re.compile(r"#\s*exact:\s*\S")


def is_nonzero_float(node: ast.AST) -> bool:
    return isinstance(node, ast.Constant) and isinstance(node.value, float) and node.value != 0.0


def exact_float_findings(tree: ast.AST, lines: list[str], name: str) -> list[str]:
    found = []
    for node in ast.walk(tree):
        hit = False
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr in ("assertEqual", "assertNotEqual")
        ):
            hit = any(is_nonzero_float(a) for a in node.args)
        elif isinstance(node, ast.Compare) and any(isinstance(o, (ast.Eq, ast.NotEq)) for o in node.ops):
            hit = any(is_nonzero_float(a) for a in [node.left, *node.comparators])
        if hit and not EXACT_NOTE.search(lines[node.lineno - 1]):
            found.append(
                f"{name}:{node.lineno}: exact float equality with a literal; compare with a tolerance "
                "or say why on the line with `# exact: <reason>`"
            )
    return found


def test_functions(tree: ast.AST):
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test"):
            yield node


def check_marker(text: str, where: str) -> list[str]:
    match = MARKER.search(text)
    if match is None:
        return [f"{where}: the Stochastic marker is malformed; write `# Stochastic: seed <n>; |z| < <k>; expected: <source>`"]
    out = []
    seed = match.group("seed")
    if not (seed.isdigit() or re.fullmatch(r"[A-Z][A-Z0-9_]*", seed)):
        out.append(f"{where}: the seed `{seed}` must be a number or an upper-case constant name")
    if match.group("k") is not None and float(match.group("k")) > 3.0:
        out.append(f"{where}: acceptance |z| < {match.group('k')} is above the 3 sigma rule")
    return out


def check_file(path: Path, display: str, exact_rule: bool = True) -> list[str]:
    source = path.read_text(encoding="utf-8")
    lines = source.splitlines()
    tree = ast.parse(source, filename=str(path))
    findings: list[str] = []
    if exact_rule:
        findings += exact_float_findings(tree, lines, display)
    stochastic = STOCHASTIC.search(source) is not None
    for number, line in enumerate(lines, 1):
        code = line.split("#", 1)[0]
        if UNSEEDED.search(code):
            findings.append(f"{display}:{number}: randomness without a seed")
        for match in SIGMA_MULTIPLE.finditer(code):
            if float(match.group(1)) > 3.0:
                findings.append(f"{display}:{number}: acceptance of {match.group(1)} sigma is above the 3 sigma rule")
        match = Z_LIMIT.search(code)
        if match and float(match.group(1)) > 3.0:
            findings.append(f"{display}:{number}: Z_LIMIT {match.group(1)} is above the 3 sigma rule")
    if re.search(r"\brandom\.(?:random|uniform|gauss|randint|choice|shuffle|sample)\(", source) and not re.search(
        r"\brandom\.seed\(", source
    ):
        findings.append(f"{display}: module-level `random` is used without `random.seed(...)`")
    if not stochastic:
        return findings
    functions = [n for n in ast.walk(tree) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]

    def body_of(node) -> str:
        return "\n".join(lines[node.lineno - 1 : node.end_lineno])

    # A helper that draws numbers or sets the seed makes every test that calls it stochastic.
    def called(node) -> set[str]:
        names = {c.func.id for c in ast.walk(node) if isinstance(c, ast.Call) and isinstance(c.func, ast.Name)}
        return names | {c.func.attr for c in ast.walk(node) if isinstance(c, ast.Call) and isinstance(c.func, ast.Attribute)}

    helpers = {n.name for n in functions if not n.name.startswith("test") and STOCHASTIC.search(body_of(n))}
    while True:  # a helper that calls a stochastic helper is stochastic too
        more = {n.name for n in functions if not n.name.startswith("test") and called(n) & helpers} - helpers
        if not more:
            break
        helpers |= more
    inside_function = {i for n in functions for i in range(n.lineno, n.end_lineno + 1)}
    module_marker = next(
        (
            (number, line)
            for number, line in enumerate(lines, 1)
            if MARKER_START.match(line.strip()) and number not in inside_function
        ),
        None,
    )
    for node in test_functions(tree):
        body = body_of(node)
        if not (STOCHASTIC.search(body) or called(node) & helpers):
            continue
        inside = next((line for line in body.splitlines() if MARKER_START.search(line)), None)
        if inside is None and module_marker is not None:
            findings += check_marker(module_marker[1].strip(), f"{display}:{module_marker[0]} (module marker)")
        elif inside is None:
            findings.append(
                f"{display}:{node.lineno}: stochastic test `{node.name}` records no seed, acceptance and expected-value source"
            )
        else:
            findings += check_marker(inside.strip(), f"{display}:{node.lineno} `{node.name}`")
    return findings


def scan(roots: list[tuple[Path, bool]], everything: bool) -> list[str]:
    findings: list[str] = []
    for root, recurse in roots:
        pattern = "**/test_*.py" if recurse or everything else "test_*.py"
        for path in sorted(root.glob(pattern)):
            findings += check_file(path, str(path.relative_to(REPO)))
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="also scan integrations/openmc/risk_tests and below")
    args = parser.parse_args()
    findings = scan(DEFAULT_ROOTS, args.all)
    for finding in findings:
        print(finding)
    if findings:
        print(f"{len(findings)} finding(s)", file=sys.stderr)
        return 1
    print("stochastic test check: no findings")
    return 0


if __name__ == "__main__":
    sys.exit(main())
