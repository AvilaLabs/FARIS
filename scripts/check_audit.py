#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Advisory check with expiring waivers (requirement SEC-031).

    python3 scripts/check_audit.py                      # runs `cargo audit --json`
    python3 scripts/check_audit.py --report audit.json  # reads a saved report instead

Fails on every RustSec finding in Cargo.lock: a vulnerability, or a warning of kind unmaintained, unsound or
yanked, unless scripts/audit_allowlist.json waives it. A waiver names the advisory (or, for a yanked crate, the
package) and carries owner, reason and an expiry date no more than 90 days after `added`. An expired waiver, one
missing a field, and one that matches no finding all fail, so the list cannot rot. Plain `cargo audit` only warns
on unmaintained crates; this turns them into a decision somebody signs.

Waiver file: {"waivers": [{"id": "RUSTSEC-...", "package": "name", "owner": ..., "reason": ..., "added": "YYYY-MM-DD",
"expiry": "YYYY-MM-DD"}]}. For a yanked crate use "id": "yanked:NAME-VERSION".
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = Path(__file__).with_name("audit_allowlist.json")
MAX_DAYS = 90


def findings(report: dict) -> list[tuple[str, str, str]]:
    """(waiver id, package label, description) for every vulnerability and warning in a cargo-audit report."""
    out = []
    for v in report.get("vulnerabilities", {}).get("list", []):
        adv = v["advisory"]
        out.append((adv["id"], f"{v['package']['name']} {v['package']['version']}", f"vulnerability: {adv['title']}"))
    for kind, items in (report.get("warnings") or {}).items():
        for w in items:
            pkg = w["package"]
            label = f"{pkg['name']} {pkg['version']}"
            if w.get("advisory"):
                out.append((w["advisory"]["id"], label, f"{kind}: {w['advisory']['title']}"))
            else:
                out.append((f"{kind}:{pkg['name']}-{pkg['version']}", label, f"{kind} crate"))
    return out


def waiver_problems(waivers: list[dict], today: date) -> list[str]:
    problems = []
    for w in waivers:
        name = w.get("id", "?")
        missing = [k for k in ("id", "package", "owner", "reason", "added", "expiry") if not str(w.get(k) or "").strip()]
        if missing:
            problems.append(f"waiver {name} lacks {', '.join(missing)}")
            continue
        try:
            added, expiry = date.fromisoformat(w["added"]), date.fromisoformat(w["expiry"])
        except ValueError:
            problems.append(f"waiver {name}: added and expiry must be YYYY-MM-DD dates")
            continue
        if expiry < today:
            problems.append(f"waiver {name} for {w['package']} expired on {expiry}; fix or re-decide it")
        elif (expiry - added).days > MAX_DAYS:
            problems.append(f"waiver {name}: expiry {expiry} is more than {MAX_DAYS} days after it was added ({added})")
    return problems


def check(report: dict, waivers: list[dict], today: date) -> list[str]:
    problems = waiver_problems(waivers, today)
    waived = {w.get("id") for w in waivers}
    found = findings(report)
    for ident, label, text in found:
        if ident not in waived:
            problems.append(f"{ident} {label}: {text}; update the crate or add a reasoned waiver")
    for ident in sorted(waived - {f[0] for f in found}):
        problems.append(f"waiver {ident} matches no finding; remove it")
    return problems


def run_audit(root: Path) -> dict:
    # cargo audit exits 1 when it finds a vulnerability, with the JSON on stdout all the same.
    out = subprocess.run(["cargo", "audit", "--json"], cwd=root, capture_output=True, text=True)
    try:
        return json.loads(out.stdout)
    except ValueError:
        raise SystemExit(f"cargo audit gave no JSON (exit {out.returncode}): {out.stderr.strip()[:400]}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--report", type=Path, help="a saved `cargo audit --json` report (default: run cargo audit)")
    ap.add_argument("--allowlist", type=Path, default=ALLOWLIST)
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    report = json.loads(args.report.read_text(encoding="utf-8")) if args.report else run_audit(args.root)
    waivers = json.loads(args.allowlist.read_text(encoding="utf-8")).get("waivers", [])
    problems = check(report, waivers, date.today())
    if problems:
        print("\n".join(problems), file=sys.stderr)
        print(f"advisory check: FAIL, {len(problems)} problems", file=sys.stderr)
        return 1
    print(f"advisory check: PASS, {len(findings(report))} findings, all waived with an unexpired waiver")
    return 0


if __name__ == "__main__":
    sys.exit(main())
