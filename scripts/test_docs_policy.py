# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for the documentation and release-policy checks: glossary, citation, changelog, deprecations,
known issues, release notes and the generated command-line reference (DOC-021, DOC-034, DOC-040, DOC-043, DOC-060,
PRV-036, QA-074, QA-082, PLAT-034, LEG-052, SEC-073, SEC-076)."""
import json
import os
import re
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_changelog  # noqa: E402
import check_citation  # noqa: E402
import check_deprecations  # noqa: E402
import check_glossary  # noqa: E402
import cli_reference  # noqa: E402
import known_issues  # noqa: E402
import release_notes  # noqa: E402

REPO = Path(__file__).resolve().parent.parent


def tmp() -> Path:
    return Path(tempfile.mkdtemp())


def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


class GlossaryTests(unittest.TestCase):
    def test_the_repository_glossary_passes_and_defines_every_verdict_word(self):
        # Verifies: DOC-034
        self.assertEqual(check_glossary.check(REPO), [])
        terms = {e["term"] for e in check_glossary.parse((REPO / "docs" / "glossary.md").read_text(encoding="utf-8"))}
        self.assertTrue({"PASS", "FAIL", "INCONCLUSIVE", "NOT_EVALUATED"} <= terms)

    def test_a_long_definition_a_duplicate_and_a_missing_link_fail(self):
        # Verifies: DOC-034
        root = tmp()
        write(root / "scripts" / "wording_lint_terms.json", json.dumps({"verdict_words": {"canonical_forms": []}}))
        write(root / "docs" / "guide" / "results.md", "# Results\n\n| Badge | Meaning |\n| --- | --- |\n| calculated | x |\n")
        long = " ".join(["word"] * 41)
        write(root / "docs" / "glossary.md",
              f"# G\n\n- **PASS.** short. [r](guide/results.md)\n- **pass.** again. [r](guide/results.md)\n"
              f"- **FAIL.** {long} [r](guide/results.md)\n- **INCONCLUSIVE.** no link\n- **NOT_EVALUATED.** bad [r](guide/gone.md)\n")
        text = "\n".join(check_glossary.check(root))
        self.assertIn("already defined", text)
        self.assertIn("41 words", text)
        self.assertIn("no link", text)
        self.assertIn("does not exist", text)
        self.assertIn("does not define it", text)  # 'calculated' from the results table is missing


class CitationTests(unittest.TestCase):
    def test_the_repository_citation_file_is_valid_and_matches_the_cite_page(self):
        # Verifies: PRV-036, DOC-060
        cff = check_citation.read_yaml((REPO / "CITATION.cff").read_text(encoding="utf-8"))
        self.assertEqual(cff["cff-version"], "1.2.0")
        self.assertEqual(cff["authors"][0], {"family-names": "Avila", "given-names": "Connor", "affiliation": "Avila Labs"})
        page = (REPO / check_citation.PAGE).read_text(encoding="utf-8")
        self.assertEqual(check_citation.problems_in(cff, check_citation.workspace_version(REPO), page), [])

    def test_invalid_files_are_rejected_with_the_reason(self):
        # Verifies: PRV-036
        good = {"cff-version": "1.2.0", "message": "m", "title": "T", "version": "1.0.0", "date-released": "2026-01-02",
                "license": "AGPL-3.0-only", "authors": [{"family-names": "A", "given-names": "B"}]}
        self.assertEqual(check_citation.problems_in(good, "1.0.0", None), [])
        cases = [
            ({"cff-version": "1.1.0"}, "cff-version must be 1.2.0"),
            ({"authors": [{"given-names": "B"}]}, "needs name"),
            ({"version": "one"}, "not a semantic version"),
            ({"date-released": "2026/01/02"}, "YYYY-MM-DD"),
            ({"doi": "doi:abc"}, "doi has the wrong shape"),
            ({"license": "not a licence"}, "SPDX"),
            ({"repository-code": "git@x"}, "http(s) URL"),
        ]
        for change, text in cases:
            problems = check_citation.problems_in({**good, **change}, "1.0.0", None)
            self.assertTrue(any(text in p for p in problems), (change, problems))
        self.assertTrue(any("differs from the workspace version" in p for p in check_citation.problems_in(good, "1.1.0", None)))
        self.assertTrue(any("does not show" in p for p in check_citation.problems_in(good, "1.0.0", "nothing here")))

    def test_the_reader_refuses_what_it_cannot_parse(self):
        # Verifies: PRV-036
        with self.assertRaises(ValueError):
            check_citation.read_yaml("title: a\ntitle: b\n")
        with self.assertRaises(ValueError):
            check_citation.read_yaml("this is not yaml\n")


class ChangelogTests(unittest.TestCase):
    def test_the_repository_changelog_passes(self):
        # Verifies: DOC-040
        version = check_citation.workspace_version(REPO)
        self.assertEqual(check_changelog.problems_in((REPO / "CHANGELOG.md").read_text(encoding="utf-8"), version), [])

    def test_layout_rules(self):
        # Verifies: DOC-040
        ok = "# C\n\n## Unreleased\n\n### Added\n\n- x\n\n## 0.2.0 — 2026-10-08\n\n### Added\n\n- y\n"
        self.assertEqual(check_changelog.problems_in(ok, "0.2.0"), [])
        dup = ok.replace("- x\n", "- x\n\n### Added\n\n- again\n")
        self.assertTrue(any("more than one '### Added'" in p for p in check_changelog.problems_in(dup, "0.2.0")))
        late = ok.replace("### Added\n\n- x", "### Added\n\n- x\n\n### Numerical changes\n\n- z")
        self.assertTrue(any("must come first" in p for p in check_changelog.problems_in(late, "0.2.0")))
        self.assertTrue(any("not one of" in p for p in check_changelog.problems_in(ok.replace("### Added\n\n- x", "### Misc\n\n- x"), "0.2.0")))
        self.assertTrue(any("is empty" in p for p in check_changelog.problems_in(ok.replace("- x\n", ""), "0.2.0")))
        order = "# C\n\n## 0.1.0 — 2026-01-01\n\n### Added\n\n- a\n\n## 0.2.0 — 2026-02-01\n\n### Added\n\n- b\n"
        self.assertTrue(any("not older" in p for p in check_changelog.problems_in(order, "0.2.0")))
        self.assertTrue(any("no section for the workspace version" in p for p in check_changelog.problems_in(
            "# C\n\n## 0.1.0 — 2026-01-01\n\n### Added\n\n- a\n", "0.3.0")))


class DeprecationTests(unittest.TestCase):
    TABLE = "<!-- deprecations:begin -->\n| Item | Replacement | Announced in | Removal in |\n| --- | --- | --- | --- |\n{rows}<!-- deprecations:end -->\n"
    CHANGELOG = "## 0.3.0 — 2026-11-01\n\n### Deprecated\n\n- x\n"

    def problems(self, rows, current="0.3.0"):
        return check_deprecations.problems_in(check_deprecations.register(self.TABLE.format(rows=rows)), current, self.CHANGELOG)

    def test_the_repository_register_passes(self):
        # Verifies: DOC-043
        text = (REPO / check_deprecations.POLICY).read_text(encoding="utf-8")
        version = check_citation.workspace_version(REPO)
        changelog = (REPO / "CHANGELOG.md").read_text(encoding="utf-8")
        self.assertEqual(check_deprecations.problems_in(check_deprecations.register(text), version, changelog), [])

    def test_a_valid_entry_passes_and_the_two_minor_release_rule_is_enforced(self):
        # Verifies: DOC-043
        self.assertEqual(self.problems("| `--old` | `--new` | 0.3.0 | 0.5.0 |\n"), [])
        self.assertTrue(any("less than 2 minor releases" in p for p in self.problems("| `--old` | `--new` | 0.3.0 | 0.4.0 |\n")))

    def test_missing_replacement_reached_removal_and_unknown_announcement_fail(self):
        # Verifies: DOC-043
        self.assertTrue(any("no replacement" in p for p in self.problems("| `--old` | none | 0.3.0 | 0.5.0 |\n")))
        self.assertTrue(any("has been reached" in p for p in self.problems("| `--old` | `--new` | 0.3.0 | 0.5.0 |\n", current="0.5.0")))
        self.assertTrue(any("no section in CHANGELOG" in p for p in self.problems("| `--old` | `--new` | 0.2.5 | 0.5.0 |\n")))


class KnownIssuesTests(unittest.TestCase):
    DEFECTS = [
        {"id": "FARIS-001", "title": "Open | bug", "severity": "S2", "status": "open", "found": "2026-10-01",
         "affected_versions": ["0.2.0"], "disposition": "Wrong value, with a warning."},
        {"id": "FARIS-002", "title": "Old", "severity": "S3", "status": "fixed", "found": "2026-09-01",
         "affected_versions": ["0.1.0"], "fixed_in": "abc123", "disposition": "Fixed."},
        {"id": "FARIS-003", "title": "Dev only", "severity": "S4", "status": "fixed", "found": "2026-09-02",
         "affected_versions": ["unreleased"], "fixed_in": "def456", "disposition": "Fixed."},
    ]

    def test_the_page_lists_open_and_released_defects_and_escapes_table_pipes(self):
        # Verifies: QA-082
        text = known_issues.render(self.DEFECTS)
        self.assertIn("1 open, 0 will not be fixed, 1 fixed that affected a released version, 1 fixed before", text)
        open_section = text.split("## Open")[1].split("## Will not")[0]
        self.assertIn("FARIS-001", open_section)
        self.assertIn("Open / bug", open_section)
        self.assertNotIn("FARIS-003", open_section)

    def test_the_committed_page_equals_a_fresh_generation(self):
        # Verifies: QA-082
        self.assertEqual(known_issues.main(["--check", "--root", str(REPO)]), 0)
        root = tmp()
        write(root / known_issues.LOG, json.dumps({"defects": self.DEFECTS}))
        self.assertEqual(known_issues.main(["--check", "--root", str(root)]), 1)  # no page yet
        self.assertEqual(known_issues.main(["--root", str(root)]), 0)
        self.assertEqual(known_issues.main(["--check", "--root", str(root)]), 0)


class ReleaseNotesTests(unittest.TestCase):
    CHANGELOG = ("# C\n\n## Unreleased\n\n### Added\n\n- a\n\n## 0.3.0 — 2026-11-01\n\n### Numerical changes\n\n"
                 "- The tritium tally is normalised per source neutron; values move by 0.4 % on the demo.\n\n"
                 "### Security\n\n- RUSTSEC-2026-0001 fixed.\n\n### Changed\n\n- Moved; see [guide](docs/migrations/0.2-to-0.3.md).\n\n"
                 "## 0.2.0 — 2026-10-08\n\n### Added\n\n- old\n")
    GATE = "PASS  wording lint clean (wording lint): ok\nFAIL  fuzz clean (QA-033): none\nrelease gate: FAIL, 1 of 2 conditions pass\n"
    RECORD = {"vv_report_sha256": "a" * 64}
    COUNTS = {"open": 1, "fixed": 9, "wont-fix": 0}

    def test_notes_carry_numeric_changes_security_vv_status_gate_result_and_export_line(self):
        # Verifies: QA-074, PLAT-034, LEG-052
        text = release_notes.render("0.3.0", self.CHANGELOG, self.RECORD, self.GATE, self.COUNTS)
        self.assertLess(text.index("## Changes that can alter a published number"), text.index("## Security fixes"))
        self.assertIn("0.4 % on the demo", text)
        self.assertIn("RUSTSEC-2026-0001", text)
        self.assertIn("a" * 64, text)
        self.assertIn("- FAIL  fuzz clean (QA-033): none", text)
        self.assertIn("release gate: FAIL, 1 of 2 conditions pass", text)
        self.assertIn("1 open and 0 unfixed", text)
        self.assertIn("docs/migrations/0.2-to-0.3.md", text)
        self.assertIn(release_notes.EXPORT_LINE, text)
        self.assertIn("NOT REVIEWED", text)

    def test_a_release_with_no_listed_numeric_change_says_that_is_not_a_measurement(self):
        # Verifies: PLAT-034
        text = release_notes.render("0.2.0", self.CHANGELOG, self.RECORD, self.GATE, self.COUNTS)
        self.assertIn("not a measurement", text)
        self.assertIn("No security fix in this release.", text)

    def test_missing_inputs_refuse_to_produce_notes(self):
        # Verifies: QA-074
        for version, record, gate in (("9.9.9", self.RECORD, self.GATE), ("0.3.0", {}, self.GATE),
                                      ("0.3.0", {"vv_report_sha256": "short"}, self.GATE), ("0.3.0", self.RECORD, "")):
            with self.assertRaises(ValueError):
                release_notes.render(version, self.CHANGELOG, record, gate, self.COUNTS)
        root = tmp()
        out = root / "notes.md"
        code = release_notes.main(["--version", "0.3.0", "--release-record", str(root / "none.json"), "--gate", str(root / "none.txt"),
                                   "--root", str(REPO), "--out", str(out)])
        self.assertEqual(code, 1)
        self.assertFalse(out.exists())

    def test_the_main_command_writes_the_file_for_the_repository_unreleased_section_inputs(self):
        # Verifies: QA-074
        root = tmp()
        write(root / "record.json", json.dumps(self.RECORD))
        write(root / "gate.txt", self.GATE)
        version = check_citation.workspace_version(REPO)
        changelog = (REPO / "CHANGELOG.md").read_text(encoding="utf-8")
        self.assertIsNotNone(re.search(rf"^##\s+{re.escape(version)}\s", changelog, re.M))
        out = root / "notes.md"
        self.assertEqual(release_notes.main(["--version", version, "--release-record", str(root / "record.json"),
                                             "--gate", str(root / "gate.txt"), "--root", str(REPO), "--out", str(out)]), 0)
        self.assertIn("Export-control review", out.read_text(encoding="utf-8"))


STUB = """#!/usr/bin/env python3
import sys
args = sys.argv[1:-1]
pages = {
    (): "Tool\\n\\nUsage: faris [OPTIONS] <COMMAND>\\n\\nCommands:\\n  run    Run it\\n  sub    A group\\n  help   Print help\\n\\nOptions:\\n  -h, --help\\n",
    ("run",): "Run it\\n\\nUsage: faris run [OPTIONS]\\n\\nOptions:\\n      --fast  Go fast\\n  -h, --help\\n",
    ("sub",): "A group\\n\\nUsage: faris sub <COMMAND>\\n\\nCommands:\\n  leaf   A leaf\\n  help   Print help\\n",
    ("sub", "leaf"): "A leaf\\n\\nUsage: faris sub leaf\\n",
}
if tuple(args) not in pages or sys.argv[-1] != "--help":
    sys.exit(2)
