# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure tests for pinned JADE response intake; no network or solver required."""
from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from validation import scoring as S  # noqa: E402
from validation.fixtures import IDENT, run_record  # noqa: E402
BUILDER = ROOT / "validation" / "cases" / "iter-1d-flux-jade-v1" / "build_manifest.py"
SPEC = importlib.util.spec_from_file_location("jade_flux_manifest", BUILDER)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(MODULE)


def sample_csv(relative_error: str = "0.1") -> bytes:
    lines = ["Cells,Value,Error"]
    lines.extend(f"{i},{i * 2}.0,{relative_error}" for i in range(1, 98))
    return ("\n".join(lines) + "\n").encode()


def sample_map() -> bytes:
    lines = ["_cell_id_x: &radii"]
    lines.extend(f"  {i}: {i}.0" for i in range(1, 105))
    lines.append("other_setting: true")
    return ("\n".join(lines) + "\n").encode()


def sample_deck() -> bytes:
    lines = ["F204:N"]
    lines.extend("     " + " ".join(str(i) for i in range(a, min(a + 10, 98)))
                  for a in range(1, 98, 10))
    lines += ["FM204 1.222e21"]
    return ("\n".join(lines) + "\n").encode()


def sample_geometry() -> bytes:
    from xml.etree.ElementTree import Element, SubElement, tostring
    root = Element("geometry")
    for i in range(1, 105):
        SubElement(root, "surface", {"id": str(i), "type": "z-cylinder",
                                      "coeffs": f"0.0 0.0 {i}.0"})
        SubElement(root, "cell", {"id": str(i), "region": f"-1000 {i - 1} -{i}"})
    return tostring(root)


