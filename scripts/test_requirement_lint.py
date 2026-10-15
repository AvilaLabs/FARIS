# SPDX-License-Identifier: AGPL-3.0-only
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import requirement_lint as rl
import requirement_trace as rt

HEAD = "| ID | Requirement | Target | Verification | Basis | Phase | Now |\n| --- | --- | --- | --- | --- | --- | --- |\n"
GOOD = "| AAA-001 | FARIS shall x. | 1 | Unit test. | FARIS choice | F1 | Partial: some, missing the rest |"


def lint(*rows: str, trace: dict | None = None, extra: dict | None = None) -> list[tuple[str, str, str]]:
    files = {"01-a.md": "# A\n\n" + HEAD + "\n".join(rows) + "\n"}
    files.update(extra or {})
    return rl.lint_tables(files, trace or {})


def rules(findings) -> list[str]:
    return [f[0] for f in findings]


class RowRules(unittest.TestCase):
    # Verifies: QA-001
    def test_good_row_is_clean(self):
        self.assertEqual(lint(GOOD), [])

    # Verifies: QA-001
    def test_wrong_cell_count_and_empty_cell(self):
        self.assertEqual(rules(lint("| AAA-001 | a | b | c | d | F1 |")), ["cells"])
        self.assertIn("empty-cell", rules(lint("| AAA-001 | a | | v test | FARIS choice | F1 | No |")))

    # Verifies: QA-001
    def test_missing_phase_and_verification_fail(self):
        found = rules(lint("| AAA-001 | a | t | | FARIS choice | | No |"))
        self.assertEqual(found.count("empty-cell"), 2)

    # Verifies: QA-001
    def test_phase_forms(self):
        ok = "| AAA-001 | a | t | test | FARIS choice | {} | No |"
        for phase in ("F7", "F1, F7", "F1 (analytic), F7 (CAD)", "F2 (5), F6 (10)"):
            self.assertEqual(lint(ok.format(phase)), [], phase)
        for phase in ("F8", "F0", "F1/F2", "later", "F1 and F2", "F1 ()"):
            self.assertIn("phase", rules(lint(ok.format(phase))), phase)

    def test_status_word_and_explanation(self):
        row = "| AAA-001 | a | t | test | FARIS choice | F1 | {} |"
        self.assertIn("now-status", rules(lint(row.format("Done: yes"))))
        self.assertIn("now-status", rules(lint(row.format("met"))))
        self.assertIn("now-explain", rules(lint(row.format("Partial"))))
        self.assertIn("now-explain", rules(lint(row.format("Partial: "))))
        self.assertIn("now-explain", rules(lint(row.format("Unmeasured"))))
        self.assertEqual(lint(row.format("No")), [])
        self.assertEqual(lint(row.format("No: nothing exists")), [])

    def test_basis_and_method(self):
        row = "| AAA-001 | a | t | {} | {} | F1 | No |"
        self.assertEqual(lint(row.format("Unit test.", "[U]; FARIS choice")), [])
        self.assertEqual(lint(row.format("Unit test.", "R5 PD-01")), [])
        self.assertEqual(lint(row.format("Unit test.", "OpenMC [V](https://x.org)")), [])
        self.assertIn("basis-mark", rules(lint(row.format("Unit test.", "somebody said so"))))
        self.assertIn("method", rules(lint(row.format("Nothing.", "[U]"))))

    def test_escaped_pipe_is_reported(self):
        self.assertIn("no-pipe", rules(lint("| AAA-001 | a \\|z\\| | t | test | [U] | F1 | No |")))

    def test_bad_header(self):
        files = {"01-a.md": "| ID | Requirement | Target | Verification | Basis | Now | Phase |\n| --- |\n"}
        self.assertEqual(rules(rl.lint_tables(files, {})), ["header"])


