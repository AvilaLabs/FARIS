#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""The release gate (requirement QA-070): one script that blocks a release unless every condition passes.

    python3 scripts/release_gate.py --vv-report DIR --release-record FILE \\
        --tests FILE --perf FILE --defects FILE --fuzz FILE --repro FILE --doc-tests FILE [--archive FILE]

It prints one line per condition, PASS or FAIL with the reason, and exits 1 unless all pass.

Conditions read from the repository (--root, default this checkout):
- every requirement ID in the "Release gate" list of docs/NEXT_RELEASE_PLAN.md is Met in the requirement
  tables (the list is parsed, never copied), except VAL-023 may use the plan's explicit access-outcome branch;
- the requirement-to-test trace is complete for those IDs (QA-002): each has a Verifies: marker and no
  marker names an unknown requirement;
- the V&V matrix (validation/matrix.json) has no blank cell, and a quantity with no benchmark is labelled
  "not validated" with a reason (VAL-051);
- every model and calculated output FARIS reports has a row in the model matrix (validation/model_matrix.json),
  its verification evidence exists, and every evidence record has exactly one class (VAL-001);
- neither matrix is stale: both equal a regeneration from the registry, models, case manifests, rules and model
  source files in the tree (VAL-037, `python3 -m validation matrix` and `python3 -m validation models`);
- wording lint is clean;
- every requirement marked Met anywhere in the tables has a Verifies: marker in a test (QA-071);
- no accessibility check of A11Y-001 to A11Y-046 that passed in the previous release's snapshot (--a11y-baseline)
  fails or goes unrun in this release's test evidence (A11Y-055, scripts/a11y_regression.py);
- the waivers file docs/release-waivers.json is complete and unexpired (QA-072).

An open S2 defect is waived in the same file with its defect id (FARIS-NNN) as the condition, or by a "waiver"
on its defect-log entry. A condition that fails can be waived for one release by an entry in docs/release-waivers.json:
{"waivers": [{"condition": NAME OR ID, "owner": ..., "reason": ..., "expiry": "YYYY-MM-DD"}]}. The waiver needs
all three fields, must not be expired, and must expire within 90 days. A waived condition prints WAIVED with the
original failure and counts as passing; an open S1 defect (QA-083) and the waiver check itself cannot be waived.

Repository checks (PLAT-073). Unless --no-repo-checks is given, the gate also runs every check that CI runs on
the tree and a few that need a build: the requirement scorecard, trace and table lints, source lint, repository
size, link check, glossary, changelog, deprecations, known-issues page, citation file, the licence policy, the
advisory check, the SBOM generation and validation, the current notices, and the command-line reference
(--faris BINARY). Each is one script run with this checkout as the working directory; a failing script, a script
that cannot run and a missing input (no --faris) all FAIL.

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
"YYYY-MM-DD"}}]}: an open S1 or any data-loss defect that is not fixed always blocks (QA-083, REL-044), an open S2 needs a complete waiver that has not
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
import tempfile
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
        self.validation_access_outcome: Path | None = None
        self.evidence = {name: None for name in ("tests", "perf", "defects", "fuzz", "repro", "doc_tests", "a11y_baseline")}
        self.repo_checks = False  # the command line turns these on; run_gate on a fixture tree leaves them off
        self.faris: Path | None = None
        self.cargo = "cargo"
        self._metadata: dict[str, Path] = {}
        for source in (vars(args) if args else {}, overrides):
            for key, value in source.items():
                if key in self.evidence:
                    self.evidence[key] = Path(value) if value else None
                elif key in ("today", "commit", "terms", "matrix", "vv_report", "release_record", "validation_access_outcome", "faris", "cargo", "repo_checks") and value is not None:
                    setattr(self, key, Path(value) if key in ("matrix", "vv_report", "release_record", "validation_access_outcome", "faris") else value)
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
    access_branch = val023_access_branch_authorized(g.root)
    unknown = [i for i in ids if i not in status]
    access_recorded = False
    not_met = []
    for rid in ids:
        if rid not in status or status[rid] == "Met":
            continue
        if rid == "VAL-023" and access_branch:
            passed, _ = check_val023_access_outcome(g)
            if passed:
                access_recorded = True
                continue
        not_met.append(rid)
    met = len(ids) - len(unknown) - len(not_met) - int(access_recorded)
    if unknown or not_met:
        parts = [f"{met} of {len(ids)} gate requirements are Met"]
        if not_met:
            parts.append("not Met: " + ", ".join(f"{i} ({status[i]})" for i in not_met))
            if "VAL-023" in not_met and access_branch:
                _, access_reason = check_val023_access_outcome(g)
                parts.append("VAL-023 access outcome not accepted: " + access_reason)
        if unknown:
            parts.append("not in any requirement table: " + ", ".join(unknown))
        if access_recorded:
            parts.append("access outcome recorded; VAL-023 remains NOT_EVALUATED")
        return False, "; ".join(parts)
    if access_recorded:
        return True, f"{met} gate requirements are Met; access outcome recorded; VAL-023 remains NOT_EVALUATED"
    return True, f"all {len(ids)} gate requirements are Met"


