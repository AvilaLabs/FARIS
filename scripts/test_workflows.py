# SPDX-License-Identifier: AGPL-3.0-only
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


if __name__ == "__main__":
    unittest.main()
