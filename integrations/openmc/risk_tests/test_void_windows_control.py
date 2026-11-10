# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""No-solver contracts for the bounded void-window control."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "integrations/openmc"))
sys.path.insert(0, str(ROOT / "integrations/openmc/risk_tests"))
import openmc_ci  # noqa: E402
import test_windows_unbiased as U  # noqa: E402
import void_windows_control as CONTROL  # noqa: E402
import design_transport as DT  # noqa: E402


class VoidCoverageAudit(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        openmc_ci.require_modules("openmc")
        import openmc

        cls.openmc = openmc

    def test_selected_0153_geometry_maps_every_mesh_voxel_and_lists_void_cell(self) -> None:
        self.openmc.reset_auto_ids()
        model = U.build_model(self.openmc, Path("/unused/cross_sections.xml"), U.ANALOG_RUN, U.ANALOG_SEED)
        coverage = CONTROL.audit_mesh_void_coverage(model, U.window_mesh(self.openmc))
        self.assertEqual(coverage["explicit_void_cell_ids"], [12])
        self.assertEqual(coverage["void_mesh_bins"], {"12": [60, 61, 62, 63, 64]})
        self.assertFalse(coverage["universe_implicit_complement_included"])

    def test_mesh_that_no_longer_covers_full_void_is_refused(self) -> None:
        self.openmc.reset_auto_ids()
        model = U.build_model(self.openmc, Path("/unused/cross_sections.xml"), U.ANALOG_RUN, U.ANALOG_SEED)
        mesh = U.window_mesh(self.openmc)
        mesh.dimension = (64, 1, 1)
        with self.assertRaisesRegex(ValueError, "exactly covers"):
            CONTROL.audit_mesh_void_coverage(model, mesh)

    def test_surface_shift_that_breaks_mesh_alignment_is_refused(self) -> None:
        self.openmc.reset_auto_ids()
        model = U.build_model(self.openmc, Path("/unused/cross_sections.xml"), U.ANALOG_RUN, U.ANALOG_SEED)
        model.geometry.get_all_surfaces()[3].x0 = 60.5
        with self.assertRaisesRegex(ValueError, "aligns with every slab"):
            CONTROL.audit_mesh_void_coverage(model, U.window_mesh(self.openmc))

    def test_selected_0153_accepts_no_sourcepoint_model_export(self) -> None:
        self.openmc.reset_auto_ids()
        with tempfile.TemporaryDirectory() as temporary:
            model = CONTROL.create_case(
                self.openmc, Path(temporary) / "cross_sections.xml", U.ANALOG_RUN, U.ANALOG_SEED
            )
            model.export_to_model_xml(str(Path(temporary) / "model.xml"))
            self.assertEqual(model.settings.sourcepoint["write"], False)


class WindowBoundsAudit(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        openmc_ci.require_modules("h5py", "numpy")
        import h5py
        import numpy as np

        cls.h5py = h5py
        cls.np = np

    def make_window_file(self, path: Path, *, invalid_void_bin: bool = False) -> None:
        lower = self.np.ones((2, 65), dtype=self.np.float64)
        upper = self.np.full((2, 65), 2.0, dtype=self.np.float64)
        # No lower-energy windows were generated in this deliberately inactive group.
        lower[0, :] = -1.0
        upper[0, :] = -1.0
        if invalid_void_bin:
            lower[1, 62] = -1.0
        with self.h5py.File(path, "w") as stream:
            group = stream.create_group("weight_windows/weight_windows_1")
            group.create_dataset("lower_ww_bounds", data=lower)
            group.create_dataset("upper_ww_bounds", data=upper)
            group.create_dataset("max_split", data=DT.MAX_SPLIT)

    def coverage(self):
        return {"void_mesh_bins": {"12": [60, 61, 62, 63, 64]}}

    def test_audits_every_active_energy_group_in_the_void(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "windows.h5"
            self.make_window_file(path)
            audit = CONTROL.audit_window_coverage(path, self.coverage())
        self.assertEqual(audit["active_energy_group_indexes"], [1])
        self.assertEqual(audit["unwindowed_energy_group_indexes"], [0])
        self.assertTrue(audit["void_cells"]["12"]["positive_finite_bounds_by_energy_group"])

    def test_missing_bound_inside_void_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "windows.h5"
            self.make_window_file(path, invalid_void_bin=True)
            with self.assertRaisesRegex(ValueError, "partially uncovered"):
                CONTROL.audit_window_coverage(path, self.coverage())


class ComparisonAndLossReceipt(unittest.TestCase):
    def test_seed_labels_are_distinct_without_claiming_stream_nonoverlap(self) -> None:
        plan = CONTROL.validate_seed_plan()
        self.assertTrue(plan["all_seed_labels_distinct"])
        self.assertEqual(plan["stream_nonoverlap"], "NOT_PROVEN")

    def test_predeclared_three_sigma_and_five_percent_thresholds(self) -> None:
        passed = CONTROL.evaluate_comparison(1.0, 0.02, 1.01, 0.02)
        self.assertTrue(passed["pass"])
        self.assertAlmostEqual(passed["z"], 0.3535533905932738)
        self.assertFalse(CONTROL.evaluate_comparison(1.0, 0.01, 1.2, 0.01)["pass"])
        self.assertFalse(CONTROL.evaluate_comparison(1.0, 0.06, 1.0, 0.06)["pass"])
        with self.assertRaisesRegex(ValueError, "finite and positive"):
            CONTROL.evaluate_comparison(0.0, 0.01, 1.0, 0.01)

    def test_loss_receipt_requires_exact_selected_library_and_zero_delta(self) -> None:
        library = Path("/selected/libopenmc.so")
        digest = "a" * 64
        expected = {
            "schema_version": "faris-openmc-loss-pass/v0.2",
            "method": "openmc-c-api-n_lost_particles-delta/v1",
            "library_path": str(library.resolve()),
            "library_sha256": digest,
            "start_batch": 0,
            "end_batch": 10,
            "start_lost_particle_events": 0,
            "end_lost_particle_events": 0,
            "lost_particle_events": 0,
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "loss.json"
            path.write_text(json.dumps(expected))
            self.assertEqual(
                CONTROL.validate_pass_receipt(path, library=library, library_sha256=digest,
                                              expected_batches=10),
                expected,
            )
            expected["lost_particle_events"] = 1
            path.write_text(json.dumps(expected))
            with self.assertRaisesRegex(ValueError, "exact run identity/counts"):
                CONTROL.validate_pass_receipt(path, library=library, library_sha256=digest,
                                              expected_batches=10)


if __name__ == "__main__":
    unittest.main()
