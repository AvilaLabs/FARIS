# SPDX-License-Identifier: AGPL-3.0-only
"""Lints over the validation cases and the V&V matrix (requirements VAL-005, 041, 070, 075, 093, QA-091).

    python3 -m validation.checks            # lint the tree; exit 1 on any finding that is not advisory
    python3 -m validation.checks --strict   # advisory findings fail too
    python3 -m validation.checks --append-rule CASE_ID   # record the current rule of a case in rule_hashes.json

Rules:
  VAL-005  every case has a rule entry in the append-only rule_hashes.json, dated no later than the case's
           declared date and before any run record; the newest entry's hash is the hash of the rule now in
           force (the manifest's compatibility block plus protocol.json when the case has one). A changed
           rule without a new entry (a new version, a later date) is a finding.
  VAL-041  a code-to-code matrix row is never labelled validated, and names the shared inputs (nuclear data,
           geometry, source). The source is advisory until the manifests name it.
  VAL-070  every benchmark row carries the hash of its case's qualified range, and it matches the manifest.
  VAL-075  every row has a not-covered list, and for a benchmark row it is the manifest's list.
  QA-091   every case pins its upstream files by sha256 and names source, retrieval date and licence, and
           vendors none of them (a restricted data set is never redistributed).
  VAL-093  `cross_thread_check` compares two run records of one case made with different thread counts.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

from .manifest import DATE, SHA256, load_manifest
from .matrix import range_sha256

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CASES = HERE / "cases"
MATRIX = HERE / "matrix.json"
RULE_HASHES = HERE / "rule_hashes.json"
RULES_SCHEMA = "faris.validation-rule-hashes/1.0.0"
# Findings of these rules are printed but do not fail the run unless --strict.
ADVISORY = {"VAL-041-source"}
SHARED_INPUTS = (("nuclear data", "VAL-041"), ("geometry", "VAL-041"), ("source", "VAL-041-source"))


def canonical_sha256(obj) -> str:
    return hashlib.sha256(json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")).hexdigest()


def rule_of(manifest: dict, case_dir: Path) -> dict:
    """The decision rule in force for a case: what its comparison is judged against."""
    rule = {"compatibility": manifest["compatibility"]}
    protocol = case_dir / "protocol.json"
    if protocol.is_file():
        rule["protocol"] = json.loads(protocol.read_text(encoding="utf-8"))
    return rule


def run_dates(case_dir: Path) -> list[tuple[str, str]]:
    out = []
    for path in sorted(case_dir.glob("records/*.run.json")):
        date = json.loads(path.read_text(encoding="utf-8")).get("run_date")
        out.append((path.name, date or ""))
    return out


def check_rule_hashes(case_dirs: list[Path], rules_file: Path = RULE_HASHES) -> list[tuple[str, str]]:
    """VAL-005. Returns (rule id, message) findings."""
    findings: list[tuple[str, str]] = []
    if not rules_file.is_file():
        return [("VAL-005", f"{rules_file.name} is missing; record the rules with --append-rule CASE_ID")]
    data = json.loads(rules_file.read_text(encoding="utf-8"))
    if data.get("schema") != RULES_SCHEMA:
        findings.append(("VAL-005", f"{rules_file.name}: schema must be {RULES_SCHEMA!r}"))
    entries = data.get("entries", [])
    for case_dir in case_dirs:
        manifest = load_manifest(case_dir / "manifest.json")
        case_id = manifest["case_id"]
        mine = [e for e in entries if e.get("case_id") == case_id]
        if not mine:
            findings.append(("VAL-005", f"{case_id}: no rule entry in {rules_file.name}; run --append-rule {case_id}"))
            continue
        versions = [e.get("version") for e in mine]
        if versions != list(range(1, len(mine) + 1)):
            findings.append(("VAL-005", f"{case_id}: rule versions must be 1, 2, ... in order, got {versions}"))
        dates = [e.get("declared", "") for e in mine]
        if not all(DATE.match(d) for d in dates) or dates != sorted(dates):
            findings.append(("VAL-005", f"{case_id}: declared dates must be YYYY-MM-DD and not go back in time, got {dates}"))
        newest = mine[-1]
        if not SHA256.match(newest.get("rule_sha256", "")):
            findings.append(("VAL-005", f"{case_id}: newest entry has no valid rule_sha256"))
        elif newest["rule_sha256"] != canonical_sha256(rule_of(manifest, case_dir)):
            findings.append(("VAL-005", f"{case_id}: the rule changed after it was recorded; append a new version with a new declared date (never edit an entry)"))
        if newest.get("declared") != manifest["compatibility"]["declared"]:
            findings.append(("VAL-005", f"{case_id}: newest entry is declared {newest.get('declared')} but the manifest says {manifest['compatibility']['declared']}"))
        for name, date in run_dates(case_dir):
            if not date or date < newest.get("declared", ""):
                findings.append(("VAL-005", f"{case_id}: run record {name} (run_date {date or 'missing'}) is not dated on or after the rule's declared date {newest.get('declared')}"))
    known = {load_manifest(d / "manifest.json")["case_id"] for d in case_dirs}
    for e in entries:
        if e.get("case_id") not in known:
            findings.append(("VAL-005", f"{rules_file.name}: entry for unknown case {e.get('case_id')!r}"))
    return findings


def append_rule(case_dir: Path, rules_file: Path = RULE_HASHES, recorded: str | None = None) -> dict:
    """Append the case's current rule as a new version; refuses when it equals the newest entry."""
    manifest = load_manifest(case_dir / "manifest.json")
    data = json.loads(rules_file.read_text(encoding="utf-8")) if rules_file.is_file() else {"schema": RULES_SCHEMA, "entries": []}
    mine = [e for e in data["entries"] if e["case_id"] == manifest["case_id"]]
    digest = canonical_sha256(rule_of(manifest, case_dir))
    if mine and mine[-1]["rule_sha256"] == digest:
        raise SystemExit(f"{manifest['case_id']}: the rule is unchanged since version {mine[-1]['version']}")
    commit = None
    if case_dir.resolve().is_relative_to(ROOT):
        commit = subprocess.run(["git", "log", "-1", "--format=%H", "--", str((case_dir / "manifest.json").resolve().relative_to(ROOT))],
                                cwd=ROOT, capture_output=True, text=True).stdout.strip() or None
    entry = {"case_id": manifest["case_id"], "version": len(mine) + 1, "declared": manifest["compatibility"]["declared"],
             "rule_sha256": digest, "manifest_last_commit": commit, "recorded": recorded or _today()}
    data["entries"].append(entry)
    rules_file.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return entry


