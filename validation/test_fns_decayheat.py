#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""FNS-96 Fe source intake, time/unit mapping, and sealed scoring regressions."""
from __future__ import annotations

import copy
import importlib.util
import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASE_DIR = ROOT / "validation" / "cases" / "fns-decayheat-fe-1996-5min"
sys.path.insert(0, str(ROOT))


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


BM = load_module("fns_decayheat_build_manifest", CASE_DIR / "build_manifest.py")
RC = load_module("fns_decayheat_run_case", CASE_DIR / "run_case.py")
MANIFEST = json.loads((CASE_DIR / "manifest.json").read_text(encoding="utf-8"))


def sample_result(manifest: dict = MANIFEST) -> dict:
    steps = [{"step": 1, "t_s": 300.0, "flux": 1.0, "heat_W_per_g": {"total": 0.0, "alpha": 0.0, "beta": 0.0, "gamma": 0.0}}]
    for i, detector in enumerate(manifest["detectors"], start=2):
        t_s = 300.0 + detector["controlling_variable"]["value"]
        steps.append({"step": i, "t_s": t_s, "flux": 0.0,
                      "heat_W_per_g": {"total": 6e-7, "alpha": 1e-7, "beta": 2e-7, "gamma": 3e-7}})
    inputs = {name: {"path": f"/data/{name}", "sha256": f"{index:064x}"}
              for index, name in enumerate(RC.LIBRARY_COMPONENTS, start=1)}
    return {"entry_point": "cli", "mode": "trace", "steps": steps,
            "certificate": {"solver": "actinv-core 1.1.2", "mode": "trace", "prune": "rate", "inputs": inputs}}