def val023_access_branch_authorized(root: Path) -> bool:
    """Only the exact VAL-023 alternative approved in the current release plan authorizes this branch."""
    try:
        lines = (root / PLAN).read_text(encoding="utf-8").splitlines()
    except OSError:
        return False
    try:
        start = next(i for i, line in enumerate(lines) if re.match(r"^##\s+Release gate\s*$", line))
    except StopIteration:
        return False
    section = []
    for line in lines[start + 1:]:
        if line.startswith("## "):
            break
        section.append(line)
    body = "\n".join(section)
    return bool(re.search(r"(?m)^-\s+\*\*Validation:\*\*[^\n]*\bVAL-023 \(or its access outcome recorded\)", body))


def check_val023_access_outcome(g: Gate) -> tuple[bool, str]:
    """Check the narrowly approved access alternative without turning it into a physical verdict."""
    if not val023_access_branch_authorized(g.root):
        return False, "the current release plan does not authorize the VAL-023 access-outcome alternative"
    status = read_statuses(g.root).get("VAL-023")
    if status is None:
        return False, "VAL-023 is not present in the requirement tables"
    if status == "Met":
        return False, "VAL-023 is Met; the access-outcome alternative is not applicable"
    try:
        matrix = read_json(g.matrix, "V&V matrix")
    except (ValueError, OSError) as e:
        return False, str(e)
    rows = [r for r in matrix.get("rows", []) if isinstance(r, dict) and r.get("quantity") == "shutdown_dose_rate"] if isinstance(matrix, dict) else []
    if not rows or any(r.get("status") != vv_matrix.NOT_VALIDATED for r in rows):
        return False, "the V&V matrix must contain shutdown_dose_rate rows and every such row must remain 'not validated'"
    try:
        matrix_problems = vv_matrix.check_matrix(matrix)
    except (TypeError, AttributeError) as e:
        return False, f"the V&V matrix is malformed: {e}"
    if matrix_problems:
        return False, "the V&V matrix is invalid: " + "; ".join(matrix_problems[:3])

    try:
        artifact = read_json(g.validation_access_outcome, "VAL-023 validation access-outcome evidence")
        required = {"schema", "requirement_id", "outcome", "status", "matrix_status", "reason", "next_step", "observations", "supporting_files"}
        if not isinstance(artifact, dict) or set(artifact) != required:
            return False, "access-outcome evidence must contain exactly the documented schema fields"
        if artifact["schema"] != "faris.validation-access-outcome/1.0.0":
            return False, "access-outcome evidence has an unsupported schema"
        if artifact["requirement_id"] != "VAL-023":
            return False, "access-outcome evidence must name VAL-023"
        if artifact["outcome"] != "access_not_obtained":
            return False, "the approved alternative requires outcome 'access_not_obtained'"
        if artifact["status"] != "NOT_EVALUATED" or artifact["matrix_status"] != vv_matrix.NOT_VALIDATED:
            return False, "an access outcome must leave VAL-023 NOT_EVALUATED and the matrix not validated"
        for field in ("reason", "next_step"):
            if not isinstance(artifact[field], str) or not artifact[field].strip():
                return False, f"access-outcome evidence needs a non-empty {field}"
        observations = artifact["observations"]
        if not isinstance(observations, list) or not observations:
            return False, "access-outcome evidence needs at least one dated observation"
        for i, observation in enumerate(observations):
            if not isinstance(observation, dict) or set(observation) != {"date", "observation"}:
                return False, f"observation {i + 1} must contain exactly date and observation"
            try:
                if not isinstance(observation["date"], str) or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", observation["date"]):
                    return False, f"observation {i + 1} needs a YYYY-MM-DD date"
                observed = date.fromisoformat(observation["date"])
            except (TypeError, ValueError):
                return False, f"observation {i + 1} needs a YYYY-MM-DD date"
            if observed > g.today:
                return False, f"observation {i + 1} is dated after the gate date"
            if not isinstance(observation["observation"], str) or not observation["observation"].strip():
                return False, f"observation {i + 1} needs non-empty text"
        supporting_files = artifact["supporting_files"]
        if not isinstance(supporting_files, list) or not supporting_files:
            return False, "access-outcome evidence needs at least one hash-pinned supporting file"
        base = g.validation_access_outcome.resolve().parent
        for i, item in enumerate(supporting_files):
            if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
                return False, f"supporting file {i + 1} must contain exactly path and sha256"
            rel = Path(item["path"]) if isinstance(item["path"], str) else Path(".")
            if rel.is_absolute() or ".." in rel.parts or not isinstance(item["path"], str):
                return False, f"supporting file {i + 1} path must stay relative to the evidence file"
            path = (base / rel).resolve()
            if not path.is_relative_to(base) or not path.is_file():
                return False, f"supporting file {item['path']!r} is absent or outside the evidence directory"
            if not isinstance(item["sha256"], str) or not SHA256.fullmatch(item["sha256"]):
                return False, f"supporting file {item['path']!r} needs a 64-character SHA-256"
            actual = sha256_file(path)
            if actual != item["sha256"]:
                return False, f"supporting file {item['path']!r} hash mismatch"
    except (ValueError, OSError, TypeError, KeyError) as e:
        return False, str(e)
    return True, "access outcome recorded; VAL-023 remains NOT_EVALUATED"