def _today() -> str:
    import datetime
    return datetime.date.today().isoformat()


def check_matrix_rows(matrix: dict, manifests: dict[str, dict]) -> list[tuple[str, str]]:
    """VAL-041, VAL-070, VAL-075 over the generated matrix."""
    findings = []
    for i, row in enumerate(matrix.get("rows", [])):
        label = f"row {i + 1} ({row.get('quantity')}, {row.get('benchmark_id')})"
        if not str(row.get("not_covered", "")).strip():
            findings.append(("VAL-075", f"{label}: empty not-covered list"))
        if row.get("benchmark_id") == "none":
            continue
        case_id = row["benchmark_id"].split("/")[0]
        m = manifests.get(case_id)
        if m is None:
            findings.append(("VAL-070", f"{label}: no manifest for case {case_id}"))
            continue
        want = range_sha256(m["qualified_range"])
        if row.get("range_sha256") != want:
            findings.append(("VAL-070", f"{label}: range_sha256 {str(row.get('range_sha256'))[:12]} does not match the manifest's qualified range ({want[:12]})"))
        if row.get("not_covered") != "; ".join(m["not_covered"]):
            findings.append(("VAL-075", f"{label}: not-covered list differs from the manifest's"))
        if m["evidence_class"] == "code-to-code":
            if row.get("status", "").startswith("validated"):
                findings.append(("VAL-041", f"{label}: code-to-code agreement is verification evidence and cannot be labelled validated"))
            text = (row.get("not_covered", "") + " " + row.get("reason", "")).lower()
            for word, rule in SHARED_INPUTS:
                if word not in text:
                    findings.append((rule, f"{label}: code-to-code row does not name the shared {word}"))
    return findings


