# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Tests for external JADE TBM candidate intake; no network or solver is used."""
from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
BUILDER = ROOT / "validation/cases/iter-tbm-jade-v1/build_manifest.py"
SPEC = importlib.util.spec_from_file_location("jade_tbm_manifest", BUILDER)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(MODULE)


def table_bytes(zero_row: int | None = None) -> bytes:
    rows = ["Cor A,Value,Error"]
    for index in range(MODULE.N_ROWS):
        coordinate = 830.0 + (1084.2 - 830.0) * index / 78
        if index == 78:
            coordinate = 1084.2
        value, error = ("0", "0") if index == zero_row else (str(index + 1), "0.1")
        rows.append(f"{coordinate:.10g},{value},{error}")
    return ("\n".join(rows) + "\n").encode()


def tables() -> dict[str, list[dict]]:
    return {name: MODULE.parse_table(table_bytes(4 if name == "neutron_flux" else None), name)
            for name in MODULE.RESPONSES}


class JadeTbmIntake(unittest.TestCase):
    def test_registration_manifests_preserve_79_slots_per_response_without_values(self):
        for case, slug in (("HCPB_TBM_1D", "hcpb"), ("WCLL_TBM_1D", "wcll")):
            with self.subTest(case=case):
                path = ROOT / f"validation/cases/{slug}-tbm-jade-v1/manifest.json"
                manifest = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual(len(manifest["detectors"]), 5 * 79)
                for response in MODULE.RESPONSES:
                    selected = [d for d in manifest["detectors"] if d["response_class"] == response]
                    self.assertEqual(len(selected), 79)
                    self.assertTrue(all(d["reference"] is None and d["blocked"] for d in selected))
                    self.assertTrue(all(d["candidate_reference"]["value"] is None for d in selected))
                self.assertEqual(manifest["license"]["spdx"], "EUPL-1.2")
                self.assertEqual(manifest["nuclear_data_identity"]["eligibility"].split(":", 1)[0],
                                 "NOT_EVALUATED")

    def test_candidate_keeps_all_bins_responses_and_unknown_zero_precision(self):
        candidate = MODULE.manifest_from_rows("HCPB_TBM_1D", tables(), "2026-11-09")
        self.assertEqual(len(candidate["detectors"]), 395)
        self.assertEqual(candidate["files_are_external_not_vendored"], True)
        self.assertEqual(candidate["license"]["spdx"], "EUPL-1.2")
        for response in MODULE.RESPONSES:
            selected = [d for d in candidate["detectors"] if d["response_class"] == response]
            self.assertEqual([d["candidate_reference"]["mesh_index"] for d in selected], list(range(79)))
            self.assertTrue(all(d["reference"] is None and d["blocked"] for d in selected))
        zero = next(d for d in candidate["detectors"] if d["id"] == "neutron_flux:mesh-004")
        self.assertEqual(zero["candidate_reference"]["value"], 0)
        self.assertEqual(zero["candidate_reference"]["precision_status"], "unknown_zero_score")
        self.assertIsNone(zero["candidate_reference"]["standard_error"])
        nonzero = next(d for d in candidate["detectors"] if d["id"] == "neutron_flux:mesh-005")
        self.assertAlmostEqual(nonzero["candidate_reference"]["standard_error"], 0.6)

    def test_response_coordinate_mismatch_is_fail_closed(self):
        raw = table_bytes()
        parsed = {name: MODULE.parse_table(raw, name) for name in MODULE.RESPONSES}
        parsed["photon_flux"][10]["cor_a_cm"] += 0.01
        with self.assertRaisesRegex(ValueError, "coordinates do not exactly match"):
            MODULE.manifest_from_rows("HCPB_TBM_1D", parsed, "2026-11-09")

    def test_metadata_and_csv_are_bound_to_exact_pinned_sha256_not_git_blob_ids(self):
        for case, spec in MODULE.CASE_SPECS.items():
            self.assertEqual(len(spec["metadata_sha256"]), 64)
            for digest, _size in spec["csv"].values():
                self.assertEqual(len(digest), 64)
        with self.assertRaisesRegex(ValueError, "pinned upstream bytes"):
            MODULE.verify_bytes(table_bytes(), "0" * 64, len(table_bytes()), "mutated candidate")

    def test_mcnp_deck_card_mapping_is_complete_and_per_response(self):
        upstream = Path("/home/connoravila/Documents/Avila-Labs/development/iaea-open-benchmarks")
        if not upstream.exists():
            self.skipTest("pinned upstream checkout not present on this host")
        for case in MODULE.CASE_SPECS:
            with self.subTest(case=case):
                bundle = MODULE.verify_upstream_inputs(upstream, case)
                cards = bundle["files"]["mcnp_deck"]["checks"]["tallies"]
                self.assertEqual(cards, MODULE.CASE_SPECS[case]["tallies"])


if __name__ == "__main__":
    unittest.main()