def check_trace(g: Gate):
    ids = gate_ids(g.root)
    named, empty = read_markers(g.root)
    known = set(read_statuses(g.root))
    untraced = [i for i in ids if i not in named]
    access_recorded = False
    if "VAL-023" in untraced and val023_access_branch_authorized(g.root):
        access_recorded, _ = check_val023_access_outcome(g)
        if access_recorded:
            untraced.remove("VAL-023")
    unknown = sorted(named - known)
    problems = []
    if untraced:
        marker_count = len(ids) - len(untraced) - int(access_recorded)
        detail = f"{marker_count} of {len(ids)} gate requirements have a test marker; none for " + ", ".join(untraced)
        if access_recorded:
            detail += "; VAL-023 access-outcome checker is recorded separately and does not claim physical test coverage"
        problems.append(detail)
    if unknown:
        problems.append("markers name unknown requirements: " + ", ".join(unknown))
    if empty:
        problems.append("markers naming no requirement: " + ", ".join(empty))
    if problems:
        return False, "; ".join(problems)
    if access_recorded:
        return True, (f"{len(ids) - 1} gate requirements have test markers; VAL-023 access-outcome checker is recorded separately; "
                      "this does not claim physical test coverage or Met status")
    return True, f"all {len(ids)} gate requirements have a test marker"


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


