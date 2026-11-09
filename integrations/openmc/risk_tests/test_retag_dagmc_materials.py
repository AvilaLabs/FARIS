# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure MOAB/HDF5 fixture checks for immutable void-to-filler H5M retagging."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
try:
    import h5py
except ImportError:
    h5py = None

HERE = Path(__file__).resolve().parent


def _load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


H5M = _load("h5m_volumes")
RETAG = _load("retag_dagmc_materials")
BUILDER = _load("build_rm_m")


def fixture(path: Path, *, include_explicit_void: bool = True):
    """Write a tiny H5M-layout fixture: steel volume, optional void volume, complement marker."""
    with h5py.File(path, "w") as f:
        t = f.create_group("tstt")
        nodes = t.create_group("nodes")
        coords = nodes.create_dataset("coordinates", data=np.array([[0.0, 0.0, 0.0]]))
        coords.attrs["start_id"] = np.int64(1)
        tri = t.create_group("elements").create_group("Tri3")
        conn = tri.create_dataset("connectivity", data=np.empty((0, 3), dtype=np.uint64))
        conn.attrs["start_id"] = np.int64(10)
        sets = t.create_group("sets")
        if include_explicit_void:
            row_values = [[-1, -1, -1, 0], [-1, -1, -1, 0], [0, -1, -1, 0],
                          [1, -1, -1, 0], [3, -1, -1, 0]]
            contents_values = [1000, 1001, 1000, 1001]
        else:
            row_values = [[-1, -1, -1, 0], [-1, -1, -1, 0], [0, -1, -1, 0], [2, -1, -1, 0]]
            contents_values = [1000, 1000, 1001]
        rows = np.array(row_values, dtype=np.int64)
        row_ds = sets.create_dataset("list", data=rows)
        row_ds.attrs["start_id"] = np.int64(1000)
        sets.create_dataset("contents", data=np.array(contents_values, dtype=np.uint64))
        sets.create_dataset("children", data=np.empty((0,), dtype=np.uint64))
        sets.create_dataset("parents", data=np.empty((0,), dtype=np.uint64))
        # MOAB files can carry opaque and vlen/object datasets alongside geometry.
        opaque = np.frombuffer(b"opaque00opaque01opaque02", dtype="V8").copy()
        t.create_dataset("opaque_fixture", data=opaque)
        vlen = t.create_dataset("vlen_fixture", shape=(2,), dtype=h5py.special_dtype(vlen=bytes))
        vlen[...] = [b"A" * 5000, b"B" * 5000]
        tags = t.create_group("tags")
        for name, ids, values in (
            ("GEOM_DIMENSION", [1000, 1001], [3, 3]),
            ("GLOBAL_ID", [1000, 1001], [8, 9]),
            ("GEOM_SENSE_2", [], []),
        ):
            group = tags.create_group(name)
            group.create_dataset("id_list", data=np.asarray(ids, dtype=np.uint64))
            array = np.empty((0, 2), dtype=np.uint64) if name == "GEOM_SENSE_2" else np.asarray(values, dtype=np.int32)
            group.create_dataset("values", data=array)
        name = tags.create_group("NAME")
        if include_explicit_void:
            name.create_dataset("id_list", data=np.array([1002, 1003, 1004], dtype=np.uint64))
            name.create_dataset("values", data=np.array([b"mat:steel", b"mat:void", b"mat:void_comp"], dtype="S32"))
        else:
            name.create_dataset("id_list", data=np.array([1002, 1003], dtype=np.uint64))
            name.create_dataset("values", data=np.array([b"mat:steel", b"mat:void_comp"], dtype="S32"))


