import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_lint as S


def lint(files: dict[str, str]) -> list[tuple[str, str, int, str]]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel, text in files.items():
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        return S.lint_rust(root)


def rules(files: dict[str, str]) -> list[str]:
    return [f[0] for f in lint(files)]


ENGINE = "crates/faris-engine/src/x.rs"
APP = "crates/faris-app/src/x.rs"


class SourceLint(unittest.TestCase):
    # Verifies: PRV-008
    def test_wall_clock_in_calculation_code_is_found_but_not_in_tests_comments_or_strings(self):
        src = ('fn f() { let t = std::time::SystemTime::now(); }\n'
               '// SystemTime::now() in a comment\n'
               'fn g() { let s = "SystemTime::now()"; }\n'
               '#[cfg(test)]\nmod tests {\n fn h() { let t = SystemTime::now(); }\n}\n')
        found = lint({ENGINE: src})
        self.assertEqual([(f[0], f[2]) for f in found], [("PRV-008", 1)])
        self.assertEqual(rules({"crates/faris-report/src/x.rs": src}), [])  # not a calculation crate
        self.assertEqual(rules({"crates/faris-engine/src/tests.rs": src}), [])

    # Verifies: PRV-008
    def test_monotonic_timing_is_reported_separately(self):
        self.assertEqual(rules({ENGINE: "fn f() { let t = Instant::now(); }\n"}), ["PRV-008-timing"])
        self.assertIn("PRV-008-timing", S.ADVISORY)

    # Verifies: CFG-054
    def test_ambient_random_sources_are_found(self):
        for call in ("rand::thread_rng()", "StdRng::from_entropy()", "OsRng.fill_bytes(&mut b)", "rand::random::<u64>()"):
            self.assertEqual(rules({ENGINE: f"fn f() {{ let r = {call}; }}\n"}), ["CFG-054"], call)
        self.assertEqual(rules({ENGINE: "fn f(seed: u64) { let r = StdRng::seed_from_u64(seed); }\n"}), [])

    # Verifies: SEC-011
    def test_a_comparison_with_a_truncated_digest_is_found(self):
        for line in ("if sha256[..8] == other { }", "if other != &digest[..16] { }", "if hash.starts_with(prefix) { }"):
            self.assertIn("SEC-011", rules({APP: f"fn f() {{ {line} }}\n"}), line)
        self.assertEqual(rules({APP: "fn f() { if sha256 == other { } }\n"}), [])

    # Verifies: SEC-011
    def test_digest_slices_are_listed_unless_allow_listed(self):
        self.assertEqual(rules({APP: "fn f() { let d = &sha256[..12]; }\n"}), ["SEC-011-slice"])
        self.assertEqual(rules({"crates/faris-engine/src/evidence_store.rs": "fn f() { let d = &sha256[..2]; }\n"}), [])

    # Verifies: PRV-011
    def test_kind_label_constants_and_literals(self):
        self.assertEqual(rules({APP: 'const LABEL: &str = "calculated";\n'}), ["PRV-011"])
        self.assertEqual(rules({APP: 'fn f() -> &str { "authored" }\n'}), ["PRV-011-literal"])
        self.assertEqual(rules({"crates/faris-engine/src/brief.rs": 'fn f() -> &str { "authored" }\n'}), [])

    # Verifies: UX-046
    def test_empty_badge_explanations_and_bare_placeholders_are_found(self):
        self.assertEqual(rules({APP: 'fn f() { badge(ui, Kind::Estimate, "estimate", ""); }\n'}), ["UX-046"])
        self.assertEqual(rules({APP: 'fn f() { badge(ui, Kind::Estimate, "estimate", why); }\n'}), [])
        self.assertEqual(rules({APP: 'fn f() { cell("n/a"); }\n'}), ["UX-046-na"])
        self.assertEqual(rules({APP: 'fn f() { cell("—"); }\n'}), ["UX-046-dash"])

    def test_line_numbers_survive_stripping(self):
        text = '/* a\nb */\nfn f() {\n    "x";\n    let t = SystemTime::now();\n}\n'
        self.assertEqual([f[2] for f in lint({ENGINE: text})], [5])

    # Verifies: DSN-085, DOC-062
    def test_the_tree_has_no_failing_finding(self):
        failing = [f for f in S.lint_rust() + S.lint_statements() if f[0] not in S.ADVISORY]
        self.assertEqual(failing, [])


if __name__ == "__main__":
    unittest.main()
