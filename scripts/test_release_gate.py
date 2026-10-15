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
from validation import fixtures as vv_fixtures  # noqa: E402
from validation import matrix as vv_matrix  # noqa: E402
from validation import models as vv_models  # noqa: E402
from validation import report as vv_report  # noqa: E402

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


def good_suite(root: Path | None = None):
    """The suite of a small fixture tree (one case, no run record), written under `root` when given."""
    if root is not None:
        return vv_fixtures.write_tree(root)
    m = vv_fixtures.manifest()
    return vv_report.build_suite([m], {}, None, registry=vv_fixtures.FIXTURE_REGISTRY)


def good_matrix():
    return good_suite()["matrix"]


def evidence(kind="checks", commit=COMMIT):
    if kind == "builds":
        return {"commit": commit, "builds": [{"artifact": "faris", "sha256_a": "a" * 64, "sha256_b": "a" * 64}]}
    return {"commit": commit, "checks": [{"name": "suite", "verdict": "PASS"}]}


FUZZ_TARGETS = ("scenario", "facets")


def fuzz_evidence(**changes):
    checks = [{"name": t, "verdict": "PASS", "seconds": 3600, "executions": 10, "crashes": 0} for t in FUZZ_TARGETS]
    return {"commit": COMMIT, "driver": "libfuzzer", "dirty": False, "checks": checks, **changes}


def make_root(d: Path, now="Met", traced=True, matrix=None):
    write(d / "docs" / "NEXT_RELEASE_PLAN.md", PLAN)
    write(d / "docs" / "requirements" / "01-a.md", "".join(row(r, now) for r in ("AAA-001", "AAA-002", "BBB-001", "GEO-010")))
    ids = "AAA-001, AAA-002, BBB-001, GEO-010" if traced else "AAA-001"
    write(d / "scripts" / "test_fixture.py", f"{MARK}{ids}\ndef test_x():\n    pass\n")
    good_suite(d)
    if matrix:
        write(d / "validation" / "matrix.json", json.dumps(matrix))


