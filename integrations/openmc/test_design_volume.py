# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure GEO-030 request and unit-conversion tests; no OpenMC import or solver run."""
import sys
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


if __name__ == "__main__":
    unittest.main()