class IdRules(unittest.TestCase):
    def test_malformed_id_is_seen(self):
        self.assertIn("id-form", rules(lint("| AAA-1 | a | t | test | [U] | F1 | No |")))
        self.assertIn("id-form", rules(lint("| aaa-001 | a | t | test | [U] | F1 | No |")))
        self.assertIn("id-form", rules(lint("| ~~AAA-001 | a | t | test | [U] | F1 | No |")))

    def test_duplicates_and_prefix_ownership(self):
        row = "| AAA-001 | a | t | test | [U] | F1 | No |"
        self.assertIn("id-unique", rules(lint(row, row)))
        other = {"02-b.md": "# B\n\n" + HEAD + "| AAA-001 | a | t | test | [U] | F1 | No |\n"}
        self.assertIn("id-unique", rules(lint(row, extra=other)))
        other = {"02-b.md": "# B\n\n" + HEAD + "| AAA-002 | a | t | test | [U] | F1 | No |\n"}
        self.assertEqual(rules(lint(row, extra=other)), ["id-prefix"])

    def test_struck_row_follows_convention(self):
        ok = "| ~~AAA-001~~ | ~~FARIS shall x.~~ | t | test | [U] | F1 | No |"
        self.assertEqual(lint(ok), [])
        self.assertIn("struck-form", rules(lint("| ~~AAA-001~~ | FARIS shall x. | t | test | [U] | F1 | No |")))
        self.assertIn("struck-form", rules(lint("| AAA-001 | ~~FARIS shall x.~~ | t | test | [U] | F1 | No |")))
        # A withdrawn row is exempt from the status rules, but its id still counts for uniqueness.
        self.assertEqual(lint("| ~~AAA-001~~ | ~~x~~ | t | test | [U] | F1 | Withdrawn |"), [])
        self.assertIn("id-unique", rules(lint(ok, GOOD)))


class MetRule(unittest.TestCase):
    ROW = "| AAA-001 | a | t | {} | [U] | F1 | Met: done |"

    # Verifies: QA-001
    def test_met_needs_a_test_unless_audit(self):
        self.assertIn("met-traced", rules(lint(self.ROW.format("Unit test."))))
        self.assertEqual(lint(self.ROW.format("Unit test."), trace={"AAA-001": ["t.rs::a"]}), [])
        self.assertEqual(lint(self.ROW.format("Package audit.")), [])
        self.assertEqual(lint("| AAA-001 | a | t | Unit test. | [U] | F1 | Partial: some |"), [])


class HistoryRules(unittest.TestCase):
    OLD = {"AAA-001": (False, "FARIS shall keep requirement ids stable"), "AAA-002": (True, "FARIS shall x")}

    # Verifies: QA-008
    def test_unchanged_is_clean(self):
        self.assertEqual(rl.lint_history(self.OLD, dict(self.OLD), "v1"), [])

    # Verifies: QA-008
    def test_removed_id_needs_a_tombstone(self):
        new = {"AAA-002": (True, "FARIS shall x")}
        self.assertEqual(rules(rl.lint_history(self.OLD, new, "v1")), ["id-removed"])
        new["AAA-001"] = (True, "FARIS shall keep requirement ids stable")
        self.assertEqual(rl.lint_history(self.OLD, new, "v1"), [])

    # Verifies: QA-008
    def test_reused_and_revived_ids(self):
        new = {"AAA-001": (False, "The viewport shall render at 60 frames per second"), "AAA-002": (False, "FARIS shall x")}
        self.assertEqual(rules(rl.lint_history(self.OLD, new, "v1")), ["id-reused", "id-revived"])

    # Verifies: QA-008
    def test_rewording_keeps_the_id(self):
        new = {"AAA-001": (False, "FARIS shall keep requirement ids stable and never reuse them."),
               "AAA-002": (True, "FARIS shall x")}
        self.assertEqual(rl.lint_history(self.OLD, new, "v1"), [])

    # Verifies: QA-008
    def test_table_ids_reads_live_and_struck(self):
        text = "# A\n\n" + HEAD + GOOD + "\n| ~~AAA-002~~ | ~~gone~~ | t | test | [U] | F1 | No |\n"
        self.assertEqual(rl.table_ids({"01-a.md": text}),
                         {"AAA-001": (False, "FARIS shall x."), "AAA-002": (True, "gone")})


class RealTables(unittest.TestCase):
    # Verifies: QA-001
    def test_committed_tables_have_no_errors(self):
        files = rl.current_files()
        trace, _ = rt.read_markers()
        errors = [f for f in rl.lint_tables(files, trace) if f[0] not in rl.ADVISORY]
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
