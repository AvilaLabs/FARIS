"""Tests of integrations/openmc/streaming_aim.py: the decisions around an aimed run (no OpenMC needed)."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import streaming_aim as A  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0f_common as F  # noqa: E402


class StreamingAimTest(unittest.TestCase):
    def test_patched_library_is_recognised_by_its_variables(self):
        with tempfile.TemporaryDirectory() as d:
            stock, patched, partial = Path(d) / "stock.so", Path(d) / "patched.so", Path(d) / "partial.so"
            stock.write_bytes(b"\x7fELF OPENMC_CROSS_SECTIONS weight windows")
            patched.write_bytes(b"\x7fELF" + b"\x00".join(A.MARKERS) + b"\x00tail")
            partial.write_bytes(b"\x7fELF OPENMC_WW_SKIP_VOID")
            self.assertFalse(A.aiming_supported(stock))
            self.assertTrue(A.aiming_supported(patched))
            self.assertFalse(A.aiming_supported(partial))
            self.assertFalse(A.aiming_supported(Path(d) / "missing.so"))
            self.assertFalse(A.aiming_supported(None))

    def test_stock_openmc_falls_back_and_says_why(self):
        out = A.choose(False, None)
        self.assertEqual(out["mode"], "windows-only")
        self.assertEqual(out["env"], {})
        self.assertIn("cannot aim", out["reason"])
        self.assertIn(A.PATCH, out["next_step"])
        self.assertTrue((Path(__file__).resolve().parents[3] / A.PATCH).is_file())

    def test_no_line_of_sight_falls_back_and_says_why(self):
        self.assertIsNone(A.choose(True, None))
        bent = A.choose(True, [])
        self.assertEqual(bent["mode"], "windows-only")
        self.assertIn("line of sight", bent["reason"])
        open_face = A.choose(True, [("face_0", None, None, None, None)])
        self.assertEqual(open_face["mode"], "windows-only")
        self.assertIn("no narrow channel", open_face["reason"])
        for out in (bent, open_face):
            self.assertTrue(out["next_step"])
        self.assertIsNone(A.choose(True, [("face_0",) + (None,) * 4, ("exit_0",) + (None,) * 4]))

    def test_aimed_run_sets_protocol_5_variables(self):
        spec = F.AUTO_APERTURE
        self.assertTrue(spec.startswith(f"{A.SOURCE_BETA:g} "))
        self.assertEqual(A.aim_env(spec), F.CONFIG_ENV["F9"])
        self.assertTrue(F.env_matches("F9", A.aim_env(spec)))


if __name__ == "__main__":
    unittest.main()
