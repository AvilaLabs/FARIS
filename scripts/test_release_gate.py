# SPDX-License-Identifier: AGPL-3.0-only
import contextlib
import hashlib
import io
import json
import sys
import tempfile
import unittest
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_gate as rg  # noqa: E402
import wording_lint  # noqa: E402
from validation import matrix as vv_matrix  # noqa: E402

# The repository's terms without its allow list, whose entries are unused in a fixture tree.
TERMS = {**wording_lint.load_terms(), "allow_list": []}
COMMIT = "c" * 40
TODAY = date(2026, 10, 13)
MARK = "# Verif" + "ies: "  # kept apart so the trace scan does not read this file's fixtures as markers

PLAN = """# Plan

## Release gate

These IDs must be Met:

- **First:** AAA-001, AAA-002.
- **Second:** BBB-001
  (or its access outcome recorded).

GEO-010 is targeted but not in the gate.

## Out of scope
"""


def row(rid, now):
    return f"| {rid} | text | target | method | basis | F1 | {now} |\n"


def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def good_matrix():
    cells = {c: "x" for c in vv_matrix.CELLS}
    return {"schema": vv_matrix.SCHEMA, "quantities": ["q1", "q2"], "rows": [
        dict(cells, quantity="q1", benchmark_id="case/resp", status="code-to-code only"),
        dict(cells, quantity="q2", benchmark_id="none", status="not validated")]}


def evidence(kind="checks", commit=COMMIT):
    if kind == "builds":
        return {"commit": commit, "builds": [{"artifact": "faris", "sha256_a": "a" * 64, "sha256_b": "a" * 64}]}
    return {"commit": commit, "checks": [{"name": "suite", "verdict": "PASS"}]}


def make_root(d: Path, now="Met", traced=True, matrix=None):
    write(d / "docs" / "NEXT_RELEASE_PLAN.md", PLAN)
    write(d / "docs" / "requirements" / "01-a.md", "".join(row(r, now) for r in ("AAA-001", "AAA-002", "BBB-001", "GEO-010")))
    ids = "AAA-001, AAA-002, BBB-001" if traced else "AAA-001"
    write(d / "scripts" / "test_fixture.py", f"{MARK}{ids}\ndef test_x():\n    pass\n")
    write(d / "validation" / "matrix.json", json.dumps(matrix or good_matrix()))


def make_report(d: Path, report=None):
    """A V&V report directory and the release record that carries its hash."""
    m = good_matrix()
    report = report or {"methods": {"a": "b"}, "matrix": m, "cases": [{"case_id": "c1"}], "qualified_ranges": {"c1": {}},
                        "known_issues": {"c1": []}, "last_run": {"c1": "never run"}}
    out = d / "vv"
    out.mkdir()
    (out / "report.json").write_text(json.dumps(report), encoding="utf-8")
    (out / "report.md").write_text("# FARIS validation report\n", encoding="utf-8")
    record = d / "record.json"
    write(record, json.dumps({"vv_report_sha256": hashlib.sha256((out / "report.json").read_bytes()).hexdigest(),
                              "vv_report_md_sha256": hashlib.sha256((out / "report.md").read_bytes()).hexdigest()}))
    return out, record


def passing_inputs(d: Path) -> dict:
    out, record = make_report(d)
    paths = {}
    for name, kind in (("tests", "checks"), ("perf", "checks"), ("fuzz", "checks"), ("doc_tests", "checks"), ("repro", "builds")):
        paths[name] = d / f"{name}.json"
        write(paths[name], json.dumps(evidence(kind)))
    paths["defects"] = d / "defects.json"
    write(paths["defects"], json.dumps({"defects": [{"id": "D-1", "severity": "S3", "status": "open"}]}))
    return dict(paths, vv_report=out, release_record=record, commit=COMMIT, today=TODAY, terms=TERMS)


def results(gate):
    return {name: (passed, reason) for passed, name, _, reason in rg.run_gate(gate)}


