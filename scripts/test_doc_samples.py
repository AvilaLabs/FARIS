# SPDX-License-Identifier: AGPL-3.0-only
import json
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import doc_samples as ds  # noqa: E402
import release_gate as rg  # noqa: E402

# A stand-in for the faris binary: it prints help for a few command paths and exits 0 for anything else.
FAKE = """#!/bin/sh
last=""
for a in "$@"; do last="$a"; done
if [ "$last" = "--help" ]; then
  case "$*" in
    "--help") printf 'Commands:\\n  validate  Check\\n  history  Ledger\\n' ;;
    "validate --help") printf 'Usage: faris validate <SCENARIO>\\n      --strict  Be strict\\n' ;;
    "history --help") printf 'Commands:\\n  run  Run\\n' ;;
    "history run --help") printf 'Usage: faris history run\\n      --assumptions <A>\\n' ;;
    *) exit 2 ;;
  esac
  exit 0
fi
case "$1" in fail) echo "broken" >&2; exit 2 ;; esac
exit 0
"""


def tree(docs: dict[str, str]) -> tuple[tempfile.TemporaryDirectory, Path, Path]:
    tmp = tempfile.TemporaryDirectory()
    root = Path(tmp.name) / "repo"
    for rel, text in docs.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(text, encoding="utf-8")
    (root / "scenarios/arc-inspired").mkdir(parents=True)
    (root / "scenarios/arc-inspired/demountable-magnet-assumptions.json").write_text("{}")
    (root / "references").mkdir()
    (root / "crates/faris-engine/tests/fixtures").mkdir(parents=True)
    (root / "crates/faris-engine/tests/fixtures/determinism_rates.json").write_text("{}")
    faris = Path(tmp.name) / "faris"
    faris.write_text(FAKE)
    faris.chmod(faris.stat().st_mode | stat.S_IEXEC)
    return tmp, root, faris


def by_kind(results, kind):
    return [c for c in results.checks if c["kind"] == kind]


class Extraction(unittest.TestCase):
    def test_commands_join_continuations_and_drop_comments_and_optional_parts(self):
        block = ds.Block("x.md", 1, "bash", "# a comment\nfaris history run \\\n  --assumptions A.json [--pack] -o out  # why\n\nfaris doctor\n")
        self.assertEqual(ds.commands(block), ["faris history run --assumptions A.json -o out", "faris doctor"])

    def test_extract_reports_fences_with_lines_and_inline_spans_outside_fences(self):
        tmp, root, _ = tree({"README.md": "Run `faris validate s.json` now.\n\n```bash\nfaris doctor\n```\n\nText `faris` alone.\n"})
        with tmp:
            blocks, spans = ds.extract(root, "README.md")
        self.assertEqual([(b.line, b.lang, b.first) for b in blocks], [(3, "bash", "faris doctor")])
        self.assertEqual(spans, [(1, "faris validate s.json")])


