#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The release gate (requirement QA-070): one script that blocks a release unless every condition passes.

    python3 scripts/release_gate.py --vv-report DIR --release-record FILE \\
        --tests FILE --perf FILE --defects FILE --fuzz FILE --repro FILE --doc-tests FILE [--archive FILE]

It prints one line per condition, PASS or FAIL with the reason, and exits 1 unless all pass.

Conditions read from the repository (--root, default this checkout):
- every requirement ID in the "Release gate" list of docs/NEXT_RELEASE_PLAN.md is Met in the requirement
  tables (the list is parsed, never copied);
- the requirement-to-test trace is complete for those IDs (QA-002): each has a Verifies: marker and no
  marker names an unknown requirement;
- the V&V matrix (validation/matrix.json) has no blank cell, and a quantity with no benchmark is labelled
  "not validated" with a reason (VAL-051);
- every model and calculated output FARIS reports has a row in the model matrix (validation/model_matrix.json),
  its verification evidence exists, and every evidence record has exactly one class (VAL-001);
- neither matrix is stale: both equal a regeneration from the registry, models, case manifests, rules and model
  source files in the tree (VAL-037, `python3 -m validation matrix` and `python3 -m validation models`);
- wording lint is clean.

The verdicts of the V&V report are re-derived from its case records: a matrix cell that differs from the
derivation, or a row that claims validation without a case record with a PASS or FAIL verdict, fails (VAL-006).

The V&V report (VAL-091) is checked from --vv-report (the directory `python3 -m validation report` wrote)
and --release-record (the JSON that command wrote the report hash into): both must exist, the hash must
match the report.json bytes, and the report must carry methods, matrix, C/E distributions, qualified
ranges, known issues and last-run dates.

Conditions a script cannot decide from the repository take their evidence from a file and FAIL when it is
absent, unreadable, empty or for another commit. Test, performance and documentation-test evidence is
JSON {"commit": SHA, "checks": [{"name": ..., "verdict": "PASS"}, ...]}; every check must be PASS and at
least one must exist. Fuzz evidence (scripts/fuzz_all.sh) has the same shape with "driver": "libfuzzer",
"dirty": false and, per target, "seconds" >= 3600 and "crashes": 0; every file in fuzz/fuzz_targets needs
its check (QA-033). Reproducible-build evidence is {"commit": SHA, "builds": [{"artifact": ...,
"sha256_a": ..., "sha256_b": ...}]} with equal hashes. The defect log is {"defects": [{"id": ...,
"severity": "S1".."S4", "status": "open" or other, "waiver": {"owner": ..., "reason": ..., "expiry":
"YYYY-MM-DD"}}]}: an open S1 always blocks (QA-083), an open S2 needs a complete waiver that has not
expired and expires within 90 days (QA-072).

At release time the gate runs after the test, performance, fuzz and build jobs, with their evidence files
as inputs. It is not a CI step on ordinary commits, because it must fail until the release gate IDs are Met.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT))
import requirement_trace as rt  # noqa: E402
import wording_lint  # noqa: E402
from validation import matrix as vv_matrix  # noqa: E402
from validation import models as vv_models  # noqa: E402

PLAN = Path("docs") / "NEXT_RELEASE_PLAN.md"
REQUIREMENTS = Path("docs") / "requirements"
REPORT_KEYS = ("methods", "matrix", "model_matrix", "cases", "qualified_ranges", "known_issues", "last_run")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
MAX_WAIVER_DAYS = 90
FUZZ_SECONDS = 3600  # QA-033: at least one CPU-hour per fuzz target per release


class Gate:
    """The inputs of one gate run."""

    def __init__(self, root: Path, args: argparse.Namespace | None = None, **overrides):
        self.root = Path(root)
        self.today = date.today()
        self.commit: str | None = None
        self.terms: dict | None = None  # wording lint terms; the repository's own unless a test supplies others
        self.matrix: Path | None = None
        self.vv_report: Path | None = None
        self.release_record: Path | None = None
        self.evidence = {name: None for name in ("tests", "perf", "defects", "fuzz", "repro", "doc_tests")}
        for source in (vars(args) if args else {}, overrides):
            for key, value in source.items():
                if key in self.evidence:
                    self.evidence[key] = Path(value) if value else None
                elif key in ("today", "commit", "terms", "matrix", "vv_report", "release_record") and value is not None:
                    setattr(self, key, Path(value) if key in ("matrix", "vv_report", "release_record") else value)
        if self.matrix is None:
            self.matrix = self.root / "validation" / "matrix.json"
        if self.commit is None:
            self.commit = git_head(self.root)


