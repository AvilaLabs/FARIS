#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_settings as cs  # noqa: E402

REPO = cs.ROOT
CI = (REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")


class KeyHistoryTests(unittest.TestCase):
    KEYS = ["a.one", "a.two"]

    def history(self, **over):
        h = {"live": {"a.one": "0.2.0", "a.two": "0.2.0"}, "retired": []}
        h.update(over)
        return h

    def test_the_repository_history_matches_the_registry_and_has_no_retired_key_in_it(self):
        # Verifies: CFG-076
        keys = cs.registry_keys()
        self.assertGreaterEqual(len(keys), 40)
        history = json.loads((REPO / cs.HISTORY_FILE).read_text(encoding="utf-8"))
        self.assertEqual(cs.check_history(keys, history), [])
        self.assertEqual(set(keys) & {e["key"] for e in history["retired"]}, set())

    def test_a_clean_history_has_no_problem(self):
        # Verifies: CFG-076
        retired = [{"key": "a.old", "replacement": "a.one", "retired_in": "0.3.0"}]
        self.assertEqual(cs.check_history(self.KEYS, self.history(retired=retired)), [])

    def test_a_retired_key_that_reappears_in_the_registry_fails(self):
        # Verifies: CFG-076
        retired = [{"key": "a.two", "replacement": "a.three", "retired_in": "0.3.0"}]
        problems = cs.check_history(self.KEYS, self.history(retired=retired))
        self.assertTrue(any("a.two is in the registry again" in p for p in problems), problems)

    def test_a_key_dropped_without_retiring_it_fails(self):
        # Verifies: CFG-076
        problems = cs.check_history(["a.one"], self.history())
        self.assertTrue(any("a.two was released but is gone" in p for p in problems), problems)

    def test_a_new_key_must_be_recorded_in_the_history(self):
        # Verifies: CFG-076
        problems = cs.check_history(self.KEYS + ["a.new"], self.history())
        self.assertTrue(any("a.new is not in the key history" in p for p in problems), problems)

    def test_a_retired_entry_needs_a_replacement_or_a_reason_and_a_version(self):
        # Verifies: CFG-076
        bare = {"key": "a.old", "replacement": None, "retired_in": "0.3.0"}
        self.assertTrue(any("no replacement and no reason" in p for p in cs.check_history(self.KEYS, self.history(retired=[bare]))))
        self.assertEqual(cs.check_history(self.KEYS, self.history(retired=[dict(bare, reason="the feature was removed")])), [])
        no_version = {"key": "a.old", "replacement": "a.one"}
        self.assertTrue(cs.check_history(self.KEYS, self.history(retired=[no_version])))
        self.assertTrue(cs.check_history(self.KEYS, {"live": []}))


class EnvironmentAuditTests(unittest.TestCase):
    def test_every_environment_variable_the_code_reads_is_registered_standard_or_listed(self):
        # Verifies: CFG-009
        reads = cs.env_reads()
        self.assertIn("FARIS_MEMORY_CAP_MB", reads)
        self.assertIn("FARIS_CONFIG_DIR", reads)
        self.assertEqual(cs.check_env(reads, cs.registry_envs()), [])

    def test_the_registry_variables_are_found_and_unlisted_ones_fail(self):
        # Verifies: CFG-009
        registered = cs.registry_envs()
        self.assertTrue({"FARIS_CAD_PYTHON", "FARIS_UNITS", "FARIS_MEMORY_CAP_MB"} <= registered)
        problems = cs.check_env({"FARIS_HIDDEN": {"crates/x/src/a.rs"}, "PATH": {"b.rs"}, "CARGO_PKG_NAME": {"c.rs"}}, registered)
        self.assertTrue(any("FARIS_HIDDEN" in p and "crates/x/src/a.rs" in p for p in problems))
        self.assertFalse(any("PATH" in p or "CARGO_PKG_NAME" in p for p in problems))

    def test_an_exception_that_no_code_reads_or_that_is_now_registered_is_reported(self):
        # Verifies: CFG-009
        problems = cs.check_env({}, {"FARIS_SCRATCH_DIR"})
        self.assertTrue(any("FARIS_SCRATCH_DIR, which no code reads" in p for p in problems))
        self.assertTrue(any("FARIS_SCRATCH_DIR, which is in the registry" in p for p in problems))

    def test_the_scan_sees_names_in_literals_and_in_env_calls_but_not_in_comments_or_tests(self):
        # Verifies: CFG-009
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "crates" / "demo" / "src"
            src.mkdir(parents=True)
            (src / "lib.rs").write_text(
                '// FARIS_IN_COMMENT\nconst A: &str = "FARIS_BY_LITERAL";\nfn f() { let _ = std::env::var_os("OTHER_VAR"); }\n'
                '#[cfg(test)]\nmod t { fn g() { let _ = std::env::var("FARIS_IN_TEST"); } }\n', encoding="utf-8")
            reads = cs.env_reads(Path(tmp))
        self.assertEqual(set(reads), {"FARIS_BY_LITERAL", "OTHER_VAR"})

    def test_ci_runs_the_settings_check(self):
        # Verifies: CFG-009, CFG-076
        self.assertIn("python3 scripts/check_settings.py", CI)


if __name__ == "__main__":
    unittest.main()