def check_a11y(g: Gate):
    """A11Y-055: no accessibility check that passed in the previous release fails or goes unrun in this one."""
    import a11y_regression
    baseline = read_json(g.evidence["a11y_baseline"], "accessibility snapshot of the previous release")
    current = a11y_regression.snapshot(_evidence(g, "tests", "test evidence"), g.root)
    new, known = a11y_regression.compare(baseline, current)
    if new:
        return False, f"{len(new)} new accessibility failures: " + "; ".join(new[:5])
    return True, f"no new accessibility failures against the snapshot of {str(baseline.get('commit'))[:12]} ({len(known)} known gaps)"


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
    by_name = {}
    for check in checks:
        if not isinstance(check, dict):
            problems.append("a fuzz check is not an object")
            continue
        name = check.get("name")
        if not isinstance(name, str) or name not in targets:
            problems.append(f"invalid or unexpected fuzz target name {name!r}")
            continue
        if name in by_name:
            problems.append(f"{name} has duplicate fuzz evidence")
        by_name[name] = check
    for target in targets:
        c = by_name.get(target)
        if c is None:
            problems.append(f"{target} was not fuzzed")
        elif (c.get("verdict") != "PASS" or type(c.get("crashes")) is not int or
              c.get("crashes") != 0 or type(c.get("exit_status")) is not int or
              c.get("exit_status") != 0):
            problems.append(f"{target} did not finish clean")
        elif type(c.get("seconds")) is not int or c["seconds"] < FUZZ_SECONDS:
            problems.append(f"{target} ran {c.get('seconds')} s, under {FUZZ_SECONDS} s")
        elif type(c.get("cpu_seconds")) is not int or c["cpu_seconds"] < FUZZ_SECONDS:
            problems.append(f"{target} used {c.get('cpu_seconds')} CPU s, under {FUZZ_SECONDS}")
    if problems:
        return False, f"{len(problems)} problems, first: {problems[0]}"
    return True, (f"all {len(targets)} fuzz targets ran and used at least {FUZZ_SECONDS} s each, "
                  f"with 0 crashes for commit {g.commit[:12]}")


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
    filed = {w.get("condition"): w for w in read_waivers(g.root) if isinstance(w, dict)}
    for d in defects:
        if d.get("data_loss") is True and d.get("status") != "fixed":
            blocking.append(f"{d.get('id')} (data loss is {d.get('status')}, cannot be waived)")
            continue
        if d.get("status") != "open" or d.get("severity") not in ("S1", "S2"):
            continue
        if d["severity"] == "S1":
            blocking.append(f"{d.get('id')} (open S1, cannot be waived)")
        else:
            # The waiver is on the defect entry or, with the other waivers, in docs/release-waivers.json under
            # the defect's id (QA-072).
            problem = waiver_problem(d.get("waiver") or filed.get(d.get("id")), g.today)
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


def check_met_rows_traced(g: Gate):
    """QA-071: a requirement marked Met must have a test that names it. Whether that test passes is the
    "all tests pass" condition, whose evidence is the test run of the same commit."""
    status = read_statuses(g.root)
    named, _ = read_markers(g.root)
    met = sorted(i for i, st in status.items() if st == "Met")
    untraced = [i for i in met if i not in named]
    if untraced:
        return False, f"{len(met) - len(untraced)} of {len(met)} Met requirements have a test marker; none for " + ", ".join(untraced)
    return True, f"all {len(met)} Met requirements have a test marker"


WAIVERS = Path("docs") / "release-waivers.json"
UNWAIVABLE = {"QA-083", "QA-072"}
DEFECT_ID = re.compile(r"FARIS-\d{3,}")  # an open S2 defect is waived by its id (check_defects)


def read_waivers(root: Path) -> list[dict]:
    path = root / WAIVERS
    if not path.is_file():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    waivers = data.get("waivers") if isinstance(data, dict) else None
    if not isinstance(waivers, list):
        raise ValueError(f'{WAIVERS} must be {{"waivers": [...]}}')
    return waivers


def check_waivers(g: Gate):
    """QA-072: every waiver of a gate condition has owner, reason and expiry, is unexpired and expires within
    90 days, and names a condition of this gate that may be waived."""
    waivers = read_waivers(g.root)
    known = {c[0] for c in CONDITIONS} | {c[1] for c in CONDITIONS} | {c[0] for c in REPO_CHECKS} | {c[1] for c in REPO_CHECKS}
    problems = []
    for w in waivers:
        label = w.get("condition", "?") if isinstance(w, dict) else "?"
        why = waiver_problem(w, g.today)
        if why:
            problems.append(f"waiver for {label}: {why}")
        elif not str(w.get("condition") or "").strip():
            problems.append("a waiver names no condition")
        elif w["condition"] not in known and not DEFECT_ID.fullmatch(str(w["condition"])):
            problems.append(f"waiver names {w['condition']!r}, which is not a condition of this gate")
        elif w["condition"] in UNWAIVABLE:
            problems.append(f"{w['condition']} cannot be waived")
    if problems:
        return False, "; ".join(problems)
    return True, f"{len(waivers)} waivers, all complete, unexpired and within {MAX_WAIVER_DAYS} days"


# --- repository checks (PLAT-073): scripts run with the checkout as working directory --------------------