def git_head(root: Path) -> str | None:
    try:
        out = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return None
    return out.stdout.strip() if out.returncode == 0 and out.stdout.strip() else None


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_json(path: Path | None, what: str):
    if path is None:
        raise ValueError(f"no {what} given; pass it as an input")
    if not path.is_file():
        raise ValueError(f"{what} {path} does not exist")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except ValueError as e:
        raise ValueError(f"{what} {path} is not valid JSON ({e})")


# --- repository tables --------------------------------------------------------------------------

def gate_ids(root: Path) -> list[str]:
    """The IDs of the Release gate list in docs/NEXT_RELEASE_PLAN.md, in order, without repeats."""
    path = root / PLAN
    if not path.is_file():
        raise ValueError(f"{PLAN} does not exist")
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if re.match(r"^##\s+Release gate\s*$", line))
    except StopIteration:
        raise ValueError(f"{PLAN} has no 'Release gate' section")
    ids: list[str] = []
    started = False
    for line in lines[start + 1:]:
        if line.startswith("## "):
            break
        if line.startswith("- "):
            started = True
        elif started and line.strip() and not line.startswith(" "):
            break  # the paragraph after the list
        if started:
            for rid in rt.ID.findall(line):
                if rid not in ids:
                    ids.append(rid)
    if not ids:
        raise ValueError(f"the Release gate section of {PLAN} lists no requirement IDs")
    return ids


def read_statuses(root: Path) -> dict[str, str]:
    found: dict[str, str] = {}
    for path in sorted((root / REQUIREMENTS).glob("[0-9][0-9]-*.md")):
        for line in path.read_text(encoding="utf-8").splitlines():
            match = rt.ROW.match(line)
            if match:
                cells = [c.strip() for c in rt.CELL_SPLIT.split(line.strip())[1:-1]]
                found[match.group(1)] = cells[-1].split(":")[0].strip()
    return found


def read_markers(root: Path) -> tuple[set[str], list[str]]:
    """Requirement IDs named by Verifies: markers under the source directories of `root`, and markers naming none."""
    named: set[str] = set()
    empty: list[str] = []
    for top in rt.SOURCE_DIRS:
        for path in sorted((root / top).rglob("*")):
            if path.suffix not in rt.SOURCE_SUFFIXES or "target" in path.parts or "__pycache__" in path.parts:
                continue
            if path.resolve() == Path(__file__).resolve():
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                marker = rt.MARKER.search(line)
                if marker:
                    ids = rt.ID.findall(marker.group(1))
                    named.update(ids)
                    if not ids:
                        empty.append(f"{path.relative_to(root)}:{number}")
    return named, empty


# --- conditions: each returns (passed, reason) ---------------------------------------------------

def check_gate_ids_met(g: Gate):
    ids = gate_ids(g.root)
    status = read_statuses(g.root)
    unknown = [i for i in ids if i not in status]
    not_met = [i for i in ids if i in status and status[i] != "Met"]
    met = len(ids) - len(unknown) - len(not_met)
    if unknown or not_met:
        parts = [f"{met} of {len(ids)} gate requirements are Met"]
        if not_met:
            parts.append("not Met: " + ", ".join(f"{i} ({status[i]})" for i in not_met))
        if unknown:
            parts.append("not in any requirement table: " + ", ".join(unknown))
        return False, "; ".join(parts)
    return True, f"all {len(ids)} gate requirements are Met"


def check_trace(g: Gate):
    ids = gate_ids(g.root)
    named, empty = read_markers(g.root)
    known = set(read_statuses(g.root))
    untraced = [i for i in ids if i not in named]
    unknown = sorted(named - known)
    problems = []
    if untraced:
        problems.append(f"{len(ids) - len(untraced)} of {len(ids)} gate requirements have a test marker; none for " + ", ".join(untraced))
    if unknown:
        problems.append("markers name unknown requirements: " + ", ".join(unknown))
    if empty:
        problems.append("markers naming no requirement: " + ", ".join(empty))
    return (False, "; ".join(problems)) if problems else (True, f"all {len(ids)} gate requirements have a test marker")


