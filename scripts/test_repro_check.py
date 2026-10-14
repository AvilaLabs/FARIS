# SPDX-License-Identifier: AGPL-3.0-only
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_gate as rg  # noqa: E402

SCRIPT = Path(__file__).resolve().parent / "repro_check.sh"


def run(*args):
    return subprocess.run(["bash", str(SCRIPT), *args], capture_output=True, text=True)


class ReproCheck(unittest.TestCase):
    # Verifies: QA-063
    def test_the_recipe_remaps_checkout_cargo_rustup_and_target_and_pins_the_epoch(self):
        out = run("--env", "--target", "/some/target-dir")
        self.assertEqual(out.returncode, 0, out.stderr)
        flags = next(l for l in out.stdout.splitlines() if "RUSTFLAGS" in l)
        for destination in ("/build/faris", "/build/cargo", "/build/rustup", "/build/target"):
            self.assertIn("=" + destination, flags)
        self.assertIn("--remap-path-prefix=/some/target-dir=/build/target", flags)
        self.assertRegex(out.stdout, r"export SOURCE_DATE_EPOCH=\d+")

    # Verifies: QA-063
    def test_equal_binaries_write_evidence_the_release_gate_accepts(self):
        with tempfile.TemporaryDirectory() as tmp:
            a, b, ev = (Path(tmp) / n for n in ("a", "b", "repro.json"))
            a.write_bytes(b"binary")
            b.write_bytes(b"binary")
            out = run("--compare", str(a), str(b), "--evidence", str(ev))
            self.assertEqual(out.returncode, 0, out.stderr)
            data = json.loads(ev.read_text())
            gate = rg.Gate(rg.ROOT, commit=data["commit"], repro=ev)
            passed, reason = rg.check_repro(gate)
        self.assertTrue(passed, reason)

    # Verifies: QA-063
    def test_different_binaries_exit_1_and_the_gate_rejects_the_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            a, b, ev = (Path(tmp) / n for n in ("a", "b", "repro.json"))
            a.write_bytes(b"one")
            b.write_bytes(b"two")
            out = run("--compare", str(a), str(b), "--evidence", str(ev))
            self.assertEqual(out.returncode, 1)
            self.assertIn("NOT reproducible", out.stderr)
            data = json.loads(ev.read_text())
            passed, _ = rg.check_repro(rg.Gate(rg.ROOT, commit=data["commit"], repro=ev))
        self.assertFalse(passed)


if __name__ == "__main__":
    unittest.main()