def check_provenance(case_dirs: list[Path]) -> list[tuple[str, str]]:
    """QA-091: hash, source, version and licence for every case; nothing vendored."""
    findings = []
    for case_dir in case_dirs:
        m = load_manifest(case_dir / "manifest.json")  # schema checks hashes, source pin, licence
        cid = m["case_id"]
        if not (m["source"].get("commit") or m["source"].get("doi")):
            findings.append(("QA-091", f"{cid}: source has neither a pinned commit nor a DOI"))
        if not m["license"].get("spdx") or not m["license"].get("attribution"):
            findings.append(("QA-091", f"{cid}: licence id or attribution missing"))
        if not all(SHA256.match(f["sha256"]) for f in m["files"]):
            findings.append(("QA-091", f"{cid}: a file entry has no sha256"))
        if m.get("files_are_external_not_vendored") is not True:
            findings.append(("QA-091", f"{cid}: upstream files must stay outside the repository"))
        listed = {Path(f["path_in_source"]).name for f in m["files"]}
        vendored = sorted(p.name for p in case_dir.rglob("*") if p.is_file() and p.name in listed)
        if vendored:
            findings.append(("QA-091", f"{cid}: upstream file(s) stored in the repository: {', '.join(vendored)}"))
    return findings


def cross_thread_check(a: dict, b: dict, k: float = 2.0) -> list[str]:
    """VAL-093: two run records of one case at different thread counts agree within k sigma per detector.

    Returns the problems (empty when they agree). Each detector's difference is compared with
    k * sqrt(u_a^2 + u_b^2) of the two Monte Carlo errors.
    """
    problems = []
    if a.get("case_id") != b.get("case_id"):
        return [f"records are for different cases ({a.get('case_id')} and {b.get('case_id')})"]
    ta, tb = (a.get("context") or {}).get("threads"), (b.get("context") or {}).get("threads")
    if ta is None or tb is None:
        problems.append("a record does not say how many threads it used (context.threads), so the comparison cannot be called cross-thread")
    elif ta == tb:
        problems.append(f"both records used {ta} thread(s); a cross-thread check needs two different counts")
    ra = {r["detector_id"]: r for r in a.get("results", [])}
    rb = {r["detector_id"]: r for r in b.get("results", [])}
    if set(ra) != set(rb):
        problems.append(f"detector sets differ: {len(set(ra) ^ set(rb))} detector(s) appear in only one record")
    for did in sorted(set(ra) & set(rb)):
        va, vb = ra[did]["value"], rb[did]["value"]
        sigma = math.hypot(ra[did].get("u_mc", 0.0), rb[did].get("u_mc", 0.0))
        if abs(va - vb) > k * sigma:
            problems.append(f"{did}: {va:.6g} and {vb:.6g} differ by more than {k:g} sigma ({k * sigma:.3g})")
    return problems


def run_all(root: Path = ROOT, strict: bool = False) -> int:
    case_dirs = sorted(p.parent for p in (root / "validation" / "cases").glob("*/manifest.json"))
    manifests = {m["case_id"]: m for m in (load_manifest(d / "manifest.json") for d in case_dirs)}
    matrix = json.loads((root / "validation" / "matrix.json").read_text(encoding="utf-8"))
    findings = check_rule_hashes(case_dirs, root / "validation" / "rule_hashes.json")
    findings += check_matrix_rows(matrix, manifests) + check_provenance(case_dirs)
    failing = 0
    for rule, message in findings:
        advisory = rule in ADVISORY and not strict
        failing += not advisory
        print(f"{'advisory ' if advisory else ''}{rule}: {message}")
    print(f"validation checks: {len(findings)} finding(s), {failing} failing")
    return 1 if failing else 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python3 -m validation.checks")
    ap.add_argument("--strict", action="store_true", help="advisory findings fail too")
    ap.add_argument("--append-rule", metavar="CASE_ID", help="record the case's current rule as a new version")
    args = ap.parse_args(argv)
    if args.append_rule:
        print(json.dumps(append_rule(CASES / args.append_rule), indent=2))
        return 0
    return run_all(ROOT, args.strict)


if __name__ == "__main__":
    sys.exit(main())