def _metadata(g: Gate, platform: str | None = None) -> Path:
    """`cargo metadata --locked --offline` of the checkout, written once per run (and per platform filter)."""
    key = platform or "all"
    if key not in g._metadata:
        argv = [*g.cargo.split(), "metadata", "--locked", "--offline", "--format-version", "1"]
        if platform:
            argv += ["--filter-platform", platform]
        out = subprocess.run(argv, cwd=g.root, capture_output=True, text=True, timeout=600)
        if out.returncode != 0:
            raise ValueError(f"{' '.join(argv)} failed: {out.stderr.strip()[-300:]}")
        tmp = Path(tempfile.mkdtemp(prefix="release-gate-")) / f"metadata-{key}.json"
        tmp.write_text(out.stdout, encoding="utf-8")
        g._metadata[key] = tmp
    return g._metadata[key]


def _script(name: str, *args: str):
    return lambda g: [sys.executable, str(g.root / "scripts" / name), *args]


def _sbom(g: Gate):
    out = tempfile.mkdtemp(prefix="release-gate-sbom-")
    return [sys.executable, str(g.root / "scripts" / "generate_sbom.py"), "--metadata", str(_metadata(g)), "--out", out, "--root", str(g.root)]


def _licences(g: Gate):
    return [sys.executable, str(g.root / "scripts" / "check_licences.py"), "--metadata", str(_metadata(g))]


def _cli(g: Gate):
    if g.faris is None:
        raise ValueError("no faris binary given (--faris); the command-line reference cannot be compared with its help")
    return [sys.executable, str(g.root / "scripts" / "cli_reference.py"), "--faris", str(g.faris), "--root", str(g.root), "--check"]


def _notices(g: Gate):
    """docs/THIRD_PARTY_NOTICES.md equals a fresh collection for the Linux graph (the file's own recipe)."""
    out = Path(tempfile.mkdtemp(prefix="release-gate-notices-")) / "notices.md"
    run = subprocess.run([sys.executable, str(g.root / "scripts" / "collect_dependency_notices.py"),
                          "--metadata", str(_metadata(g, "x86_64-unknown-linux-gnu")), "--output", str(out)],
                         cwd=g.root, capture_output=True, text=True, timeout=900)
    if run.returncode != 0:
        return False, "could not collect the notices: " + (run.stderr.strip().splitlines() or ["no message"])[-1]
    if out.read_bytes() != (g.root / "docs" / "THIRD_PARTY_NOTICES.md").read_bytes():
        return False, "docs/THIRD_PARTY_NOTICES.md differs from a fresh collection for the Linux graph; regenerate it"
    return True, "docs/THIRD_PARTY_NOTICES.md equals a fresh collection for the Linux graph"


# (name, ident, builder). A builder returns the argv of a script that exits non-zero on a problem, or a
# (passed, reason) pair.
REPO_CHECKS = (
    ("requirement scorecard is current", "QA-009", _script("requirement_scorecard.py", "--check")),
    ("requirement trace is current", "QA-002", _script("requirement_trace.py", "--check")),
    ("requirement tables follow the table rules", "QA-001", _script("requirement_lint.py")),
    ("source rules for the calculation crates", "QA-039", _script("source_lint.py")),
    ("tracked files stay small", "QA-090", _script("check_repository_size.py")),
    ("dependency licences fit AGPL-3.0", "LEG-003", _licences),
    ("no unwaived advisory", "SEC-031", _script("check_audit.py")),
    ("sign-in advisories fixed in time", "SEC-039", _script("advisory_log.py")),
    ("every fuzz finding has a reproducer and none is open", "SEC-022", _script("fuzz_findings.py", "--release")),
    ("SBOM generates and validates", "SEC-030", _sbom),
    ("dependency notices are current", "LEG-001", _notices),
    ("documentation links resolve", "DOC-026", _script("check_links.py")),
    ("glossary lint", "DOC-034", _script("check_glossary.py")),
    ("changelog layout", "DOC-040", _script("check_changelog.py")),
    ("deprecation register", "DOC-043", _script("check_deprecations.py")),
    ("known-issues page is current", "QA-082", _script("known_issues.py", "--check")),
    ("citation file is valid", "PRV-036", _script("check_citation.py")),
    ("command-line reference is current", "DOC-021", _cli),
)


