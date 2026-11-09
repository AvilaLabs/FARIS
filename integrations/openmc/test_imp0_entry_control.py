#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure receipt/completeness checks for the optional OpenMC imp:n=0 control."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "integrations/openmc/risk_tests"))
import imp0_entry_control as CONTROL  # noqa: E402


class StatePointCompleteness(unittest.TestCase):
    def test_one_thousand_particles_are_one_tally_realization(self) -> None:
        CONTROL.validate_statepoint_metadata(
            current_batch=1,
            n_batches=1,
            n_particles=1000,
            n_realizations=1,
            version=(0, 15, 3),
        )

    def test_rejects_particle_count_misread_as_realizations(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "n_realizations=1000"):
            CONTROL.validate_statepoint_metadata(
                current_batch=1,
                n_batches=1,
                n_particles=1000,
                n_realizations=1000,
                version=(0, 15, 3),
            )

    def test_rejects_incomplete_or_wrong_version_statepoint(self) -> None:
        for kwargs in (
            {"current_batch": 0, "n_batches": 1, "n_particles": 1000,
             "n_realizations": 1, "version": (0, 15, 3)},
            {"current_batch": 1, "n_batches": 1, "n_particles": 999,
             "n_realizations": 1, "version": (0, 15, 3)},
            {"current_batch": 1, "n_batches": 1, "n_particles": 1000,
             "n_realizations": 1, "version": (0, 16, 0)},
        ):
            with self.subTest(kwargs=kwargs), self.assertRaises(RuntimeError):
                CONTROL.validate_statepoint_metadata(**kwargs)


class SealedReceipt(unittest.TestCase):
    def test_canonical_digest_independent_of_key_order(self) -> None:
        report = {"schema": "faris.openmc-mcnp-imp0-control/1.0.0", "status": "PASS",
                  "cases": [{"statepoint_n_realizations": 1, "statepoint_n_particles": 1000}]}
        digest = CONTROL.canonical_receipt_digest(report)
        reordered = {"cases": report["cases"], "status": report["status"], "schema": report["schema"]}
        reordered["receipt_sha256"] = digest
        self.assertTrue(CONTROL.verify_receipt_digest(reordered))
        reordered["status"] = "FAIL"
        self.assertFalse(CONTROL.verify_receipt_digest(reordered))

    def test_nonfinite_receipt_values_are_refused(self) -> None:
        with self.assertRaises(ValueError):
            CONTROL.canonical_receipt_digest({"value": float("nan")})


if __name__ == "__main__":
    unittest.main()
