# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Checks on the CI workflow files: pinned actions, token permissions and the jobs other requirements name."""
import re
import unittest
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github/workflows"
USES = re.compile(r"^\s*(?:-\s+)?uses:\s*(\S+)(?:\s+#\s*(.*))?$", re.M)


def workflow_files():
    return sorted(WORKFLOWS.glob("*.yml"))


class WorkflowTests(unittest.TestCase):
    def test_every_action_is_pinned_by_full_commit_hash_with_its_version_in_a_comment(self):
        # Verifies: SEC-033
        problems = []
        count = 0
        for path in workflow_files():
            for match in USES.finditer(path.read_text(encoding="utf-8")):
                reference, comment = match.group(1), match.group(2)
                if reference.startswith("./"):
                    continue
                count += 1
                if not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", reference) or not comment:
                    problems.append(f"{path.name}: {reference}")
        self.assertGreater(count, 20)
        self.assertEqual(problems, [])

    def test_tokens_are_read_only_by_default_and_no_workflow_runs_untrusted_code_with_secrets(self):
        # Verifies: SEC-033
        for path in workflow_files():
            text = path.read_text(encoding="utf-8")
            self.assertRegex(text, r"(?m)^permissions:\s*\n\s+contents:\s*(read|write)\b", path.name)
            self.assertNotIn("pull_request_target", text, path.name)
        # Only the release smoke test needs write access (reading draft releases), and it says why.
        writers = [p.name for p in workflow_files() if re.search(r"(?m)^permissions:\s*\n\s+contents:\s*write", p.read_text())]
        self.assertEqual(writers, ["release-smoke.yml"])

    # Verifies: SEC-031
    def test_the_main_workflow_runs_the_static_analysis_and_size_and_sign_off_checks(self):
        # Verifies: QA-039, QA-090, LEG-005, QA-022
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("cargo clippy --workspace", text)
        self.assertIn("-D warnings", text)
        self.assertIn("cargo doc --workspace", text)
        self.assertRegex(text, r"(?m)^  audit:\s*$")
        self.assertIn("scripts/check_audit.py", text)
        self.assertIn("scripts/check_repository_size.py", text)
        self.assertRegex(text, r"(?m)^  sign-off:\s*$")
        self.assertRegex(text, r"(?m)^  recorded-demo:\s*$")
        self.assertIn("verify.sh", text)

    def test_the_toolchain_and_every_dependency_are_pinned_and_ci_uses_the_lock_file(self):
        # Verifies: QA-060
        root = WORKFLOWS.parent.parent
        self.assertRegex((root / "rust-toolchain.toml").read_text(encoding="utf-8"), r'channel = "\d+\.\d+\.\d+"')
        self.assertTrue((root / "Cargo.lock").is_file())
        for path in workflow_files():
            text = path.read_text(encoding="utf-8")
            for match in re.finditer(r"^\s*(?:- )?(?:run: )?(cargo (?:build|test|clippy|doc|check|metadata|fetch)\b[^\n]*)$", text, re.M):
                self.assertIn("--locked", match.group(1), f"{path.name}: {match.group(1)}")
            for match in re.finditer(r"toolchain:\s*(\S+)", text):
                self.assertTrue(match.group(1) in ("1.98.1", "nightly") or match.group(1).startswith("${{"), f"{path.name}: {match.group(0)}")

    def test_the_sign_off_job_covers_every_pull_request_and_the_history_job_has_full_depth(self):
        # Verifies: LEG-005, QA-008, QA-085
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        sign_off = re.search(r"(?ms)^  sign-off:\n(.*?)^  \S", text).group(1)
        self.assertIn("if: github.event_name == 'pull_request'", sign_off)  # every pull request, forks or not
        self.assertNotIn("fork", sign_off.split("steps:")[0])
        self.assertIn("scripts/check_signoff.py", sign_off)
        self.assertIn("fetch-depth: 0", sign_off)
        history = re.search(r"(?ms)^  history:\n(.*?)^  \S", text).group(1)
        self.assertIn("fetch-depth: 0", history)
        self.assertIn("scripts/requirement_lint.py --against", history)
        self.assertIn("--history", history)
        self.assertIn("scripts/merge_audit.py", history)
        self.assertIn("scripts/check_external_data.py", text)

    def test_cargo_deny_runs_daily_and_on_dependency_changes_with_the_licence_policy_of_the_checker(self):
        # Verifies: QA-039, LEG-003
        import json
        import tomllib

        root = WORKFLOWS.parent.parent
        workflow = (WORKFLOWS / "deny.yml").read_text(encoding="utf-8")
        self.assertRegex(workflow, r"(?m)^  schedule:\s*\n\s+- cron:")
        self.assertIn("cargo deny --locked check licenses bans sources", workflow)
        self.assertIn("tool: cargo-deny", workflow)
        deny = tomllib.loads((root / "deny.toml").read_text(encoding="utf-8"))
        policy = json.loads((root / "scripts" / "licence_policy.json").read_text(encoding="utf-8"))
        self.assertEqual(deny["licenses"]["allow"], policy["allowed"])
        self.assertNotIn("GPL-2.0-only", deny["licenses"]["allow"])
        self.assertEqual([e["crate"] for e in deny["licenses"]["exceptions"]], [e["package"] for e in policy["exceptions"]])
        self.assertEqual(deny["sources"]["unknown-registry"], "deny")
        self.assertEqual(deny["sources"]["unknown-git"], "deny")


if __name__ == "__main__":
    unittest.main()