print(pages[tuple(args)], end="")
"""


class CliReferenceTests(unittest.TestCase):
    def stub(self) -> Path:
        path = tmp() / "faris"
        write(path, STUB)
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def test_the_walk_finds_every_subcommand_at_every_depth_and_skips_help(self):
        # Verifies: DOC-021
        walked = cli_reference.walk(self.stub())
        self.assertEqual([w for w, _ in walked], [(), ("run",), ("sub",), ("sub", "leaf")])

    def test_examples_come_from_the_handbook_and_missing_ones_are_listed(self):
        # Verifies: DOC-021
        root = tmp()
        write(root / "README.md", "Try `faris run --fast` now.\n")
        write(root / "docs" / "guide" / "a.md", "```sh\nfaris run \\\n  --fast\n```\n")
        lines = cli_reference.command_lines(root)
        self.assertEqual(lines, ["faris run --fast", "faris run --fast"])  # a code span, then a joined continuation line
        text = cli_reference.render(cli_reference.walk(self.stub()), lines)
        self.assertIn("Example: `faris run --fast`", text)
        self.assertIn("1 of 3 commands have an example in the handbook; these have none yet: `faris sub`, `faris sub leaf`.", text)
        self.assertIn("Example: none in the handbook yet.", text)
        self.assertIn("--fast  Go fast", text)

    def test_check_passes_when_current_and_fails_when_a_flag_changes(self):
        # Verifies: DOC-021
        root = tmp()
        binary = self.stub()
        self.assertEqual(cli_reference.main(["--faris", str(binary), "--root", str(root), "--check"]), 1)  # no page yet
        (root / "docs" / "guide").mkdir(parents=True)
        self.assertEqual(cli_reference.main(["--faris", str(binary), "--root", str(root)]), 0)
        self.assertEqual(cli_reference.main(["--faris", str(binary), "--root", str(root), "--check"]), 0)
        write(binary, STUB.replace("Go fast", "Go faster"))
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
        self.assertEqual(cli_reference.main(["--faris", str(binary), "--root", str(root), "--check"]), 1)

    def test_a_binary_that_fails_is_an_error_not_an_empty_page(self):
        # Verifies: DOC-021
        path = tmp() / "faris"
        write(path, "#!/bin/sh\nexit 3\n")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        self.assertEqual(cli_reference.main(["--faris", str(path), "--root", str(tmp()), "--check"]), 1)


class ContributionPolicyTests(unittest.TestCase):
    def test_contributing_states_the_regression_test_rule_and_the_template_has_the_checkbox(self):
        # Verifies: QA-085
        text = (REPO / "CONTRIBUTING.md").read_text(encoding="utf-8")
        self.assertIn("## Every fix comes with a regression test", text)
        self.assertIn("fails without the fix and passes with it", text)
        template = (REPO / ".github" / "PULL_REQUEST_TEMPLATE.md").read_text(encoding="utf-8")
        self.assertRegex(template, r"- \[ \] If this fixes a defect: I added a regression test")

    def test_the_defect_log_lint_already_rejects_a_fixed_defect_without_an_existing_test(self):
        # Verifies: QA-085
        import defect_log
        entry = {"id": "FARIS-900", "title": "t", "severity": "S2", "status": "fixed", "found": "2026-10-01",
                 "affected_versions": ["unreleased"], "fixed_in": "abc", "disposition": "d", "regression_test": None}
        problems = defect_log.lint({"defects": [entry]}, tmp())
        self.assertTrue(problems)

    def test_release_documents_exist_and_the_migration_template_has_before_and_after(self):
        # Verifies: DOC-041
        text = (REPO / "docs" / "MIGRATION_GUIDE_TEMPLATE.md").read_text(encoding="utf-8")
        for needle in ("## Before", "## After", "## Does it change a number", "## Your existing files"):
            self.assertIn(needle, text)
        self.assertIn("before and an after example", text)


class SecurityDocumentTests(unittest.TestCase):
    def test_security_policy_has_a_private_channel_and_the_stated_targets(self):
        # Verifies: SEC-073
        text = (REPO / "SECURITY.md").read_text(encoding="utf-8")
        self.assertIn("https://github.com/AvilaLabs/FARIS/security/advisories/new", text)
        for target in ("3 business days", "7 days", "30 days", "90 days"):
            self.assertIn(target, text)

    def test_the_advisory_process_covers_cve_release_notes_and_a_regression_test(self):
        # Verifies: SEC-074
        text = (REPO / "docs" / "SECURITY_PROCESS.md").read_text(encoding="utf-8")
        for needle in ("CVE", "release notes", "regression test", "Advisory log"):
            self.assertIn(needle, text)

    def test_every_threat_maps_to_an_existing_requirement_or_a_listed_accepted_risk(self):
        # Verifies: SEC-076
        text = (REPO / "docs" / "THREAT_MODEL.md").read_text(encoding="utf-8")
        known = set()
        for path in (REPO / "docs" / "requirements").glob("[0-9][0-9]-*.md"):
            known.update(re.findall(r"^\| ([A-Z]+-\d+) \|", path.read_text(encoding="utf-8"), re.M))
        accepted = set(re.findall(r"^- \*\*(AR-\d+)\.\*\*", text, re.M))
        rows = re.findall(r"^\| ([A-Z]\d+) \| (\w[\w ]*) \| .*? \| (.*?) \|$", text, re.M)
        self.assertGreaterEqual(len(rows), 30)
        stride = {"Spoofing", "Tampering", "Repudiation", "Information disclosure", "Denial of service", "Elevation of privilege"}
        for ident, category, closed_by in rows:
            self.assertIn(category, stride, ident)
            ids = re.findall(r"[A-Z]+-\d+", closed_by)
            risks = re.findall(r"AR-\d+", closed_by)
            self.assertTrue(ids or risks or "AGENTS.md" in closed_by, ident)
            self.assertTrue(set(ids) - {r for r in risks} <= known, (ident, set(ids) - known))
            self.assertTrue(set(risks) <= accepted, (ident, risks))
        areas = ("Files", "Adapters", "Sign-in", "Updates and distribution", "Data")
        headings = re.findall(r"^## (.+)$", text, re.M)
        for area in areas:
            self.assertTrue(any(h.startswith(area) for h in headings), area)

    def test_the_export_control_statement_is_marked_as_a_draft_and_claims_no_review(self):
        # Verifies: LEG-050, LEG-052
        text = (REPO / "docs" / "EXPORT_CONTROL.md").read_text(encoding="utf-8")
        self.assertIn("DRAFT for owner and counsel review", text)
        self.assertIn("No lawyer or export-control specialist has reviewed this text", text)
        self.assertIn("does not classify anything", text)
        self.assertIn("Users are responsible for their own studies", text)
        self.assertIn("NOT REVIEWED", text)
        self.assertIn(release_notes.EXPORT_LINE, text.replace("\n> ", " ").replace("> ", ""))


if __name__ == "__main__":
    unittest.main()
