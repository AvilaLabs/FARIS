import tempfile
import unittest
from pathlib import Path

import fuzz_seed

ROOT = Path(__file__).resolve().parent.parent


class FuzzSeed(unittest.TestCase):
    # Verifies: QA-033
    def test_every_fuzz_target_gets_seeds_from_the_repository(self):
        targets = sorted(p.stem for p in (ROOT / "fuzz" / "fuzz_targets").glob("*.rs"))
        self.assertTrue(targets)
        with tempfile.TemporaryDirectory() as out:
            counts = fuzz_seed.build(Path(out))
            for target in targets:
                self.assertGreater(counts.get(target, 0), 0, f"{target} has no seeds")
                self.assertTrue(any((Path(out) / target).iterdir()))
            self.assertEqual(set(counts) - set(targets), set(), "seeds for a target that does not exist")


if __name__ == "__main__":
    unittest.main()