def run_repo_check(g: Gate, name: str, ident: str, build) -> tuple[bool, str, str, str]:
    try:
        plan = build(g)
        if isinstance(plan, tuple):
            return plan[0], name, ident, plan[1]
        if not Path(plan[1]).is_file():
            return False, name, ident, f"{Path(plan[1]).name} does not exist in this tree"
        out = subprocess.run(plan, cwd=g.root, capture_output=True, text=True, timeout=1800)
    except (ValueError, OSError, subprocess.SubprocessError) as e:
        return False, name, ident, str(e) if isinstance(e, ValueError) else f"could not run ({type(e).__name__}: {e})"
    lines = [l for l in (out.stdout + "\n" + out.stderr).splitlines() if l.strip()]
    detail = lines[-1].strip() if lines else f"exit {out.returncode}, no output"
    if out.returncode != 0 and len(lines) > 1:
        detail = f"{lines[0].strip()} (and {len(lines) - 1} more lines); {detail}"
    return out.returncode == 0, name, ident, detail


def apply_waivers(g: Gate, results: list) -> list:
    """A failed, waivable condition with a valid waiver is reported WAIVED and counts as passing."""
    try:
        waivers = [w for w in read_waivers(g.root) if isinstance(w, dict) and not waiver_problem(w, g.today)]
    except (ValueError, OSError):
        return results  # the waiver condition reports the unreadable file
    out = []
    for passed, name, ident, reason in results:
        waiver = next((w for w in waivers if w.get("condition") in (name, ident)), None)
        if not passed and waiver and ident not in UNWAIVABLE:
            reason = f"WAIVED by {waiver['owner']} until {waiver['expiry']} ({waiver['reason']}); the condition fails: {reason}"
            passed = True
        out.append((passed, name, ident, reason))
    return out


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
    ("no new accessibility failures", "A11Y-055", check_a11y),
    ("fuzz clean", "QA-033", check_fuzz),
    ("reproducible build", "QA-063", check_repro),
    ("documentation tests pass", "QA-016", _checks("doc_tests", "documentation-test evidence")),
    ("every Met requirement has a test marker", "QA-071", check_met_rows_traced),
    ("gate waivers are complete and unexpired", "QA-072", check_waivers),
)


def run_gate(g: Gate) -> list[tuple[bool, str, str, str]]:
    results = []
    for name, ident, check in CONDITIONS:
        try:
            passed, reason = check(g)
        except (ValueError, OSError, KeyError, TypeError, AttributeError) as e:
            passed, reason = False, str(e) if isinstance(e, ValueError) else f"could not be checked ({type(e).__name__}: {e})"
        results.append((passed, name, ident, reason))
    if g.repo_checks:
        results += [run_repo_check(g, name, ident, build) for name, ident, build in REPO_CHECKS]
    return apply_waivers(g, results)


def render(results) -> str:
    lines = [f"{'WAIVED' if p and reason.startswith('WAIVED') else 'PASS' if p else 'FAIL'}  {name} ({ident}): {reason}"
             for p, name, ident, reason in results]
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
    ap.add_argument("--validation-access-outcome", type=Path,
                    help="hash-pinned evidence for the release plan's VAL-023 access-outcome alternative")
    ap.add_argument("--tests", type=Path, help="test results evidence")
    ap.add_argument("--perf", type=Path, help="performance gate evidence")
    ap.add_argument("--defects", type=Path, help="defect log")
    ap.add_argument("--fuzz", type=Path, help="fuzz results evidence")
    ap.add_argument("--repro", type=Path, help="reproducible-build evidence")
    ap.add_argument("--a11y-baseline", dest="a11y_baseline", type=Path,
                    help="accessibility snapshot of the previous release (scripts/a11y_regression.py snapshot)")
    ap.add_argument("--doc-tests", dest="doc_tests", type=Path, help="documentation test evidence")
    ap.add_argument("--archive", type=Path, help="also write the output here, for the release record")
    ap.add_argument("--faris", type=Path, help="the faris binary, for the command-line reference check")
    ap.add_argument("--cargo", default="cargo", help="the cargo command (default: cargo)")
    ap.add_argument("--no-repo-checks", dest="no_repo_checks", action="store_true",
                    help="skip the repository scripts (scorecard, links, SBOM, licences, ...); for fixture trees")
    args = ap.parse_args(argv)
    results = run_gate(Gate(args.root, args, repo_checks=not args.no_repo_checks))
    text = render(results)
    sys.stdout.write(text)
    if args.archive:
        args.archive.write_text(text, encoding="utf-8")
    return 0 if all(r[0] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
