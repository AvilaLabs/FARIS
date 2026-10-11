"""Tests for the UX-079 baseline driver's logic (cad_to_answer_baseline). Standard library only."""

import json
import unittest
from pathlib import Path
import tempfile

import cad_to_answer_baseline as B

ROLES = [{"name": "plasma", "tag": "void", "volume_id": 1, "cad_volume_cm3": 100.0},
         {"name": "wall", "tag": "tungsten", "volume_id": 2, "cad_volume_cm3": 50.0},
         {"name": "tf_coil_00", "tag": "tf_coil", "volume_id": 3, "cad_volume_cm3": 10.0}]


class Tags(unittest.TestCase):
    def test_one_tag_per_solid_in_order(self):
        self.assertEqual(B.solid_tags(ROLES), ["void", "tungsten", "tf_coil"])
        self.assertEqual(B.tag_entries(ROLES), [{"step_index": 0, "tag": "void"}, {"step_index": 1, "tag": "tungsten"},
                                                {"step_index": 2, "tag": "tf_coil"}])

    def test_real_roles_file_tags_fit_the_converter(self):
        path = B.MODEL / B.ROLES_NAME
        if not path.is_file():
            self.skipTest("RM-M model folder not present")
        roles = json.loads(path.read_text())["solids"]
        self.assertEqual(len(B.solid_tags(roles)), 30)
        self.assertTrue(all(0 < len(t) <= 28 for t in B.solid_tags(roles)))


class Limits(unittest.TestCase):
    def test_own_limits_when_cap_is_free(self):
        self.assertEqual(B.effective_limit("B2", 0.0, 2), 1500.0)
        self.assertEqual(B.effective_limit("B1", 1200.0, 1), 2700.0)

    def test_cap_shrinks_the_limit(self):
        # 4200 - 2500 used - 300 reserved for one validity = 1400 < 2700
        self.assertEqual(B.effective_limit("B1", 2500.0, 1), 1400.0)
        self.assertEqual(B.effective_limit("B1", 4200.0, 1), 0.0)

    def test_start_threshold(self):
        self.assertFalse(B.can_start(100.0))
        self.assertTrue(B.can_start(B.MIN_CONVERSION_S))

    def test_constants(self):
        self.assertEqual((B.B1_LIMIT_S, B.B2_LIMIT_S, B.TOTAL_CAP_S), (2700.0, 1500.0, 4200.0))
        self.assertEqual((B.CHORD_CM, B.ANGULAR_RAD, B.SCALE_FACTOR, B.VALIDITY_HISTORIES), (0.1, 0.1, 0.1, 10000))


class Records(unittest.TestCase):
    def test_wall_report(self):
        self.assertEqual(B.wall_report(870.4, False, 1500.0), "870 s")
        self.assertEqual(B.wall_report(1500.2, True, 1500.0), "more than 1500 s")

    def test_timeout_is_reported_as_more_than(self):
        job = {"returncode": "timeout", "wall_s": 1500.1, "loadavg_start": "1 1 1 1/1 1", "loadavg_end": "3 1 1 1/1 1", "contended": False}
        rec = B.conversion_record("B2", ["x"], job, 1500.0, None, {})
        self.assertTrue(rec["timed_out"])
        self.assertFalse(rec["h5m_written"])
        self.assertEqual(rec["wall_report"], "more than 1500 s")

    def test_h5m_written_needs_a_zero_exit_and_the_file(self):
        with tempfile.TemporaryDirectory() as d:
            h5m = Path(d) / "model.h5m"
            h5m.write_bytes(b"x")
            ok = B.conversion_record("B1", ["x"], {"returncode": 0, "wall_s": 10.0}, 2700.0, h5m, {})
            bad = B.conversion_record("B1", ["x"], {"returncode": 1, "wall_s": 10.0}, 2700.0, h5m, {})
            self.assertTrue(ok["h5m_written"])
            self.assertFalse(bad["h5m_written"])

    def test_sha256(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "a"
            p.write_bytes(b"abc")
            self.assertEqual(B.sha256(p), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")


class Validity(unittest.TestCase):
    def test_zero_lost_is_valid(self):
        self.assertTrue(B.validity_verdict(0, {"restart_files": 0})["valid"])

    def test_lost_particles_invalidate(self):
        v = B.validity_verdict(0, {"restart_files": 3})
        self.assertFalse(v["valid"])
        self.assertIn("3", v["reason"])

    def test_failed_run_is_invalid(self):
        self.assertFalse(B.validity_verdict(1, None)["valid"])
        self.assertFalse(B.validity_verdict(0, None)["valid"])

    def test_volume_mapping(self):
        facets = {1: {"faceted_volume": 100.01}, 2: {"faceted_volume": 50.0}, 3: {"faceted_volume": 10.0}}
        self.assertTrue(B.volume_mapping(ROLES, facets)["volumes_within_rtol"])
        facets[3] = {"faceted_volume": 20.0}
        m = B.volume_mapping(ROLES, facets)
        self.assertEqual(m["ids_off"], [3])
        self.assertFalse(B.volume_mapping(ROLES, {1: facets[1]})["ids_match"])


if __name__ == "__main__":
    unittest.main()
