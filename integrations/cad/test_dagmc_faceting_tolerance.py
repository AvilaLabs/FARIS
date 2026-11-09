# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Focused HDF5 tests for the DAGMC faceting-tolerance metadata repair."""
import tempfile
import unittest
from pathlib import Path
import sys

import h5py
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from dagmc_faceting_tolerance import correct_faceting_tolerance, file_sha256


def make_h5m(path: Path) -> None:
    """Create a small H5M-shaped file using h5py's real on-disk tag layout."""
    with h5py.File(path, "w") as handle:
        tags = handle.create_group("tstt/tags")
        nodes = handle.create_group("tstt/nodes")
        nodes.create_dataset("coordinates", data=np.asarray([[0.0, 1.0, 2.0]], dtype=np.float64))
        sets = handle.create_group("tstt/sets")
        set_list = sets.create_dataset("list", data=np.asarray([[0, 0, 0, 0]], dtype=np.int64))
        set_list.attrs["start_id"] = np.int64(41)
        old = tags.create_group("FACETING_TOLERANCE")
        old.attrs.create("class", 2, dtype=np.int32)
        old["type"] = np.dtype("f8")
        for name in ("default", "global", "values"):
            old.create_dataset(name, data=np.asarray([0.123], dtype=np.float64))
        old.create_dataset("id_list", data=np.asarray([0], dtype=np.uint64))


class FacetingToleranceMetadata(unittest.TestCase):
    def test_copy_writes_exact_sparse_double_root_tag_and_preserves_other_datasets(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.h5m"
            corrected = Path(directory) / "corrected.h5m"
            make_h5m(source)
            source_before = source.read_bytes()

            report = correct_faceting_tolerance(source, corrected, 0.05)

            self.assertEqual(source.read_bytes(), source_before)
            self.assertTrue(report["source_unchanged"])
            self.assertEqual(report["source_sha256"], file_sha256(source))
            self.assertNotEqual(report["source_sha256"], report["corrected_sha256"])
            self.assertEqual(report["declared_mesher_tolerance_cm"], 0.05)
            self.assertIn("not a certified maximum", report["interpretation"])
            with h5py.File(corrected, "r") as handle:
                tags = handle["tstt/tags"]
                self.assertNotIn("FACETING_TOLERANCE", tags)
                tag = tags["FACETING_TOL"]
                self.assertEqual(int(tag.attrs["class"]), 1)
                self.assertEqual(tag["type"].dtype, np.dtype("f8"))
                self.assertEqual(tag["id_list"].dtype, np.dtype("u8"))
                self.assertEqual(tag["id_list"][()].tolist(), [41])
                self.assertEqual(tag["values"].dtype, np.dtype("f8"))
                self.assertEqual(tag["values"][()].tolist(), [0.05])
                np.testing.assert_array_equal(handle["tstt/nodes/coordinates"][()], [[0.0, 1.0, 2.0]])

    def test_preexisting_destination_and_existing_target_tag_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.h5m"
            destination = Path(directory) / "copy.h5m"
            make_h5m(source)
            destination.write_bytes(b"keep")
            with self.assertRaises(FileExistsError):
                correct_faceting_tolerance(source, destination, 0.05)
            self.assertEqual(destination.read_bytes(), b"keep")

            destination.unlink()
            with h5py.File(source, "r+") as handle:
                handle["tstt/tags"].create_group("FACETING_TOL")
            with self.assertRaisesRegex(ValueError, "already has FACETING_TOL"):
                correct_faceting_tolerance(source, destination, 0.05)
            self.assertFalse(destination.exists())

    def test_missing_serialized_geometry_root_handle_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.h5m"
            destination = Path(directory) / "corrected.h5m"
            make_h5m(source)
            with h5py.File(source, "r+") as handle:
                del handle["tstt/sets/list"]
            with self.assertRaisesRegex(ValueError, "sets/list start_id"):
                correct_faceting_tolerance(source, destination, 0.05)
            self.assertFalse(destination.exists())

    def test_invalid_tolerance_fails_before_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.h5m"
            destination = Path(directory) / "corrected.h5m"
            make_h5m(source)
            for value in (0.0, -0.1, float("inf"), float("nan")):
                with self.subTest(value=value), self.assertRaises(ValueError):
                    correct_faceting_tolerance(source, destination, value)
                self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main()
