import importlib.util
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("check_stochastic_tests", HERE / "check_stochastic_tests.py")
LINT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LINT)

GOOD = '''
import random
import unittest


class T(unittest.TestCase):
    def test_sampling(self):
        # Stochastic: seed 7; |z| < 3; expected: analytic volume (GEO-024)
        rng = random.Random(7)
        self.assertLess(abs(rng.random() - 0.5), 1.0)

    def test_plain(self):
        self.assertEqual(1 + 1, 2)
'''


def findings(source, exact_rule=True):
    with tempfile.TemporaryDirectory() as temp:
        path = Path(temp) / "test_x.py"
        path.write_text(source)
        return LINT.check_file(path, "test_x.py", exact_rule)


class StochasticTestLint(unittest.TestCase):
    # Verifies: QA-017, QA-093
    def test_a_marked_seeded_test_passes(self):
        self.assertEqual(findings(GOOD), [])

    # Verifies: QA-093
    def test_a_stochastic_test_without_a_marker_is_found(self):
        source = GOOD.replace("        # Stochastic: seed 7; |z| < 3; expected: analytic volume (GEO-024)\n", "")
        (found,) = findings(source)
        self.assertIn("test_sampling", found)
        self.assertIn("records no seed", found)

    # Verifies: QA-017
    def test_acceptance_above_three_sigma_is_found(self):
        source = GOOD.replace("|z| < 3", "|z| < 5")
        self.assertTrue(any("above the 3 sigma rule" in f for f in findings(source)))
        loose = GOOD + "\nZ_LIMIT = 4.5\n"
        self.assertTrue(any("Z_LIMIT 4.5" in f for f in findings(loose)))
        multiple = GOOD.replace("assertLess(abs(rng.random() - 0.5), 1.0)", "assertLess(abs(x), 5 * sigma)")
        self.assertTrue(any("5 sigma" in f for f in findings(multiple)))

    # Verifies: QA-093
    def test_a_malformed_marker_or_seed_is_found(self):
        for bad in (
            "# Stochastic: seed 7; expected: nothing",
            "# Stochastic: seed 7; |z| < 3;",
            "# Stochastic: seed later; |z| < 3; expected: analytic volume",
        ):
            source = GOOD.replace("# Stochastic: seed 7; |z| < 3; expected: analytic volume (GEO-024)", bad)
            self.assertTrue(findings(source), bad)

    # Verifies: QA-017
    def test_unseeded_randomness_is_found(self):
        for call in ("random.Random()", "numpy.random.default_rng()"):
            source = GOOD.replace("random.Random(7)", call)
            self.assertTrue(any("without a seed" in f for f in findings(source)), call)
        module_level = GOOD.replace("rng = random.Random(7)", "rng = random").replace("rng.random()", "random.random()")
        self.assertTrue(any("without `random.seed" in f for f in findings(module_level)))

    # Verifies: QA-017
    def test_exact_float_equality_is_found_unless_justified(self):
        source = GOOD.replace("self.assertEqual(1 + 1, 2)", "self.assertEqual(1 / 3, 0.333)")
        self.assertTrue(any("exact float equality" in f for f in findings(source)))
        compare = GOOD.replace("self.assertEqual(1 + 1, 2)", "assert 1 / 3 == 0.333")
        self.assertTrue(any("exact float equality" in f for f in findings(compare)))
        justified = GOOD.replace(
            "self.assertEqual(1 + 1, 2)", "self.assertEqual(0.5 * 2, 1.0)  # exact: both sides are powers of two"
        )
        self.assertEqual(findings(justified), [])
        # Zero is exact by construction and is not flagged; integers are not floats.
        zero = GOOD.replace("self.assertEqual(1 + 1, 2)", "self.assertEqual(0.0 * 3, 0.0)")
        self.assertEqual(findings(zero), [])

    def test_a_test_that_calls_a_seeded_helper_needs_the_marker_too(self):
        source = '''
import unittest


def run(openmc):
    settings.seed = 11


class T(unittest.TestCase):
    def go(self):
        return run(None)

    def test_it(self):
        self.go()
'''
        (found,) = findings(source)
        self.assertIn("test_it", found)
        marked = source.replace(
            "        self.go()", "        # Stochastic: seed 11; structural; expected: none, a behaviour check\n        self.go()"
        )
        self.assertEqual(findings(marked), [])

    # Verifies: QA-017, QA-093
    def test_the_repository_controls_have_no_findings(self):
        self.assertEqual(LINT.scan(LINT.DEFAULT_ROOTS, False), [])


if __name__ == "__main__":
    unittest.main()