@unittest.skipIf(h5py is None, "h5py is not installed in this Python")
class RetagDagmcMaterialsTests(unittest.TestCase):
    def test_retags_only_void_name_groups_and_preserves_source_and_geometry(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / "original.h5m", Path(tmp) / "random-ray.h5m"
            fixture(source)
            source_before = source.read_bytes()
            record = RETAG.retag_void_h5m(source, target, [9])
            self.assertEqual(source.read_bytes(), source_before)
            self.assertEqual(record["changed_material_groups"], [
                {"set_handle": 1003, "from": "mat:void", "to": "mat:filler", "volume_global_ids": [9]},
                {"set_handle": 1004, "from": "mat:void_comp", "to": "mat:filler_comp", "volume_global_ids": [8, 9]},
            ])
            with h5py.File(source, "r") as original, h5py.File(target, "r") as copied:
                for dataset in RETAG.IMMUTABLE_DATASETS:
                    np.testing.assert_array_equal(original[dataset][()], copied[dataset][()])
                before = [bytes(value).rstrip(b"\0").decode() for value in original["tstt/tags/NAME/values"][()]]
                after = [bytes(value).rstrip(b"\0").decode() for value in copied["tstt/tags/NAME/values"][()]]
                self.assertEqual(before, ["mat:steel", "mat:void", "mat:void_comp"])
                self.assertEqual(after, ["mat:steel", "mat:filler", "mat:filler_comp"])
            self.assertIn("tstt/nodes/coordinates", record["geometry_dataset_hashes"])
            self.assertIn("tstt/sets/contents", record["immutable_dataset_hashes"])

    def test_wrong_void_role_ids_or_missing_explicit_void_fail_without_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / "original.h5m", Path(tmp) / "random-ray.h5m"
            fixture(source)
            with self.assertRaisesRegex(ValueError, "do not match"):
                RETAG.retag_void_h5m(source, target, [8])
            self.assertFalse(target.exists())
            fixture(source, include_explicit_void=False)
            with self.assertRaisesRegex(ValueError, "exactly one mat:void"):
                RETAG.retag_void_h5m(source, target, [9])
            self.assertFalse(target.exists())

    def test_existing_destination_is_never_replaced(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / "original.h5m", Path(tmp) / "random-ray.h5m"
            fixture(source)
            target.write_bytes(b"prior output")
            with self.assertRaises(FileExistsError):
                RETAG.retag_void_h5m(source, target, [9])
            self.assertEqual(target.read_bytes(), b"prior output")

    def test_dataset_identity_hash_preserves_opaque_and_long_vlen_payloads(self):
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp) / "first.h5", Path(tmp) / "second.h5"
            for path, changed in ((first, False), (second, True)):
                with h5py.File(path, "w") as f:
                    opaque = np.frombuffer(b"opaque00opaque01opaque02", dtype="V8").copy()
                    if changed:
                        opaque[1] = np.void(b"OPAQUE!!")
                    f.create_dataset("opaque", data=opaque)
                    vlen = f.create_dataset("vlen", shape=(1,), dtype=h5py.special_dtype(vlen=bytes))
                    payload = b"A" * 2500 + (b"B" if changed else b"A") + b"A" * 2499
                    vlen[0] = payload
            with h5py.File(first, "r") as left, h5py.File(second, "r") as right:
                self.assertNotEqual(RETAG._dataset_hash(left["opaque"]), RETAG._dataset_hash(right["opaque"]))
                self.assertNotEqual(RETAG._dataset_hash(left["vlen"]), RETAG._dataset_hash(right["vlen"]))
                # Full retag verification hashes these datasets without truncating/stringifying arrays.
                self.assertGreater(len(RETAG._canonical_dataset_payload(left["vlen"])), 10_000)

    def test_facet_material_lookup_uses_direct_contents_and_ignores_complement(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "tagged.h5m"
            fixture(source)
            observed = H5M.facet_volumes(str(source))
            self.assertEqual(observed[8]["material_tag"], "steel")
            self.assertEqual(observed[9]["material_tag"], "void")

    def test_builder_refuses_a_geometry_match_with_the_wrong_material_group(self):
        summary = {"name": "plasma", "tag": "void", "role": "plasma",
                   "cad_volume_cm3": 1.0, "centroid_cm": [0.0, 0.0, 0.0]}
        facets = {8: {"faceted_volume": 1.0, "centroid": [0.0, 0.0, 0.0],
                      "triangles": 12, "material_tag": "steel"}}
        with patch.object(BUILDER, "solid_summary", return_value=summary):
            with self.assertRaisesRegex(RuntimeError, "H5M material tag"):
                BUILDER.match_roles([{}], facets)
            facets[8]["material_tag"] = "void"
            roles = BUILDER.match_roles([{}], facets)
        self.assertEqual(roles[0]["volume_id"], 8)
        self.assertEqual(roles[0]["h5m_material_tag"], "void")


if __name__ == "__main__":
    unittest.main()
