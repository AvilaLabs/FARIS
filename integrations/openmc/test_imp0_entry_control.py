#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure receipt/completeness checks for the optional OpenMC imp:n=0 control."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

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


class ExplicitCliLibraryBinding(unittest.TestCase):
    def test_requires_exact_ldd_binding(self) -> None:
        executable = Path(sys.executable).resolve(strict=True)
        library = Path(__file__).resolve(strict=True)
        bindings = []
        for address in ("0x1234", "0xabcd"):
            row = f"\tlibopenmc.so => {library} ({address})"
            completed = subprocess.CompletedProcess(["ldd"], 0, row + "\n", "")
            with patch.object(CONTROL.subprocess, "run", return_value=completed) as mocked:
                bindings.append(CONTROL.verify_cli_library_binding(executable, library, {}))
            self.assertEqual(mocked.call_args.args[0], ["ldd", str(executable)])
        self.assertEqual(bindings, [f"libopenmc.so => {library}"] * 2)

    def test_refuses_other_or_unresolved_cli_library(self) -> None:
        executable = Path(sys.executable).resolve(strict=True)
        library = Path(__file__).resolve(strict=True)
        for output in (
            "libopenmc.so => /other/libopenmc.so (0x1234)\n",
            "libopenmc.so => not found\n",
            "libopenmc.so => /one.so (0x1234)\nlibopenmc.so => /two.so (0x5678)\n",
        ):
            completed = subprocess.CompletedProcess(["ldd"], 0, output, "")
            with patch.object(CONTROL.subprocess, "run", return_value=completed):
                with self.subTest(output=output), self.assertRaises(RuntimeError):
                    CONTROL.verify_cli_library_binding(executable, library, {})


class ActualOpenMCApiModelExport(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        try:
            import openmc
        except ImportError as error:
            raise unittest.SkipTest(f"OpenMC is unavailable: {error}")
        if openmc.__version__ != "0.15.3":
            raise unittest.SkipTest(f"requires OpenMC 0.15.3, found {openmc.__version__}")
        cls.openmc = openmc

    def test_all_four_control_models_export_through_openmc_0153_api(self) -> None:
        cases = (
            ("entry_neutron", "neutron", False, True),
            ("source_birth_neutron", "neutron", True, True),
            ("marked_photon", "photon", False, True),
            ("unmarked_neutron", "neutron", False, False),
        )
        with tempfile.TemporaryDirectory(prefix="faris-imp0-model-export-") as temporary:
            root = Path(temporary)
            for name, particle, source_in_zero_cell, marked in cases:
                with self.subTest(case=name):
                    self.openmc.reset_auto_ids()
                    directory = root / name
                    directory.mkdir()
                    geometry_xml = CONTROL.make_model(
                        self.openmc,
                        directory,
                        particle=particle,
                        source_in_zero_cell=source_in_zero_cell,
                        marked=marked,
                    )
                    cells = ET.parse(geometry_xml).getroot().findall("cell")
                    marked_ids = [
                        cell.get("id") for cell in cells
                        if cell.get("faris_neutron_importance") == "0"
                    ]
                    self.assertEqual(marked_ids, ["3"] if marked else [])
                    settings = ET.parse(directory / "settings.xml").getroot()
                    self.assertEqual(settings.findtext("max_write_lost_particles"), "1")


if __name__ == "__main__":
    unittest.main()
