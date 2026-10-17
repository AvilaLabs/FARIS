# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests of the process checks: release scorecard, merge audit, S1 disclosure, release known issues, external data."""
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_external_data as ext  # noqa: E402
import check_signoff as signoff  # noqa: E402
import defect_log as dl  # noqa: E402
import known_issues  # noqa: E402
import make_release  # noqa: E402
import merge_audit as audit  # noqa: E402
import release_gate  # noqa: E402
import release_scorecard as card  # noqa: E402

REPO = Path(__file__).resolve().parent.parent


def run_git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), "-c", "user.name=T", "-c", "user.email=t@example.org",
                           "-c", "commit.gpgsign=false", *args], check=True, capture_output=True, text=True).stdout.strip()


def commit(root: Path, files: dict[str, str], message: str) -> str:
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text, encoding="utf-8")
    run_git(root, "add", "-A")
    run_git(root, "commit", "-q", "-m", message)
    return run_git(root, "rev-parse", "HEAD")


def defect(**changes):
    d = {"id": "FARIS-001", "title": "t", "severity": "S3", "status": "fixed", "found": "2026-10-01",
         "affected_versions": ["unreleased"], "fixed_in": "abc1234", "disposition": "Wrong; fixed.",
         "regression_test": "tests/t.py::test_it"}
    d.update(changes)
    return d


class ReleaseScorecardTests(unittest.TestCase):
    # Verifies: QA-007
    def test_the_card_has_at_most_fifteen_measures_with_no_empty_field_and_each_is_a_gate_condition(self):
        self.assertLessEqual(len(card.MEASURES), 15)
        self.assertEqual(card.problems(), [])
        for m in card.MEASURES:
            for field in ("name", "formula", "threshold", "source"):
                self.assertTrue(m[field].strip(), (m["name"], field))

    # Verifies: QA-007
    def test_the_verdict_column_comes_from_the_gate_output_and_reads_not_run_without_it(self):
        self.assertNotIn("| PASS", card.render())
        self.assertEqual(card.render().count("not run"), len(card.MEASURES))
        gate = ("PASS  all tests pass (QA-070): ok\nFAIL  fuzz clean (QA-033): 2 targets\n"
                "WAIVED  documentation tests pass (QA-016): WAIVED by x until 2026-11-01 (r); the condition fails: y\n")
        self.assertEqual(card.verdicts(gate), {"QA-070": "PASS", "QA-033": "FAIL", "QA-016": "WAIVED"})
        rows = {line.split(" | ")[0].lstrip("| "): line.split(" | ")[-1] for line in card.render(gate).splitlines() if line.startswith("| ")}
        self.assertEqual(rows["Test suite"], "PASS |")
        self.assertEqual(rows["Fuzzing"], "FAIL |")
        self.assertEqual(rows["Documentation tests"], "WAIVED |")

    # Verifies: QA-007
    def test_a_sixteenth_measure_an_empty_field_or_an_unknown_condition_fails(self):
        saved = card.MEASURES
        try:
            card.MEASURES = saved + ({**saved[0], "ident": "QA-070"},) * 1
            self.assertTrue(any("at most 15" in p for p in card.problems()))
            card.MEASURES = (*saved[:-1], {**saved[-1], "threshold": " "})
            self.assertTrue(any("empty threshold" in p for p in card.problems()))
            card.MEASURES = (*saved[:-1], {**saved[-1], "ident": "NOPE-001"})
            self.assertTrue(any("not a condition of the release gate" in p for p in card.problems()))
        finally:
            card.MEASURES = saved


class MergeAuditTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        run_git(self.root, "init", "-q")

    def tearDown(self):
        self._tmp.cleanup()

    # Verifies: QA-085
    def test_a_fix_commit_that_has_its_test_passes_and_one_that_lacks_it_fails(self):
        fix = commit(self.root, {"tests/t.py": "def test_it():\n    pass\n", "src.py": "fixed\n"}, "fix with its test")
        self.assertEqual(audit.audit([defect(fixed_in=fix)], self.root), [])
        bare = commit(self.root, {"src.py": "fixed again\n"}, "fix without a test")
        late = commit(self.root, {"tests/late.py": "def test_late():\n    pass\n"}, "test arrives afterwards")
        problems = audit.audit([defect(fixed_in=bare, regression_test="tests/late.py::test_late")], self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("does not exist at fix commit", problems[0])
        problems = audit.audit([defect(fixed_in=bare, regression_test="tests/t.py::test_other")], self.root)
        self.assertIn("is not in tests/t.py at fix commit", problems[0])
        # A late test is accepted only when the log says so and names the commit that added it.
        self.assertEqual(audit.audit([defect(fixed_in=bare, regression_test="tests/late.py::test_late", retrospective_test=late)], self.root), [])

    # Verifies: QA-085
    def test_a_fix_with_no_commit_or_an_unknown_commit_fails_and_s4_and_open_defects_are_not_audited(self):
        commit(self.root, {"tests/t.py": "def test_it():\n    pass\n"}, "first")
        self.assertIn("not a commit", audit.audit([defect(fixed_in="unreleased")], self.root)[0])
        self.assertIn("no fixed_in", audit.audit([defect(fixed_in="")], self.root)[0])
        self.assertEqual(audit.audit([defect(severity="S4", fixed_in="unreleased"), defect(status="open", fixed_in=None)], self.root), [])
        self.assertIn("not a commit", audit.audit([defect(fixed_in="HEAD", retrospective_test="deadbeef")], self.root)[0])


class DisclosureTests(unittest.TestCase):
    # Verifies: QA-082
    def test_an_s1_that_reached_a_release_is_disclosed_within_seven_days_of_being_found(self):
        today = date(2026, 11, 1)
        released = dict(severity="S1", affected_versions=["0.2.0"], found="2026-10-10")
        self.assertTrue(any("no disclosed date" in p for p in dl.disclosure_problems(defect(**released), today)))
        self.assertEqual(dl.disclosure_problems(defect(**released), date(2026, 10, 15)), [])  # still inside the 7 days
        self.assertEqual(dl.disclosure_problems(defect(**released, disclosed="2026-10-17"), today), [])
        self.assertTrue(any("more than 7 days" in p for p in dl.disclosure_problems(defect(**released, disclosed="2026-10-18"), today)))
        self.assertTrue(any("before it was found" in p for p in dl.disclosure_problems(defect(**released, disclosed="2026-10-09"), today)))
        self.assertTrue(any("not a YYYY-MM-DD" in p for p in dl.disclosure_problems(defect(**released, disclosed="soon"), today)))

    # Verifies: QA-082
    def test_disclosure_is_not_asked_for_unreleased_defects_or_lower_severities_and_runs_in_the_lint(self):
        today = date(2026, 12, 1)
        self.assertEqual(dl.disclosure_problems(defect(severity="S1"), today), [])
        self.assertEqual(dl.disclosure_problems(defect(severity="S2", affected_versions=["0.2.0"]), today), [])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tests").mkdir()
            (root / "tests" / "t.py").write_text("def test_it(): pass\n", encoding="utf-8")
            late = defect(severity="S1", affected_versions=["0.2.0"], found="2026-10-10")
            self.assertTrue(any("no disclosed date" in p for p in dl.lint({"defects": [late]}, root, today)))
            self.assertEqual(dl.lint({"defects": [{**late, "disclosed": "2026-10-12"}]}, root, today), [])

    # Verifies: QA-082
    def test_the_real_log_meets_the_disclosure_rule(self):
        data = json.loads((REPO / dl.LOG).read_text(encoding="utf-8"))
        for d in data["defects"]:
            self.assertEqual(dl.disclosure_problems(d, date.today()), [], d["id"])


class ReleaseKnownIssuesTests(unittest.TestCase):
    def tree(self, root: Path, page=None):
        (root / "docs" / "defects").mkdir(parents=True)
        log = {"defects": [defect(regression_test="x")]}
        (root / known_issues.LOG).write_text(json.dumps(log), encoding="utf-8")
        (root / known_issues.PAGE).write_text(known_issues.render(log["defects"]) if page is None else page, encoding="utf-8")

    # Verifies: QA-082
    def test_the_release_carries_the_current_known_issues_page_with_its_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root, out = Path(directory) / "tree", Path(directory) / "out"
            out.mkdir()
            self.tree(root)
            name, digest = make_release.write_known_issues(out, "1.2.3", root)
            self.assertEqual(name, "FARIS-1.2.3-KNOWN_ISSUES.md")
            self.assertEqual(digest, make_release.sha256(out / name))
            self.assertEqual((out / name).read_text(encoding="utf-8"), (root / known_issues.PAGE).read_text(encoding="utf-8"))

    # Verifies: QA-082
    def test_a_stale_known_issues_page_stops_the_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root, out = Path(directory) / "tree", Path(directory) / "out"
            out.mkdir()
            self.tree(root, page="old page\n")
            with self.assertRaisesRegex(SystemExit, "out of date"):
                make_release.write_known_issues(out, "1.2.3", root)

    # Verifies: QA-082
    def test_the_release_notes_list_the_known_issues_file(self):
        text = make_release.downloads_section("1.2.3", [], None, "FARIS-1.2.3-KNOWN_ISSUES.md")
        self.assertIn("`FARIS-1.2.3-KNOWN_ISSUES.md`", text)
        self.assertNotIn("KNOWN_ISSUES", make_release.downloads_section("1.2.3", [], None))


class ExternalDataTests(unittest.TestCase):
    ENTRY = {"name": "D", "url": "https://example.org/d.xz", "md5": "a" * 32, "verified_in": ["check.py"]}

    def tree(self, root: Path, manifest: dict, fetching: str = "curl -fL https://example.org/d.xz -o d.xz\n", verifier: str = "a" * 32):
        (root / "scripts").mkdir(parents=True)
        (root / "check.py").write_text(f"EXPECTED = '{verifier}'\n", encoding="utf-8")
        (root / "scripts" / "fetch.sh").write_text(fetching, encoding="utf-8")
        (root / ext.MANIFEST).write_text(json.dumps(manifest), encoding="utf-8")

    # Verifies: QA-090
    def test_a_listed_dataset_with_a_hash_its_verifier_contains_and_a_fetch_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            self.tree(Path(directory), {"data": [self.ENTRY]})
            self.assertEqual(ext.problems(Path(directory)), [])

    # Verifies: QA-090
    def test_a_fetched_url_that_is_not_in_the_manifest_or_has_no_hash_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": []})
            self.assertTrue(any("not in scripts/external_data.json" in p for p in ext.problems(root)))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": [{k: v for k, v in self.ENTRY.items() if k != "md5"}]})
            self.assertTrue(any("no sha256, md5 or file_sha256" in p for p in ext.problems(root)))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": [{**self.ENTRY, "md5": "xyz"}]})
            self.assertTrue(any("not a md5 digest" in p for p in ext.problems(root)))

    # Verifies: QA-090
    def test_a_hash_the_verifying_file_lacks_a_stale_entry_and_a_listed_installer_are_handled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": [self.ENTRY]}, verifier="b" * 32)
            self.assertTrue(any("does not contain the md5" in p for p in ext.problems(root)))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": [self.ENTRY]}, fetching="echo nothing\n")
            self.assertTrue(any("no workflow, script or document fetches it" in p for p in ext.problems(root)))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, {"data": [self.ENTRY], "tools": [{"url": "https://sh.example.org"}]},
                      fetching="curl -fL https://example.org/d.xz -o d.xz\ncurl -fsSL https://sh.example.org | sh\n")
            self.assertEqual(ext.problems(root), [])

    # Verifies: QA-090
    def test_every_url_the_repository_fetches_is_in_the_manifest_with_a_hash(self):
        self.assertEqual(ext.problems(REPO), [])


class SignOffTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        run_git(self.root, "init", "-q")
        commit(self.root, {"a": "1"}, "base")

    def tearDown(self):
        self._tmp.cleanup()

    def make(self, message: str, email: str = "t@example.org", day: str = "2026-10-20") -> str:
        (self.root / "a").write_text(message, encoding="utf-8")
        run_git(self.root, "add", "-A")
        run_git(self.root, "-c", f"user.email={email}", "commit", "-q", "-m", message, "--date", f"{day}T12:00:00", "--author", f"A <{email}>")
        return run_git(self.root, "rev-parse", "HEAD")

    # Verifies: LEG-005
    def test_a_commit_signed_off_with_the_authors_address_passes_and_an_unsigned_or_misaddressed_one_fails(self):
        base = run_git(self.root, "rev-parse", "HEAD")
        self.make("signed\n\nSigned-off-by: A <t@example.org>")
        self.make("unsigned")
        self.make("someone else\n\nSigned-off-by: B <b@example.org>")
        found = signoff.commits(self.root, f"{base}..HEAD")
        self.assertEqual(len(found), 3)
        self.assertEqual([signoff.signed(c) for c in found], [False, False, True])  # newest first
        problems = signoff.problems(found)
        self.assertEqual(len(problems), 2)
        self.assertTrue(all("no Signed-off-by" in p for p in problems))
        self.assertEqual(signoff.main([f"{base}..HEAD", "--root", str(self.root)]), 1)

    # Verifies: LEG-005
    def test_maintainers_commits_are_checked_too_and_a_fully_signed_range_passes(self):
        base = run_git(self.root, "rev-parse", "HEAD")
        self.make("by a maintainer\n\nSigned-off-by: M <maintainer@example.org>", email="maintainer@example.org")
        self.assertEqual(signoff.main([f"{base}..HEAD", "--root", str(self.root)]), 0)
        self.make("maintainer, no sign-off", email="maintainer@example.org")
        self.assertEqual(signoff.main([f"{base}..HEAD", "--root", str(self.root)]), 1)
        self.assertEqual(signoff.main(["HEAD", "--root", str(self.root)]), 1)  # a single commit is checked alone

    # Verifies: LEG-005
    def test_commits_authored_before_the_policy_date_are_not_checked(self):
        old = self.make("old, unsigned", day="2026-10-01")
        self.assertEqual(signoff.problems(signoff.commits(self.root, old)), [])
        self.assertEqual(len(signoff.problems(signoff.commits(self.root, old), since="2026-09-01")), 1)


class GateTests(unittest.TestCase):
    # Verifies: QA-072
    def test_the_waiver_rules_are_in_the_gate_documentation(self):
        self.assertIn("FARIS-NNN", release_gate.__doc__)


if __name__ == "__main__":
    unittest.main()