class ReleaseGate(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.d = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    # Verifies: QA-070
    def test_a_gate_with_every_input_good_passes_every_condition(self):
        make_root(self.d)
        res = rg.run_gate(rg.Gate(self.d, **passing_inputs(self.d)))
        self.assertEqual([r for r in res if not r[0]], [])
        self.assertEqual(len(res), len(rg.CONDITIONS))
        self.assertIn("release gate: PASS", rg.render(res))

    # Verifies: QA-070
    def test_an_unmet_requirement_fails_the_gate_and_the_exit_code_is_nonzero(self):
        make_root(self.d, now="Partial")
        inputs = passing_inputs(self.d)
        res = results(rg.Gate(self.d, **inputs))
        passed, reason = res["gate requirements Met"]
        self.assertFalse(passed)
        self.assertIn("0 of 3 gate requirements are Met", reason)
        self.assertIn("AAA-001 (Partial)", reason)
        self.assertNotIn("GEO-010", reason)  # the paragraph after the list is not part of the gate
        argv = ["--root", str(self.d), "--commit", COMMIT, "--vv-report", str(inputs["vv_report"]), "--release-record", str(inputs["release_record"])]
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(rg.main(argv), 1)
        self.assertIn("FAIL  gate requirements Met", out.getvalue())

    # Verifies: QA-070
    def test_the_gate_ids_are_parsed_from_the_plan(self):
        make_root(self.d)
        self.assertEqual(rg.gate_ids(self.d), ["AAA-001", "AAA-002", "BBB-001"])
        write(self.d / "docs" / "NEXT_RELEASE_PLAN.md", "# Plan\n")
        with self.assertRaises(ValueError):
            rg.gate_ids(self.d)

    # Verifies: QA-070
    def test_trace_gap_and_unknown_marker_fail(self):
        make_root(self.d, traced=False)
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["requirement-to-test trace complete"]
        self.assertFalse(passed)
        self.assertIn("none for AAA-002, BBB-001", reason)
        write(self.d / "scripts" / "test_more.py", f"{MARK}ZZZ-999\n")
        self.assertIn("unknown requirements: ZZZ-999", results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["requirement-to-test trace complete"][1])

    # Verifies: QA-070
    def test_missing_evidence_fails_and_never_passes_by_default(self):
        make_root(self.d)
        res = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))
        for name in ("all tests pass", "performance gates pass", "fuzz clean", "reproducible build", "documentation tests pass",
                     "no open S1, no open S2 without a waiver", "V&V report exists and its hash is recorded"):
            self.assertFalse(res[name][0], name)
            self.assertIn("no ", res[name][1])

    # Verifies: QA-070
    def test_evidence_must_name_the_commit_and_every_check_must_pass(self):
        make_root(self.d)
        inputs = passing_inputs(self.d)
        write(inputs["tests"], json.dumps(evidence(commit="d" * 40)))
        passed, reason = results(rg.Gate(self.d, **inputs))["all tests pass"]
        self.assertFalse(passed)
        self.assertIn("not " + COMMIT[:12], reason)
        write(inputs["tests"], json.dumps({"commit": COMMIT, "checks": [{"name": "a", "verdict": "PASS"}, {"name": "b", "verdict": "FAIL"}]}))
        self.assertIn("1 of 2 checks are not PASS: b", results(rg.Gate(self.d, **inputs))["all tests pass"][1])
        write(inputs["tests"], json.dumps({"commit": COMMIT, "checks": []}))
        self.assertFalse(results(rg.Gate(self.d, **inputs))["all tests pass"][0])
        write(inputs["tests"], "not json")
        self.assertFalse(results(rg.Gate(self.d, **inputs))["all tests pass"][0])

    # Verifies: QA-070
    def test_reproducible_build_needs_equal_hashes(self):
        make_root(self.d)
        inputs = passing_inputs(self.d)
        bad = evidence("builds")
        bad["builds"][0]["sha256_b"] = "b" * 64
        write(inputs["repro"], json.dumps(bad))
        passed, reason = results(rg.Gate(self.d, **inputs))["reproducible build"]
        self.assertFalse(passed)
        self.assertIn("faris", reason)

    # Verifies: QA-070
    def test_defects_open_s1_blocks_and_open_s2_needs_a_valid_waiver(self):
        make_root(self.d)
        inputs = passing_inputs(self.d)
        name = "no open S1, no open S2 without a waiver"

        def gate_with(defects):
            write(inputs["defects"], json.dumps({"defects": defects}))
            return results(rg.Gate(self.d, **inputs))[name]

        soon = (TODAY + timedelta(days=30)).isoformat()
        waiver = {"owner": "CA", "reason": "documented workaround", "expiry": soon}
        self.assertTrue(gate_with([{"id": "D-2", "severity": "S2", "status": "open", "waiver": waiver}, {"id": "D-3", "severity": "S2", "status": "fixed"}])[0])
        self.assertIn("cannot be waived", gate_with([{"id": "D-1", "severity": "S1", "status": "open", "waiver": waiver}])[1])
        self.assertIn("no waiver recorded", gate_with([{"id": "D-2", "severity": "S2", "status": "open"}])[1])
        for bad, text in (({**waiver, "owner": " "}, "lacks owner"), ({**waiver, "expiry": (TODAY - timedelta(days=1)).isoformat()}, "expired"),
                          ({**waiver, "expiry": (TODAY + timedelta(days=91)).isoformat()}, "more than 90 days"), ({**waiver, "expiry": "soon"}, "not a YYYY-MM-DD")):
            self.assertIn(text, gate_with([{"id": "D-2", "severity": "S2", "status": "open", "waiver": bad}])[1])

    # Verifies: VAL-051
    def test_a_blank_matrix_cell_blocks_the_gate(self):
        bad = good_matrix()
        bad["rows"][0]["bias"] = "  "
        make_root(self.d, matrix=bad)
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["V&V matrix has no blank row"]
        self.assertFalse(passed)
        self.assertIn("blank cell bias", reason)

    # Verifies: VAL-051
    def test_the_gate_reports_a_good_matrix_and_a_missing_one(self):
        make_root(self.d)
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["V&V matrix has no blank row"]
        self.assertTrue(passed)
        self.assertIn("1 rows labelled not validated", reason)
        (self.d / "validation" / "matrix.json").unlink()
        self.assertFalse(results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["V&V matrix has no blank row"][0])

    # Verifies: VAL-091
    def test_the_gate_fails_without_a_report_or_a_matching_recorded_hash(self):
        make_root(self.d)
        name = "V&V report exists and its hash is recorded"
        out, record = make_report(self.d)
        self.assertTrue(results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][0])
        self.assertFalse(results(rg.Gate(self.d, release_record=record, commit=COMMIT, terms=TERMS))[name][0])
        (out / "report.json").write_text("{}", encoding="utf-8")  # changed after the hash was recorded
        self.assertIn("differs from the release record", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])
        (out / "report.json").unlink()
        self.assertIn("does not exist", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])

    # Verifies: VAL-091
    def test_the_gate_fails_on_a_report_missing_a_required_section_or_hash(self):
        make_root(self.d)
        name = "V&V report exists and its hash is recorded"
        incomplete = {"methods": {"a": "b"}, "matrix": good_matrix(), "cases": [{"case_id": "c1"}], "qualified_ranges": {"c1": {}}, "known_issues": {"c1": []}}
        out, record = make_report(self.d, incomplete)
        self.assertIn("lacks last_run", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])
        write(record, json.dumps({}))
        self.assertIn("no vv_report_sha256", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])

    def test_wording_lint_findings_fail_the_gate(self):
        make_root(self.d)
        write(self.d / "README.md", "FARIS is fully validated against every benchmark.\n")
        res = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["wording lint clean"]
        self.assertFalse(res[0])

    def test_archive_flag_writes_the_gate_output(self):
        make_root(self.d)
        archive = self.d / "gate.txt"
        with contextlib.redirect_stdout(io.StringIO()):
            rg.main(["--root", str(self.d), "--commit", COMMIT, "--archive", str(archive)])
        self.assertIn("release gate: FAIL", archive.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