class Dispositions(unittest.TestCase):
    # Verifies: QA-016
    # Verifies: DOC-020
    def test_a_runnable_command_is_executed_and_a_failing_one_fails_the_check(self):
        docs = {"README.md": "```bash\nfaris validate s.json\n```\n\n```bash\nfaris fail\n```\n"}
        tmp, root, faris = tree(docs)
        with tmp:
            results = ds.run_all(root, faris, None)
        executed = by_kind(results, "executed")
        self.assertEqual([c["verdict"] for c in executed], ["PASS"])
        # `fail` is not a command of the binary, so it is caught before it is run.
        self.assertIn("not a command", " ".join(c.get("detail", "") for c in results.failures()))

    # Verifies: QA-016
    # Verifies: DOC-021
    def test_a_flag_the_binary_does_not_have_fails(self):
        tmp, root, faris = tree({"README.md": "```bash\nfaris validate s.json --nonsense\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        self.assertTrue(any("--nonsense" in c.get("detail", "") for c in results.failures()))

    # Verifies: QA-016
    def test_inline_commands_are_resolved_against_the_binary(self):
        tmp, root, faris = tree({"docs/a.md": "Use `faris history run --assumptions A` or `faris bogus x`.\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        inline = by_kind(results, "inline")
        self.assertEqual(sorted(c["verdict"] for c in inline), ["FAIL", "PASS"])

    # Verifies: QA-016
    def test_a_block_with_no_disposition_fails(self):
        tmp, root, faris = tree({"docs/a.md": "```text\nsome new output example\n```\n\n```json\n{\"a\": 1}\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        unknown = [c for c in results.failures() if c["kind"] == "unknown"]
        self.assertEqual(len(unknown), 2)

    def test_a_command_that_needs_an_outside_tool_is_checked_not_run(self):
        tmp, root, faris = tree({"docs/a.md": "```bash\nfaris history run --assumptions /path/to/a.json\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        self.assertEqual([c["verdict"] for c in by_kind(results, "checked")], ["PASS"])
        self.assertEqual(by_kind(results, "executed"), [])

    def test_unknown_programs_fail_and_listed_ones_skip_with_a_reason(self):
        tmp, root, faris = tree({"docs/a.md": "```bash\ntar xzf a.tar.gz\n```\n\n```bash\nfrobnicate now\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        self.assertEqual(by_kind(results, "skipped")[0]["detail"], ds.SKIP_PROGRAMS["package"])
        self.assertEqual([c["kind"] for c in results.failures() if c["kind"] != "json-test"], ["unknown"])

    def test_python_snippets_are_parsed(self):
        tmp, root, faris = tree({"docs/a.md": "```python\nx = (1,\n```\n\n```python\nx = 1\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
        self.assertEqual([c["verdict"] for c in by_kind(results, "python")], ["FAIL", "PASS"])

    def test_the_expected_failure_table_is_empty_and_every_entry_would_need_a_reason(self):
        self.assertEqual(ds.EXPECTED_FAIL, {})


class Evidence(unittest.TestCase):
    # Verifies: QA-016
    def test_the_evidence_file_is_what_the_release_gate_reads(self):
        tmp, root, faris = tree({"docs/a.md": "```bash\nfaris validate s.json\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
            results.checks = [c for c in results.checks if c["kind"] != "json-test"]
            path = Path(tmp.name) / "doc-tests.json"
            path.write_text(json.dumps({"commit": "c" * 40, "checks": results.checks}))
            gate = rg.Gate(root, commit="c" * 40, doc_tests=path)
            passed, reason = rg._checks("doc_tests", "documentation-test evidence")(gate)
        self.assertTrue(passed, reason)

    def test_a_failing_check_blocks_the_gate(self):
        tmp, root, faris = tree({"docs/a.md": "```bash\nfaris validate s.json --nonsense\n```\n"})
        with tmp:
            results = ds.run_all(root, faris, None)
            path = Path(tmp.name) / "doc-tests.json"
            path.write_text(json.dumps({"commit": "c" * 40, "checks": results.checks}))
            gate = rg.Gate(root, commit="c" * 40, doc_tests=path)
            passed, _ = rg._checks("doc_tests", "documentation-test evidence")(gate)
        self.assertFalse(passed)


class RepositoryHandbook(unittest.TestCase):
    def test_every_scanned_block_of_this_repository_has_a_rule(self):
        """Static part of the real run: no block of the handbook is outside the runner's tables."""
        blocks = [b for rel in ds.scanned_files(ds.ROOT) for b in ds.extract(ds.ROOT, rel)[0]]
        self.assertGreater(len(blocks), 40)
        for b in blocks:
            if b.path in ds.LISTING_FILES or b.lang in ("python",):
                continue
            if b.path in ds.GENERATED_FILES:
                self.assertEqual(b.lang, "text", b.name)
                continue
            if b.lang in ("", "bash", "sh"):
                self.assertTrue(ds.commands(b), b.name)
            elif b.lang == "json" and (b.path, b.first) not in ds.FRAGMENTS:
                self.assertIn(b.path, ds.JSON_TESTS, b.name)
            else:
                self.assertIn((b.path, b.first), ds.FRAGMENTS, b.name)


if __name__ == "__main__":
    unittest.main()