class FnsDecayHeatCase(unittest.TestCase):
    def test_case_manifest_valid_and_preserves_all_reference_points(self):
        from validation.manifest import check_manifest

        checked = check_manifest(MANIFEST)
        self.assertEqual(checked["case_id"], "fns-decayheat-fe-1996-5min")
        self.assertEqual(len(checked["detectors"]), 20)
        self.assertEqual(checked["evidence_class"], "experiment")
        self.assertEqual(checked["compatibility"]["covariance"]["kind"], "unknown")
        self.assertEqual(checked["source"]["original_archive_sha256"], BM.ARCHIVE_SHA256)

    def test_archive_member_identities_are_checked_against_pinned_source_note(self):
        source_members = dict(BM.ARCHIVE_MEMBERS)
        source_members["fns/Fe/total_1996exp_5min.pdf"] = "d6ea06aadbf0fc9c2ad91452b32e8e0e64afe692c5624d07ccd03fb1673bb876"
        note = "\n".join(f"| `{Path(name).name}` | `{digest}` |" for name, digest in source_members.items()).encode()
        RC.verify_archive_members(MANIFEST, note)
        bad = copy.deepcopy(MANIFEST)
        bad["source"]["original_archive_members"]["fns/Fe/total_1996exp_5min.pdf"] = "d6ea06aadbf0fc9c2ad91461054aec7d415ef19d946aba8b9886b31de218252d"
        with self.assertRaisesRegex(ValueError, "archive member hashes differ"):
            RC.verify_archive_members(bad, note)
        with self.assertRaisesRegex(ValueError, "four original"):
            RC.verify_archive_members(MANIFEST, b"no archive member table")

    def test_reported_relative_error_is_converted_to_absolute_without_confidence_claim(self):
        lines = ["time,Value,Error"]
        for detector in MANIFEST["detectors"]:
            lines.append(f"{detector['controlling_variable']['value']},{detector['reference']['value']},"
                         f"{detector['reported_error_relative']}")
        parsed = BM.parse_measurements(("\n".join(lines) + "\n").encode())
        self.assertTrue(all("confidence" in item["meaning"] for item in MANIFEST["uncertainty"]["components"]
                            if item["name"] == "reported experimental error magnitude"))
        self.assertEqual(parsed[0]["reported_heat_microW_per_g"], 0.128)
        self.assertAlmostEqual(parsed[0]["reported_error_microW_per_g"], 0.128 * 0.05)
        self.assertEqual(parsed[0]["reported_error_relative"], 0.05)

    def test_measurement_intake_rejects_bad_header_nonfinite_and_missing_points(self):
        for raw, message in [
            (b"time,Value,Error\n66,0.1,0.05\n", "expected all 20"),
            (b"time,Value,wrong\n66,0.1,0.05\n", "header must be exactly"),
            (b"time,Value,Error\n66,nan,0.05\n", "finite and positive"),
        ]:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                BM.parse_measurements(raw)

    def test_pinned_problem_accepts_declared_total_scaling_without_mutating_group_shape(self):
        measurements = [dict(cooling_time_s=float(d["controlling_variable"]["value"])) for d in MANIFEST["detectors"]]
        cumulative = 0.0
        schedule = [{"dt": "300 s", "flux": 1.0}]
        for row in measurements:
            next_time = row["cooling_time_s"]
            schedule.append({"dt": f"{next_time - cumulative:g} s", "flux": 0.0})
            cumulative = next_time
        groups = [0.0] * 709
        groups[300] = 11_166_502_554.2
        spec = {"spec": "actinv-spec-1", "projectile": "neutron",
                "library": {"path": "catalog:tendl-2025-neutron-709g"},
                "decay": {"primary": "catalog:endfb-viii-0-decay", "fallback": "catalog:jeff-3-3-decay"},
                "material": {"mass_g": 1.0, "basis": "wt_percent", "composition": {"FE": 100.0}},
                "spectrum": {"structure": "fispact-709", "descending": True, "total": 1.116e10, "flux_per_group": groups},
                "schedule": schedule}
        source_groups = list(spec["spectrum"]["flux_per_group"])
        BM.validate_problem(spec, measurements)
        self.assertNotEqual(math.fsum(source_groups), spec["spectrum"]["total"])
        self.assertEqual(spec["spectrum"]["flux_per_group"], source_groups)

    def test_actinv_output_maps_exact_cooling_endpoints_and_converts_w_per_g(self):
        result = sample_result()
        rows = RC.parse_result(result, MANIFEST)
        self.assertEqual(len(rows), 20)
        self.assertEqual(rows[0]["time_s"], 366.0)
        self.assertEqual(rows[0]["value"], 0.6)
        self.assertEqual(rows[0]["unit"], "microW/g")
        self.assertEqual(rows[0]["u_mc"], 0.0)

    def test_actinv_mapping_rejects_time_shift_flux_or_broken_heat_closure(self):
        bad = sample_result()
        bad["steps"][1]["t_s"] += 10.0
        with self.assertRaisesRegex(ValueError, r"does not match shutdown\+cooling"):
            RC.parse_result(bad, MANIFEST)

        bad = sample_result()
        bad["steps"][1]["flux"] = 1.0
        with self.assertRaisesRegex(ValueError, "nonzero flux"):
            RC.parse_result(bad, MANIFEST)

        bad = sample_result()
        bad["steps"][1]["heat_W_per_g"]["total"] = 7e-7
        with self.assertRaisesRegex(ValueError, "does not close"):
            RC.parse_result(bad, MANIFEST)

    def test_nonfinite_actinv_values_are_rejected_before_sealing(self):
        bad = sample_result()
        bad["steps"][1]["heat_W_per_g"]["total"] = math.inf
        with self.assertRaisesRegex(ValueError, "finite and non-negative"):
            RC.parse_result(bad, MANIFEST)

    def test_sealed_record_flows_through_shared_scorer_and_stays_inconclusive(self):
        from validation.scoring import Identity, INCONCLUSIVE, check_run_record, score_case

        result = sample_result()
        mapped = RC.parse_result(result, MANIFEST)
        raw = json.dumps(result, sort_keys=True, allow_nan=False).encode()
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "actinv"
            binary.write_bytes(b"synthetic executable identity for this unit test")
            record = RC.make_run_record("synthetic-test-only", MANIFEST, b"synthetic spec", raw, result, binary, mapped)
        identity = Identity.from_dict(record["identity"])
        self.assertEqual(check_run_record(record, RC.CASE_ID), identity)
        report = score_case(MANIFEST, record, identity)
        self.assertEqual(len(report["rows"]), 20)
        self.assertTrue(all(row["verdict"] == INCONCLUSIVE for row in report["rows"]))
        self.assertTrue(all(row["ce"] is not None for row in report["rows"]))
        self.assertTrue(all("covariance" in row["why"] for row in report["rows"]))

    def test_library_identity_does_not_depend_on_local_install_paths(self):
        result = sample_result()
        mapped = RC.parse_result(result, MANIFEST)
        raw = json.dumps(result, sort_keys=True).encode()
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "actinv"
            binary.write_bytes(b"test binary")
            first = RC.make_run_record("first", MANIFEST, b"spec", raw, result, binary, mapped)
            for item in result["certificate"]["inputs"].values():
                item["path"] = "/another/installation/with/different/path"
            second = RC.make_run_record("second", MANIFEST, b"spec", raw, result, binary, mapped)
        self.assertEqual(first["identity"]["library_sha256"], second["identity"]["library_sha256"])

    def test_command_help_runs_from_repo_root_by_script_path(self):
        for script in ("build_manifest.py", "run_case.py"):
            path = CASE_DIR / script
            result = subprocess.run([sys.executable, str(path), "--help"], capture_output=True, text=True, check=True)
            self.assertIn("usage:", result.stdout.lower())


if __name__ == "__main__":
    unittest.main()