def check_matrix(g: Gate):
    matrix = read_json(g.matrix, "V&V matrix")
    registry_path = g.root / "validation" / "quantities.json"
    registry = json.loads(registry_path.read_text(encoding="utf-8")) if registry_path.is_file() else None
    problems = vv_matrix.check_matrix(matrix, registry)
    if problems:
        shown = "; ".join(problems[:5]) + (f"; and {len(problems) - 5} more" if len(problems) > 5 else "")
        return False, f"{len(problems)} matrix problems: {shown}"
    rows = matrix["rows"]
    unvalidated = sum(1 for r in rows if r["status"] == vv_matrix.NOT_VALIDATED)
    return True, f"{len(rows)} rows, no blank cell; {unvalidated} rows labelled not validated, each with a reason (list them in the release notes)"


def check_vv_report(g: Gate):
    record = read_json(g.release_record, "release record")
    if g.vv_report is None:
        raise ValueError("no V&V report directory given; run `python3 -m validation report --out DIR --release-record FILE`")
    report_json, report_md = g.vv_report / "report.json", g.vv_report / "report.md"
    for p in (report_json, report_md):
        if not p.is_file():
            raise ValueError(f"{p} does not exist")
    recorded = record.get("vv_report_sha256")
    if not isinstance(recorded, str) or not SHA256.match(recorded):
        raise ValueError("the release record has no vv_report_sha256 (a 64-character hex hash)")
    actual = sha256_file(report_json)
    if actual != recorded:
        raise ValueError(f"report.json hash {actual[:12]} differs from the release record's {recorded[:12]}: the report changed or the record is stale")
    recorded_md = record.get("vv_report_md_sha256")
    if recorded_md != sha256_file(report_md):
        raise ValueError("report.md hash differs from, or is missing in, the release record")
    report = json.loads(report_json.read_text(encoding="utf-8"))
    missing = [k for k in REPORT_KEYS if not report.get(k)]
    if missing:
        raise ValueError("the report lacks " + ", ".join(missing))
    problems = vv_matrix.check_matrix(report["matrix"]) + vv_models.check_model_matrix(report["model_matrix"])
    if problems:
        raise ValueError("the report's matrix has problems: " + "; ".join(problems[:3]))
    cases = [c["case_id"] for c in report["cases"]]
    absent = [c for c in cases if c not in report["last_run"] or c not in report["qualified_ranges"] or c not in report["known_issues"]]
    if absent:
        raise ValueError("last run, qualified range or known issues missing for " + ", ".join(absent))
    return True, f"report.json hash {actual[:12]} matches the release record; {len(cases)} cases, matrix of {len(report['matrix']['rows'])} rows"


def _registry(g: Gate) -> dict:
    return vv_matrix.load_registry(g.root / "validation" / "quantities.json")


def check_model_rows(g: Gate):
    """VAL-001: a reported model or quantity with no row, or a row with no real evidence, blocks the release."""
    problems = vv_models.lint_models(g.root)
    mm = read_json(g.root / "validation" / "model_matrix.json", "model matrix")
    problems += vv_models.check_model_matrix(mm)
    committed = {r.get("model") for r in mm.get("rows", [])} if isinstance(mm, dict) else set()
    problems += [f"model {m['id']} has no row in model_matrix.json" for m in vv_models.load_models(g.root)["models"] if m["id"] not in committed]
    if problems:
        shown = "; ".join(problems[:5]) + (f"; and {len(problems) - 5} more" if len(problems) > 5 else "")
        return False, f"{len(problems)} problems: {shown}"
    return True, f"{len(mm['rows'])} models, each with a row, existing verification evidence and one class per record"


def check_not_stale(g: Gate):
    """VAL-037: the committed matrices equal a regeneration from the tree; a changed input makes rows STALE."""
    matrix = read_json(g.matrix, "V&V matrix")
    mm = read_json(g.root / "validation" / "model_matrix.json", "model matrix")
    stale = vv_models.staleness(g.root, mm, matrix)
    if stale:
        shown = "; ".join(stale[:4]) + (f"; and {len(stale) - 4} more" if len(stale) > 4 else "")
        return False, f"{len(stale)} stale: {shown}. Regenerate with `python3 -m validation matrix` and `python3 -m validation models`, rerun the affected benchmarks, then commit"
    return True, "both matrices equal a regeneration from the registry, models, cases, rules and model sources"


