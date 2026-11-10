#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure tests for the scoped default-tolerance corpus qualifier."""
from __future__ import annotations

import unittest

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
        with self.assertRaisesRegex(ValueError, "matching STEP"):
            faris_corpus.deviation_volume_bindings(parts, {"volumes": [
                {"index": 1, "material_tag": "mat:core"},
                {"index": 2, "material_tag": None},
            ]})


if __name__ == "__main__":
    unittest.main()
