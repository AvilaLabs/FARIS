# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure CAD history-response contract and covariance tests; no OpenMC or solver."""
from __future__ import annotations

import copy
import hashlib
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import design_transport as worker


class CadHistoryResponseTests(unittest.TestCase):
    def setUp(self):
        self.solids = [
            {"solid_id": "blanket", "material_id": "li6"},
            {"solid_id": "wall", "material_id": "steel"},
            {"solid_id": "coil", "material_id": "copper"},
        ]
        self.response = {
            "components": [
                {"component_id": "blanket", "solid_ids": ["blanket"], "volume_m3": 0.2},
                {"component_id": "first-wall", "solid_ids": ["wall"], "volume_m3": 0.1},
                {"component_id": "tf-coil", "solid_ids": ["coil"], "volume_m3": 0.3},
            ],
            "breeder_component_id": "blanket",
            "h3_solid_ids": ["blanket"],
            "h3_total_scope": "whole_model",
            "flux_energy_min_ev": 0.0,
        }

    def test_exact_partition_and_breeder_binding(self):
        self.assertEqual(worker.validate_history_response(self.response, self.solids), self.response)
        changed = copy.deepcopy(self.response)
        changed["components"][1]["solid_ids"] = ["coil"]
        with self.assertRaisesRegex(ValueError, "assigned more than once"):
            worker.validate_history_response(changed, self.solids)
        changed = copy.deepcopy(self.response)
        changed["h3_solid_ids"] = ["wall"]
        with self.assertRaisesRegex(ValueError, "complete breeder"):
            worker.validate_history_response(changed, self.solids)

    def test_threshold_is_exactly_all_energy(self):
        changed = copy.deepcopy(self.response)
        changed["flux_energy_min_ev"] = 100_000.0
        with self.assertRaisesRegex(ValueError, "all-energy"):
            worker.validate_history_response(changed, self.solids)

    def test_total_and_breeder_tritium_responses_remain_distinct(self):
        total, breeder = 5.0, 3.0
        worker.validate_h3_domain_closure(total, breeder)
        self.assertNotEqual(total, breeder)
        with self.assertRaisesRegex(ValueError, "exceeds whole-model"):
            worker.validate_h3_domain_closure(2.0, 3.0)

    def test_batch_covariance_retains_shared_sampling(self):
        stats = worker.BatchCovarianceMoments(2)
        stats.add([1.0, 2.0])
        stats.add([3.0, 6.0])
        covariance = stats.covariance_of_mean
        self.assertEqual(stats.count, 2)
        self.assertAlmostEqual(float(covariance[0, 0]), 1.0)
        self.assertAlmostEqual(float(covariance[0, 1]), 2.0)
        self.assertAlmostEqual(float(covariance[1, 1]), 4.0)
        self.assertFalse(stats.target_reached(0.49, 2))
        self.assertTrue(stats.target_reached(0.5, 2))

    def test_bounded_count_and_invalid_samples_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "between 2 and 256"):
            worker.BatchCovarianceMoments(257)
        stats = worker.BatchCovarianceMoments(2)
        with self.assertRaisesRegex(ValueError, "finite"):
            stats.add([1.0, float("nan")])

    def test_referenced_xs_table_identity_is_bounded_and_ignores_unused_catalogue(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            table = root / "Li6.h5"
            table.write_bytes(b"neutron-data-v1")
            xml = root / "cross_sections.xml"
            xml.write_text(
                '<cross_sections><library type="neutron" materials="Li6" path="Li6.h5" />'
                '<library type="neutron" materials="Unused" path="missing-unused.h5" /></cross_sections>'
            )
            request = {
                "cross_sections": str(xml),
                "materials": [{"nuclide_atom_fractions": {"Li6": 1.0}}],
            }
            first = worker.history_nuclear_data_identity(request)
            self.assertEqual(first["schema"], "faris-cad-history-neutron-data/v0.1")
            self.assertIn("photon transport and thermal-scattering tables disabled", first["scope"])
            self.assertEqual(first["tables"][0]["nuclides"], ["Li6"])
            self.assertEqual(first["tables"][0]["library_indices"], [0])
            self.assertEqual(first["tables"][0]["bytes"], len(b"neutron-data-v1"))
            self.assertEqual(first["tables"][0]["sha256"], hashlib.sha256(b"neutron-data-v1").hexdigest())
            table.write_bytes(b"neutron-data-v2")
            changed = worker.history_nuclear_data_identity(request)
            self.assertNotEqual(first, changed)

    def test_missing_or_duplicate_referenced_xs_mapping_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Li6.h5").write_bytes(b"data")
            xml = root / "cross_sections.xml"
            request = {"cross_sections": str(xml), "materials": [{"nuclide_atom_fractions": {"Li6": 1.0}}]}
            xml.write_text('<cross_sections><library type="neutron" materials="Other" path="Li6.h5" /></cross_sections>')
            with self.assertRaisesRegex(ValueError, "no neutron table"):
                worker.history_nuclear_data_identity(request)
            xml.write_text('<cross_sections><library type="neutron" materials="Li6" path="Li6.h5" />'
                           '<library type="neutron" materials="Li6" path="Li6.h5" /></cross_sections>')
            with self.assertRaisesRegex(ValueError, "more than one"):
                worker.history_nuclear_data_identity(request)


if __name__ == "__main__":
    unittest.main()