def check_claims(g: Gate):
    """VAL-006: no verdict in the report's matrix is a hand edit, and no validated row lacks a passing case record."""
    if g.vv_report is None:
        raise ValueError("no V&V report directory given; run `python3 -m validation report --out DIR --release-record FILE`")
    report = read_json(g.vv_report / "report.json", "V&V report")
    problems = vv_matrix.check_claims(report.get("matrix") or {}, report.get("cases") or [], _registry(g))
    if problems:
        shown = "; ".join(problems[:4]) + (f"; and {len(problems) - 4} more" if len(problems) > 4 else "")
        return False, f"{len(problems)} problems: {shown}"
    rows = report["matrix"]["rows"]
    claimed = sum(1 for r in rows if r["status"].startswith("validated"))
    return True, f"every one of {len(rows)} rows equals the value derived from the case records; {claimed} claim validation, each with a case record"


def check_wording(g: Gate):
    findings = wording_lint.run(g.root, g.terms or wording_lint.load_terms())
    if findings:
        return False, f"{len(findings)} wording lint findings, first: {findings[0].render()}"
    return True, "wording lint is clean"


def _evidence(g: Gate, name: str, what: str) -> dict:
    data = read_json(g.evidence[name], what)
    if not g.commit:
        raise ValueError("cannot tell which commit is being released (pass --commit, or run inside the git checkout)")
    if not isinstance(data, dict) or data.get("commit") != g.commit:
        raise ValueError(f"{what} is for commit {str(data.get('commit') if isinstance(data, dict) else None)[:12]}, not {g.commit[:12]}")
    return data


def _checks(name: str, what: str):
    def check(g: Gate):
        data = _evidence(g, name, what)
        checks = data.get("checks")
        if not isinstance(checks, list) or not checks:
            raise ValueError(f"{what} lists no checks")
        bad = [str(c.get("name")) if isinstance(c, dict) else "?" for c in checks if not isinstance(c, dict) or c.get("verdict") != "PASS"]
        if bad:
            return False, f"{len(bad)} of {len(checks)} checks are not PASS: " + ", ".join(bad[:5])
        return True, f"all {len(checks)} checks are PASS for commit {g.commit[:12]}"
    return check


def check_fuzz(g: Gate):
    """QA-033: every fuzz target in the repository ran for an hour with libFuzzer and found nothing.

    Evidence is what scripts/fuzz_all.sh writes: the usual checks (one per target, verdict PASS) with
    `seconds` and `crashes`, plus `driver` and `dirty`. The smoke driver and a dirty tree do not count.
    """
    data = _evidence(g, "fuzz", "fuzz evidence")
    checks = data.get("checks")
    if not isinstance(checks, list) or not checks:
        raise ValueError("fuzz evidence lists no checks")
    targets = sorted(p.stem for p in (g.root / "fuzz" / "fuzz_targets").glob("*.rs"))
    if not targets:
        return False, "the repository has no fuzz targets (fuzz/fuzz_targets)"
    problems = []
    if data.get("driver") != "libfuzzer":
        problems.append(f"driver is {data.get('driver')!r}, not libfuzzer")
    if data.get("dirty") is not False:
        problems.append("the fuzzed tree was not clean")
    by_name = {c.get("name"): c for c in checks if isinstance(c, dict)}
    for target in targets:
        c = by_name.get(target)
        if c is None:
            problems.append(f"{target} was not fuzzed")
        elif c.get("verdict") != "PASS" or c.get("crashes") != 0:
            problems.append(f"{target} did not finish clean")
        elif not isinstance(c.get("seconds"), int) or c["seconds"] < FUZZ_SECONDS:
            problems.append(f"{target} ran {c.get('seconds')} s, under {FUZZ_SECONDS} s")
    if problems:
        return False, f"{len(problems)} problems, first: {problems[0]}"
    return True, f"all {len(targets)} fuzz targets ran at least {FUZZ_SECONDS} s with 0 crashes for commit {g.commit[:12]}"


def check_repro(g: Gate):
    data = _evidence(g, "repro", "reproducible-build evidence")
    builds = data.get("builds")
    if not isinstance(builds, list) or not builds:
        raise ValueError("reproducible-build evidence lists no builds")
    bad = [str(b.get("artifact")) for b in builds
           if not (SHA256.match(str(b.get("sha256_a"))) and b.get("sha256_a") == b.get("sha256_b"))]
    if bad:
        return False, f"{len(bad)} of {len(builds)} artifacts differ between two builds or lack hashes: " + ", ".join(bad[:5])
    return True, f"{len(builds)} artifacts build to identical hashes twice"


