# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure GEO-030 request and unit-conversion tests; no OpenMC import or solver run."""
import sys
import importlib.util
import math
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import design_volume as dv


def request():
    return {
        "schema": dv.REQUEST_SCHEMA,
        "h5m_sha256": "0" * 64,
        "cross_sections_sha256": "1" * 64,
        "seed": 17,
        "samples": 100_000,
        "bounds_m": {"lower_left": [-0.5, -0.5, -0.1], "upper_right": [0.5, 0.5, 0.1]},
        "targets": [{
            "volume": 1, "global_id": 1, "solid_id": "torus", "material_tag": "mat:torus",
            "cad_volume_m3": 0.098696044, "faceted_volume_m3": 0.0986142629,
        }],
        "surface_ids": [1, 2, 3],
    }


class StochasticVolumeContract(unittest.TestCase):
    def test_request_requires_unique_positive_actual_cell_ids_and_bound_material_tags(self):
        dv.validate_request(request())
        bad = request()
        bad["targets"].append(dict(bad["targets"][0], volume=2))
        with self.assertRaisesRegex(ValueError, "unique"):
            dv.validate_request(bad)
        bad = request()
        bad["targets"][0]["global_id"] = 0
        with self.assertRaisesRegex(ValueError, "positive"):
            dv.validate_request(bad)

    def test_si_bounds_are_converted_to_cm_with_padding(self):
        low, high = dv.padded_bounds_cm(request()["bounds_m"])
        self.assertEqual(low, [-55.0, -55.0, -15.0])
        self.assertEqual(high, [55.0, 55.0, 15.0])

    def test_origin_sphere_contains_every_mixed_sign_corner(self):
        lower = [-100.0, -20.0, -100.0]
        upper = [10.0, 200.0, 10.0]
        radius = dv.bounding_sphere_radius(lower, upper)
        for x in (lower[0], upper[0]):
            for y in (lower[1], upper[1]):
                for z in (lower[2], upper[2]):
                    self.assertLessEqual(math.sqrt(x * x + y * y + z * z), radius)

    def test_far_is_material_tags_normalize_against_openmc_names(self):
        self.assertEqual(dv.dagmc_material_name("mat:core"), "core")
        self.assertEqual(
            dv.resolve_dummy_material_names(
                ["mat:core", "mat:shell"], ["core", "shell", "void"]
            ),
            ["core", "shell", "void"],
        )
        with self.assertRaisesRegex(ValueError, "absent"):
            dv.resolve_dummy_material_names(["mat:missing"], ["core", "void"])


@unittest.skipUnless(importlib.util.find_spec("openmc") and importlib.util.find_spec("h5py"),
                     "OpenMC and h5py are needed for the metadata-only H5M API check")
class DagmcNameApiContract(unittest.TestCase):
    def test_tracked_h5m_manifest_tags_match_openmc_names_after_prefix_normalization(self):
        import openmc

        cad_dir = HERE.parent / "cad"
        sys.path.insert(0, str(cad_dir))
        import faris_facets

        h5m = cad_dir / "fixtures" / "three-boxes.h5m"
        with tempfile.TemporaryDirectory() as temporary:
            exported = faris_facets.export_h5m(h5m, Path(temporary) / "fixture")
        tags = [item["material_tag"] for item in exported["manifest"]["volumes"]]
        actual = openmc.DAGMCUniverse(str(h5m.resolve())).material_names
        self.assertEqual(dv.resolve_dummy_material_names(tags, actual), sorted(actual))


if __name__ == "__main__":
    unittest.main()
