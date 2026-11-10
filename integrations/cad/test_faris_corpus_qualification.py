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


if __name__ == "__main__":
    unittest.main()