def make_report(d: Path, report=None):
    """A V&V report directory and the release record that carries its hash."""
    report = report or good_suite(d / "tree")
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
    write(paths["fuzz"], json.dumps(fuzz_evidence()))
    for target in FUZZ_TARGETS:
        write(d / "fuzz" / "fuzz_targets" / f"{target}.rs", "")
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
    # Verifies: PLAT-073
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

    # Verifies: QA-033
    # Verifies: SEC-021
    def test_fuzz_evidence_needs_every_target_for_an_hour_from_libfuzzer_with_no_crash(self):
        make_root(self.d)
        inputs = passing_inputs(self.d)
        self.assertTrue(results(rg.Gate(self.d, **inputs))["fuzz clean"][0])

        def reason(**changes):
            write(inputs["fuzz"], json.dumps(fuzz_evidence(**changes)))
            passed, why = results(rg.Gate(self.d, **inputs))["fuzz clean"]
            self.assertFalse(passed)
            return why

        short = fuzz_evidence()["checks"]
        short[1] = dict(short[1], seconds=60)
        self.assertIn("facets ran 60 s", reason(checks=short))
        crashed = fuzz_evidence()["checks"]
        crashed[0] = dict(crashed[0], crashes=1, verdict="FAIL")
        self.assertIn("scenario did not finish clean", reason(checks=crashed))
        self.assertIn("facets was not fuzzed", reason(checks=fuzz_evidence()["checks"][:1]))
        self.assertIn("not libfuzzer", reason(driver="smoke"))
        self.assertIn("not clean", reason(dirty=True))
        self.assertIn("not " + COMMIT[:12], reason(commit="d" * 40))

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
    # Verifies: QA-072
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
        self.assertIn("2 rows labelled not validated", reason)
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
        incomplete = {k: v for k, v in good_suite(self.d / "tree").items() if k != "last_run"}
        out, record = make_report(self.d, incomplete)
        self.assertIn("lacks last_run", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])
        write(record, json.dumps({}))
        self.assertIn("no vv_report_sha256", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])

    # Verifies: VAL-001
    def test_a_reported_model_without_a_row_or_with_missing_evidence_blocks_the_gate(self):
        name = "every reported model has a V&V row"
        make_root(self.d)
        self.assertTrue(results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name][0])
        # A new output catalogue that no model claims.
        write(self.d / "crates" / "fx" / "src" / "more.rs", 'pub const NEW_OUTPUTS: &[Output] = &[\n    calculated("x", Quantity::Flux),\n];\n')
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name]
        self.assertFalse(passed)
        self.assertIn("NEW_OUTPUTS", reason)
        (self.d / "crates" / "fx" / "src" / "more.rs").unlink()
        # A model dropped from the committed model matrix.
        mm = json.loads((self.d / "validation" / "model_matrix.json").read_text(encoding="utf-8"))
        mm["rows"] = [dict(mm["rows"][0], model="other")]
        write(self.d / "validation" / "model_matrix.json", json.dumps(mm))
        self.assertIn("fixture-model has no row", results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name][1])

    # Verifies: VAL-037
    def test_a_changed_model_source_or_case_makes_the_matrices_stale_and_blocks_the_gate(self):
        name = "V&V matrices are not stale"
        make_root(self.d)
        self.assertTrue(results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name][0])
        src = self.d / "crates" / "fx" / "src" / "lib.rs"
        src.write_text(src.read_text(encoding="utf-8") + "// kernel changed\n", encoding="utf-8")
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name]
        self.assertFalse(passed)
        self.assertIn("STALE model_matrix.json: model fixture-model: source changed (crates/fx/src/lib.rs)", reason)
        vv_fixtures.write_tree(self.d)  # regenerated
        self.assertTrue(results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name][0])
        manifest = json.loads((self.d / "validation" / "cases" / "fixture" / "manifest.json").read_text(encoding="utf-8"))
        manifest["not_covered"] = ["ports", "a new gap"]
        write(self.d / "validation" / "cases" / "fixture" / "manifest.json", json.dumps(manifest))
        self.assertIn("STALE matrix.json", results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))[name][1])

    # Verifies: VAL-006
    def test_a_hand_edited_matrix_cell_or_an_unbacked_validated_claim_blocks_the_gate(self):
        name = "V&V verdicts are derived from case records"
        make_root(self.d)
        suite = good_suite(self.d / "tree")
        out, record = make_report(self.d, suite)
        gate = lambda: results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name]  # noqa: E731
        self.assertTrue(gate()[0])
        edited = json.loads(json.dumps(suite))
        edited["matrix"]["rows"][0]["status"] = "validated within range"
        (out / "report.json").write_text(json.dumps(edited), encoding="utf-8")
        passed, reason = gate()
        self.assertFalse(passed)
        self.assertIn("claims 'validated within range' without a case record", reason)
        self.assertIn("cell status differs from the value derived", reason)

    # Verifies: VAL-091
    def test_the_gate_fails_on_a_report_without_the_model_matrix(self):
        make_root(self.d)
        name = "V&V report exists and its hash is recorded"
        without = {k: v for k, v in good_suite(self.d / "tree").items() if k != "model_matrix"}
        out, record = make_report(self.d, without)
        self.assertIn("lacks model_matrix", results(rg.Gate(self.d, vv_report=out, release_record=record, commit=COMMIT, terms=TERMS))[name][1])

    # Verifies: QA-071
    def test_a_met_requirement_without_a_test_marker_fails_the_gate(self):
        make_root(self.d, now="Met", traced=False)
        passed, reason = results(rg.Gate(self.d, commit=COMMIT, terms=TERMS))["every Met requirement has a test marker"]
        self.assertFalse(passed)
        self.assertIn("1 of 4 Met requirements have a test marker; none for AAA-002, BBB-001, GEO-010", reason)
        make_root(self.d / "ok", now="Met")
        self.assertTrue(results(rg.Gate(self.d / "ok", commit=COMMIT, terms=TERMS))["every Met requirement has a test marker"][0])
        make_root(self.d / "partial", now="Partial", traced=False)  # a Partial row needs no marker for this condition
        self.assertTrue(results(rg.Gate(self.d / "partial", commit=COMMIT, terms=TERMS))["every Met requirement has a test marker"][0])

    def waiver_file(self, **fields):
        waiver = {"condition": "QA-016", "owner": "Connor", "reason": "doc job down", "expiry": str(TODAY + timedelta(days=30)), **fields}
        write(self.d / "docs" / "release-waivers.json", json.dumps({"waivers": [{k: v for k, v in waiver.items() if v is not None}]}))

    # Verifies: QA-072
    def test_a_waiver_needs_owner_reason_and_an_unexpired_date_within_ninety_days(self):
        make_root(self.d)
        name = "gate waivers are complete and unexpired"
        self.assertTrue(results(rg.Gate(self.d, commit=COMMIT, today=TODAY, terms=TERMS))[name][0])  # no file: no waivers
        self.waiver_file()
        self.assertTrue(results(rg.Gate(self.d, commit=COMMIT, today=TODAY, terms=TERMS))[name][0])
        for fields, text in (({"owner": None}, "owner"), ({"reason": " "}, "reason"), ({"expiry": str(TODAY - timedelta(days=1))}, "expired"),
                             ({"expiry": str(TODAY + timedelta(days=91))}, "90 days"), ({"condition": "no such condition"}, "not a condition"),
                             ({"condition": "QA-083"}, "cannot be waived")):
            self.waiver_file(**fields)
            passed, reason = results(rg.Gate(self.d, commit=COMMIT, today=TODAY, terms=TERMS))[name]
            self.assertFalse(passed, fields)
            self.assertIn(text, reason)

    # Verifies: QA-072
    def test_a_valid_waiver_turns_a_failing_condition_into_waived_and_an_invalid_one_does_not(self):
        make_root(self.d)
        inputs = passing_inputs(self.d)
        write(inputs["doc_tests"], json.dumps({"commit": COMMIT, "checks": [{"name": "x", "verdict": "FAIL"}]}))
        self.assertFalse(results(rg.Gate(self.d, **inputs))["documentation tests pass"][0])
        self.waiver_file()
        res = rg.run_gate(rg.Gate(self.d, **inputs))
        waived = next(r for r in res if r[1] == "documentation tests pass")
        self.assertTrue(waived[0])
        self.assertIn("WAIVED by Connor until", waived[3])
        self.assertIn("WAIVED  documentation tests pass", rg.render(res))
        self.assertIn("release gate: PASS", rg.render(res))
        self.waiver_file(expiry=str(TODAY - timedelta(days=1)))  # expired: the condition fails again, and so does the waiver check
        res = results(rg.Gate(self.d, **inputs))
        self.assertFalse(res["documentation tests pass"][0])
        self.assertFalse(res["gate waivers are complete and unexpired"][0])

    # Verifies: PLAT-073
    def test_repository_checks_run_every_script_fail_on_a_failing_one_and_on_a_missing_input(self):
        make_root(self.d)
        for name, _, _ in rg.REPO_CHECKS:
            self.assertTrue(name)
        scripts = self.d / "scripts"
        for script in ("requirement_scorecard.py", "requirement_trace.py", "requirement_lint.py", "source_lint.py",
                       "check_repository_size.py", "check_audit.py", "check_links.py", "check_glossary.py", "check_changelog.py",
                       "check_deprecations.py", "known_issues.py", "check_citation.py"):
            write(scripts / script, "print('ok: fixture')\n")
        write(scripts / "check_changelog.py", "import sys\nprint('changelog: broken heading')\nsys.exit(1)\n")
        gate = rg.Gate(self.d, commit=COMMIT, terms=TERMS, repo_checks=True)
        # cargo metadata and the faris binary are not available in the fixture tree: those checks must FAIL, not pass
        gate.cargo = "definitely-not-a-cargo"
        res = results(gate)
        self.assertTrue(res["requirement scorecard is current"][0])
        self.assertTrue(res["glossary lint"][0])
        self.assertFalse(res["changelog layout"][0])
        self.assertIn("changelog: broken heading", res["changelog layout"][1])
        for name in ("SBOM generates and validates", "dependency licences fit AGPL-3.0", "dependency notices are current"):
            self.assertFalse(res[name][0], name)
        passed, reason = res["command-line reference is current"]
        self.assertFalse(passed)
        self.assertIn("--faris", reason)
        self.assertEqual(len(res), len(rg.CONDITIONS) + len(rg.REPO_CHECKS))

    # Verifies: PLAT-073
    def test_the_command_exits_nonzero_when_a_repository_check_fails_and_can_skip_them(self):
        make_root(self.d)
        argv = ["--root", str(self.d), "--commit", COMMIT]
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(rg.main(argv), 1)
        self.assertIn("FAIL  requirement scorecard is current", out.getvalue())
        with contextlib.redirect_stdout(io.StringIO()) as out:
            rg.main(argv + ["--no-repo-checks"])
        self.assertNotIn("scorecard", out.getvalue())

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
