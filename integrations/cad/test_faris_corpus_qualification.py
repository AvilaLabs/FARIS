#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure tests for the scoped default-tolerance corpus qualifier."""
from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import faris_corpus
import faris_facets


class FacetedVolumeTests(unittest.TestCase):
    def setUp(self):
        self.vertices = [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0],
        ]
        self.triangles = [
            [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4],
            [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7],
        ]

    def test_oriented_unit_cube_has_one_cubic_metre(self):
        data = faris_facets.unpack_facets(faris_facets.pack_facets(
            self.vertices, self.triangles, [(0, 12, 1, 0)], 1,
        ))
        self.assertAlmostEqual(faris_corpus.faceted_volumes_m3(data)[1], 1.0)

    def test_inward_winding_remains_a_negative_volume(self):
        inward = [[triangle[0], triangle[2], triangle[1]] for triangle in self.triangles]
        data = faris_facets.unpack_facets(faris_facets.pack_facets(
            self.vertices, inward, [(0, 12, 1, 0)], 1,
        ))
        self.assertAlmostEqual(faris_corpus.faceted_volumes_m3(data)[1], -1.0)

    def test_deviation_binding_follows_material_tags_when_volume_order_changes(self):
        parts = [("core", object()), ("blanket", object()), ("shield", object())]
        manifest = {"volumes": [
            {"index": 1, "material_tag": "mat:shield"},
            {"index": 2, "material_tag": "mat:core"},
            {"index": 3, "material_tag": "mat:blanket"},
        ]}
        self.assertEqual(faris_corpus.deviation_volume_bindings(parts, manifest), [
            {"step_index": 0, "volume": 2},
            {"step_index": 1, "volume": 3},
            {"step_index": 2, "volume": 1},
        ])

    def test_deviation_binding_rejects_missing_or_duplicate_tags(self):
        parts = [("core", object()), ("blanket", object())]
        with self.assertRaisesRegex(ValueError, "one-to-one"):
            faris_corpus.deviation_volume_bindings(parts, {"volumes": [
                {"index": 1, "material_tag": "mat:core"},
                {"index": 2, "material_tag": "mat:core"},
            ]})


class DeviationReporterContractTests(unittest.TestCase):
    def test_helper_sample_diagnostics_and_global_certifications_are_preserved(self):
        parts = [("core", object()), ("blanket", object())]

        def write_helper_report(args):
            args.report.write_text(json.dumps({"solids": [
                {"step_index": 0, "sampled_max_distance_m": 0.12, "points": 10,
                 "points_available": 10, "global_bound_status": "certified",
                 "global_upper_bound_m": 0.09},
                {"step_index": 1, "sampled_max_distance_m": 0.21, "points": 8,
                 "points_available": 10, "global_bound_status": "unresolved",
                 "global_upper_bound_m": 0.18},
            ]}))

        with TemporaryDirectory() as temp:
            out_dir = Path(temp)
            with patch("faris_cad.deviation", side_effect=write_helper_report):
                result = faris_corpus.run_deviation(
                    out_dir / "model.step", out_dir / "model.facets.bin", parts, out_dir,
                    tolerance_m=0.1,
                )

        self.assertNotIn("error", result)
        self.assertEqual(result["solids"], [
            {"solid_id": "core", "sampled_max_distance_m": 0.12,
             "global_bound_status": "certified", "global_upper_bound_m": 0.09,
             "complete": True},
            {"solid_id": "blanket", "sampled_max_distance_m": 0.21,
             "global_bound_status": "unresolved", "global_upper_bound_m": 0.18,
             "complete": False},
        ])
        with self.assertRaisesRegex(ValueError, "matching STEP"):
            faris_corpus.deviation_volume_bindings(parts, {"volumes": [
                {"index": 1, "material_tag": "mat:core"},
                {"index": 2, "material_tag": None},
            ]})


if __name__ == "__main__":
    unittest.main()