class JadeFluxIntake(unittest.TestCase):
    def test_pinned_dataset_build_is_97_radial_responses_and_keeps_covariance_unknown(self):
        data = sample_csv()
        rows = MODULE.parse_table(data)
        self.assertEqual(len(rows), 97)
        self.assertEqual(rows[0], {"position_cm": 1.0, "value": 2.0, "u": 0.2, "relative_standard_error": 0.1})
        mapped = MODULE.map_rows_to_cells(rows, MODULE.parse_cell_radius_map(sample_map()),
                                          MODULE.parse_tally_cells_and_scale(sample_deck())[0])
        MODULE.verify_openmc_mapping(mapped, MODULE.parse_openmc_outer_radii(sample_geometry()))
        manifest = MODULE.manifest_from_rows(mapped, "2026-11-09")
        self.assertEqual(manifest["case_id"], "iter-1d-flux-jade-v1")
        self.assertEqual(len(manifest["detectors"]), 97)
        self.assertEqual(manifest["compatibility"]["covariance"]["kind"], "unknown")
        self.assertEqual(manifest["license"]["spdx"], "EUPL-1.2")
        self.assertTrue(manifest["files_are_external_not_vendored"])
        self.assertEqual(manifest["normalisation"]["status"], "confirmed")
        self.assertFalse(manifest["normalisation"]["blocks_scoring"])
        self.assertEqual(manifest["detectors"][0]["id"], "neutron_flux:cell-1")
        self.assertEqual(manifest["detectors"][0]["reference"], {"value": 2 / MODULE.FM_SCALE, "u": 0.2 / MODULE.FM_SCALE})
        self.assertEqual(manifest["detectors"][0]["controlling_variable"]["name"],
                         "outer radial boundary of tallied cell")

    def test_mapping_requires_unique_coordinate_and_exact_tally_coverage(self):
        rows = MODULE.parse_table(sample_csv())
        mapping = MODULE.parse_cell_radius_map(sample_map())
        tally_ids, scale = MODULE.parse_tally_cells_and_scale(sample_deck())
        self.assertEqual(scale, MODULE.FM_SCALE)
        self.assertEqual(len(MODULE.map_rows_to_cells(rows, mapping, tally_ids)), 97)
        with self.assertRaisesRegex(ValueError, "unmapped"):
            MODULE.map_rows_to_cells(rows, {k: v for k, v in mapping.items() if k != 1.0}, tally_ids)
        with self.assertRaisesRegex(ValueError, "absent from F204"):
            MODULE.map_rows_to_cells(rows, mapping, set(range(1, 97)) | {999})
        radii = MODULE.parse_openmc_outer_radii(sample_geometry())
        MODULE.verify_openmc_mapping(MODULE.map_rows_to_cells(rows, mapping, tally_ids), radii)
        radii[1] = 0.5
        with self.assertRaisesRegex(ValueError, "differs from JADE"):
            MODULE.verify_openmc_mapping(MODULE.map_rows_to_cells(rows, mapping, tally_ids), radii)

    def test_malformed_map_and_tally_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "104"):
            MODULE.parse_cell_radius_map(b"_cell_id_x: &radii\n  1: 1.0\n")
        with self.assertRaisesRegex(ValueError, "97 unique"):
            MODULE.parse_tally_cells_and_scale(b"F204:N 1 2\nFM204 1.222e21\n")

    def test_duplicate_or_nonfinite_coordinates_are_rejected(self):
        duplicate = b"Cells,Value,Error\n" + b"1,2,0.1\n" * 97
        with self.assertRaisesRegex(ValueError, "unique"):
            MODULE.parse_table(duplicate)
        rows = ["Cells,Value,Error"] + [f"{i},{i * 2}.0,0.1" for i in range(1, 97)] + ["nan,2,0.1"]
        with self.assertRaisesRegex(ValueError, "finite"):
            MODULE.parse_table(("\n".join(rows) + "\n").encode())

    def test_malformed_or_negative_uncertainty_is_rejected(self):
        rows = ["Cells,Value,Error"] + [f"{i},{i * 2}.0,0.1" for i in range(1, 97)] + ["97,194,-0.1"]
        with self.assertRaisesRegex(ValueError, "non-negative"):
            MODULE.parse_table(("\n".join(rows) + "\n").encode())

    def test_wrong_hash_and_wrong_byte_count_are_rejected_before_manifest_creation(self):
        with self.assertRaisesRegex(ValueError, "pinned JADE artifact"):
            MODULE.build_manifest(sample_csv(), "2026-11-09")

    def test_script_help_runs_by_path_from_repository_root(self):
        result = subprocess.run([sys.executable, str(BUILDER), "--help"], cwd=ROOT,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--fetch", result.stdout)

    def test_fetch_stops_after_small_pinned_size_limit(self):
        class Response:
            headers = {}
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return False
            def read(self, limit):
                self.limit = limit
                return b"12345"

        response = Response()
        with patch.object(MODULE.urllib.request, "urlopen", return_value=response):
            with self.assertRaisesRegex(ValueError, "download limit"):
                MODULE.fetch("https://example.invalid", "0" * 64, Path("/tmp/unused-jade-data"), 4)
        self.assertEqual(response.limit, 5)

    def test_source_gap_blocks_every_response_even_with_confirmed_normalisation_and_independent_covariance(self):
        rows = MODULE.parse_table(sample_csv())
        mapped = MODULE.map_rows_to_cells(rows, MODULE.parse_cell_radius_map(sample_map()),
                                          MODULE.parse_tally_cells_and_scale(sample_deck())[0])
        case = MODULE.manifest_from_rows(mapped, "2026-11-09")
        case["normalisation"]["status"] = "confirmed"
        case["compatibility"]["covariance"] = {"kind": "independent", "basis": "adversarial regression"}
        record = run_record(case, [10.0] * len(case["detectors"]))
        report = S.score_case(case, record, IDENT)
        self.assertEqual(len(report["rows"]), 97)
        for row in report["rows"]:
            self.assertEqual(row["verdict"], S.NOT_EVALUATED)
            self.assertIsNone(row["ce"])
            self.assertIn("not been established", row["why"])
            self.assertTrue(row["next_step"])


if __name__ == "__main__":
    unittest.main()