def check_defects(g: Gate):
    data = read_json(g.evidence["defects"], "defect log")
    defects = data.get("defects") if isinstance(data, dict) else None
    if not isinstance(defects, list):
        raise ValueError("the defect log has no 'defects' list")
    blocking = []
    for d in defects:
        if d.get("status") != "open" or d.get("severity") not in ("S1", "S2"):
            continue
        if d["severity"] == "S1":
            blocking.append(f"{d.get('id')} (open S1, cannot be waived)")
        else:
            problem = waiver_problem(d.get("waiver"), g.today)
            if problem:
                blocking.append(f"{d.get('id')} (open S2: {problem})")
    if blocking:
        return False, f"{len(blocking)} blocking defects: " + "; ".join(blocking)
    return True, f"{len(defects)} defects logged; no open S1 and no open S2 without a valid waiver"


def waiver_problem(waiver, today: date) -> str | None:
    if not isinstance(waiver, dict):
        return "no waiver recorded"
    missing = [k for k in ("owner", "reason", "expiry") if not str(waiver.get(k) or "").strip()]
    if missing:
        return "waiver lacks " + ", ".join(missing)
    try:
        expiry = date.fromisoformat(waiver["expiry"])
    except (TypeError, ValueError):
        return "waiver expiry is not a YYYY-MM-DD date"
    if expiry < today:
        return f"waiver expired on {expiry}"
    if (expiry - today).days > MAX_WAIVER_DAYS:
        return f"waiver expiry {expiry} is more than {MAX_WAIVER_DAYS} days away"
    return None


CONDITIONS = (
    ("gate requirements Met", "NEXT_RELEASE_PLAN", check_gate_ids_met),
    ("requirement-to-test trace complete", "QA-002", check_trace),
    ("V&V matrix has no blank row", "VAL-051", check_matrix),
    ("every reported model has a V&V row", "VAL-001", check_model_rows),
    ("V&V matrices are not stale", "VAL-037", check_not_stale),
    ("V&V report exists and its hash is recorded", "VAL-091", check_vv_report),
    ("V&V verdicts are derived from case records", "VAL-006", check_claims),
    ("wording lint clean", "wording lint", check_wording),
    ("all tests pass", "QA-070", _checks("tests", "test evidence")),
    ("performance gates pass", "PERF-051", _checks("perf", "performance evidence")),
    ("no open S1, no open S2 without a waiver", "QA-083", check_defects),
    ("fuzz clean", "QA-033", check_fuzz),
    ("reproducible build", "QA-063", check_repro),
    ("documentation tests pass", "QA-016", _checks("doc_tests", "documentation-test evidence")),
)


def run_gate(g: Gate) -> list[tuple[bool, str, str, str]]:
    results = []
    for name, ident, check in CONDITIONS:
        try:
            passed, reason = check(g)
        except (ValueError, OSError, KeyError, TypeError, AttributeError) as e:
            passed, reason = False, str(e) if isinstance(e, ValueError) else f"could not be checked ({type(e).__name__}: {e})"
        results.append((passed, name, ident, reason))
    return results


def render(results) -> str:
    lines = [f"{'PASS' if p else 'FAIL'}  {name} ({ident}): {reason}" for p, name, ident, reason in results]
    ok = sum(1 for r in results if r[0])
    lines.append(f"release gate: {'PASS' if ok == len(results) else 'FAIL'}, {ok} of {len(results)} conditions pass")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--commit", help="the commit being released (default: HEAD of --root)")
    ap.add_argument("--matrix", type=Path, help="V&V matrix (default: validation/matrix.json under --root)")
    ap.add_argument("--vv-report", type=Path, help="directory holding report.json and report.md")
    ap.add_argument("--release-record", type=Path, help="JSON release record holding vv_report_sha256")
    ap.add_argument("--tests", type=Path, help="test results evidence")
    ap.add_argument("--perf", type=Path, help="performance gate evidence")
    ap.add_argument("--defects", type=Path, help="defect log")
    ap.add_argument("--fuzz", type=Path, help="fuzz results evidence")
    ap.add_argument("--repro", type=Path, help="reproducible-build evidence")
    ap.add_argument("--doc-tests", dest="doc_tests", type=Path, help="documentation test evidence")
    ap.add_argument("--archive", type=Path, help="also write the output here, for the release record")
    args = ap.parse_args(argv)
    results = run_gate(Gate(args.root, args))
    text = render(results)
    sys.stdout.write(text)
    if args.archive:
        args.archive.write_text(text, encoding="utf-8")
    return 0 if all(r[0] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
