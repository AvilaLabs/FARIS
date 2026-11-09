# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Lightweight protocol and analysis tests; no OpenMC solver is imported or run."""
from __future__ import annotations

import importlib.util
import math
from pathlib import Path
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("s0g_inboard_campaign", HERE / "s0g_inboard_campaign.py")
CAMPAIGN = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAMPAIGN)
TEST_RUNS = Path(tempfile.mkdtemp(prefix="faris-s0g-fixtures-"))
_fixture_index = 0


def plan():
    return CAMPAIGN.plan_record({"ready": True, "prerequisites": [], "model_dir": "/model", "work_dir": "/work",
                                 "stem": "rm_m", "model_hashes": {"transport_h5m": "abc", "random_ray_h5m": "def",
                                     "roles": "roles", "model_card": "card"}, "data": {},
                                 "runtime_identity": {"openmc_version": "0.16.0", "openmc_cli_version": "0.16.0",
                                     "openmc_executable_sha256": "exe-hash", "openmc_python_shared_library_sha256": "py-lib-hash",
                                     "openmc_python_shared_library": "/openmc-python/libopenmc.so",
                                     "openmc_cli_shared_library_sha256": "cli-lib-hash"}})


def run(configuration, replicate_index, *, mean=1.0, se=0.01, elapsed=10.0, high_vov=False):
    global _fixture_index
    _fixture_index += 1
    batches = CAMPAIGN.DEFAULT_BATCHES[configuration]
    if high_vov:
        values = [mean - se for _ in range(batches - 1)] + [mean + se * (batches - 1)]
    else:
        spread = se * (batches - 1) ** 0.5
        values = [mean - spread if i % 2 == 0 else mean + spread for i in range(batches)]
    stats = CAMPAIGN.RM.batch_statistics(values)
    deviations = [value - stats["mean"] for value in values]
    sum2 = math.fsum(value * value for value in deviations)
    stats["vov"] = math.fsum(value ** 4 for value in deviations) / (sum2 * sum2) - 1.0 / batches if sum2 else None
    fom = 1.0 / (stats["relative_error"] ** 2 * elapsed)
    p = plan()
    seed = p["sampling"]["seeds_by_configuration"][configuration][replicate_index]
    config_plan = p["sampling"]["configurations"][configuration]
    run_dir = TEST_RUNS / f"run-{_fixture_index}"
    run_dir.mkdir()
    model_xml_path = run_dir / "model.xml"
    model_xml_path.write_text(f"fixture model {_fixture_index}\n")
    model_xml = {"path": str(model_xml_path.resolve()), "sha256": CAMPAIGN.sha256(model_xml_path)}
    loss = {"status": "PASS", "lost_particles": 0, "histories": config_plan["histories_per_seed"],
            "fraction_limit": CAMPAIGN.R3_STATS.LOST_FRACTION_LIMIT,
            "scope": "all particle species transported in this C API run"}
    lib = {"path": p["preflight"]["runtime_identity"]["openmc_python_shared_library"],
           "sha256": p["preflight"]["runtime_identity"]["openmc_python_shared_library_sha256"]}
    integrity = {"solver_model": model_xml, "c_api_library": lib, "lost_particle_assessment": loss}
    return {"schema": CAMPAIGN.RUN_SCHEMA, "configuration": configuration, "seed": seed, "fom": fom,
            "replicate_index": replicate_index,
            "histories": config_plan["histories_per_seed"], "batches": config_plan["batches_per_seed"],
            "threads": p["sampling"]["threads"], "elapsed_seconds": elapsed, "batch_values": values,
            "stats": stats,
            "response_sha256": CAMPAIGN.canonical_hash(CAMPAIGN.RESPONSE), "model_sha256": "model-hash",
            "model_input_hashes": p["preflight"]["model_hashes"],
            "run_directory": str(run_dir.resolve()), "solver_status": "COMPLETED",
            "solver_mode": "OpenMC C API in exact selected Python environment",
            "solver_stop_reason": "batch_limit", "solver_completed_batches": config_plan["batches_per_seed"],
            "solver_terminal_status": 1,
            "solver_histories": config_plan["histories_per_seed"], "solver_model_xml": model_xml,
            "solver_model_xml_sha256": model_xml["sha256"], "solver_integrity": integrity,
            "solver_c_api_library": lib, "lost_particle_assessment": loss,
            "campaign_plan_sha256": CAMPAIGN.canonical_hash(p),
            "data_manifest_sha256": CAMPAIGN.canonical_hash(p["preflight"]["data"]),
            "source_configuration_sha256": p["source_configuration_sha256"],
            "roles_sha256": "roles-hash", "weight_windows_sha256": "windows-hash" if configuration == "windows" else None,
            "windows_record_sha256": "windows-record-hash" if configuration == "windows" else None,
            "openmc_version": "0.16.0", "openmc_shared_library_sha256": "lib-hash",
            "openmc_executable_sha256": "exe-hash", "runtime_identity": p["preflight"]["runtime_identity"]}


class CampaignProtocolTests(unittest.TestCase):
    def test_plan_refuses_existing_snapshot_before_prerequisite_inspection(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "campaign-plan.json").write_text("prior frozen plan\n")
            with mock.patch.object(CAMPAIGN, "prerequisites", side_effect=AssertionError("must not inspect")):
                with self.assertRaisesRegex(FileExistsError, "frozen campaign plan"):
                    CAMPAIGN.main(["plan", "--model-dir", "/missing", "--work-dir", str(work)])

    def test_prepare_refuses_existing_windows_directory_before_openmc_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "windows").mkdir()
            with mock.patch.object(CAMPAIGN, "_openmc_modules", side_effect=AssertionError("must not load")):
                with self.assertRaisesRegex(FileExistsError, "campaign windows directory"):
                    CAMPAIGN.prepare_windows({"ready": False}, work, 1)

    def test_run_refuses_existing_measurement_directory_before_openmc_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "runs").mkdir()
            with mock.patch.object(CAMPAIGN, "_openmc_modules", side_effect=AssertionError("must not load")):
                with self.assertRaisesRegex(FileExistsError, "campaign runs directory"):
                    CAMPAIGN.run_campaign({"status": "ready"}, work, Path("/missing"), "rm_m")

    def test_json_measurement_artifact_creation_never_replaces_existing_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "record.json"
            path.write_text("prior measurement\n")
            with self.assertRaises(FileExistsError):
                CAMPAIGN.write_json_new(path, {"replacement": True})
            self.assertEqual(path.read_text(), "prior measurement\n")

    def test_model_card_requires_explicit_true_geometry_identity(self):
        hashes = {"transport_h5m": "normal", "random_ray_h5m": "rr"}
        base = {"outputs": {"h5m_sha256": "normal", "h5m_random_ray_sha256": "rr"}}
        self.assertTrue(any("random_ray_geometry_identical" in item
                            for item in CAMPAIGN._model_card_identity_reasons(base, hashes)))
        base["outputs"]["random_ray_geometry_identical"] = False
        self.assertTrue(any("random_ray_geometry_identical" in item
                            for item in CAMPAIGN._model_card_identity_reasons(base, hashes)))
        base["outputs"]["random_ray_geometry_identical"] = True
        self.assertEqual(CAMPAIGN._model_card_identity_reasons(base, hashes), [])

    def test_model_card_hash_mismatch_fails(self):
        card = {"outputs": {"h5m_sha256": "wrong", "h5m_random_ray_sha256": "rr",
                            "random_ray_geometry_identical": True}}
        reasons = CAMPAIGN._model_card_identity_reasons(card, {"transport_h5m": "normal", "random_ray_h5m": "rr"})
        self.assertTrue(any("transport_h5m does not match" in item for item in reasons))

    def test_plan_hashes_all_local_runtime_helpers_including_dose_statistics(self):
        p = plan()
        helper = str((CAMPAIGN.HERE / "r3_dose_statistics.py").relative_to(CAMPAIGN.REPO))
        self.assertIn(helper, p["implementation_hashes"])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for source in CAMPAIGN.IMPLEMENTATION_FILES:
                relative = source.relative_to(CAMPAIGN.REPO)
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(source.read_bytes())
            CAMPAIGN.verify_implementation_hashes(p, root)
            helper_copy = root / helper
            helper_copy.write_bytes(helper_copy.read_bytes() + b"\n# changed after plan\n")
            with self.assertRaisesRegex(RuntimeError, "r3_dose_statistics.py"):
                CAMPAIGN.verify_implementation_hashes(p, root)

    def test_live_cumulative_sums_are_differenced_into_each_batch_once(self):
        batches, callback = CAMPAIGN._scalar_batch_delta_collector()

        class FakeLiveHelper:
            @staticmethod
            def invoke(on_batch):
                for batch, cumulative in enumerate((3.0, 9.0, 12.0), start=1):
                    on_batch([cumulative], batch)

        FakeLiveHelper.invoke(callback)
        self.assertEqual(batches, [3.0, 6.0, 3.0])

    def test_runtime_identity_rejects_path_override_and_version_mismatch(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            exe, py_lib, cli_lib = (Path(tmp) / name for name in ("openmc", "python-lib.so", "cli-lib.so"))
            for path in (exe, py_lib, cli_lib):
                path.write_bytes(path.name.encode())
            identity, reasons = CAMPAIGN._runtime_identity(exe, "/other/openmc", "0.16.0", py_lib, cli_lib,
                                                           "OpenMC version 0.15.3\n")
            self.assertEqual(identity["openmc_executable"], str(exe.resolve()))
            self.assertNotEqual(identity["openmc_python_shared_library_sha256"],
                                identity["openmc_cli_shared_library_sha256"])
            self.assertTrue(any("PATH resolves openmc" in item for item in reasons))
            self.assertTrue(any("CLI version" in item for item in reasons))

    def test_plan_uses_exact_box_threshold_and_reserved_generator_seeds(self):
        p = plan()
        self.assertEqual(p["response"]["energy_min_ev"], 100_000.0)
        self.assertEqual(p["response"]["region_cm"], {"x": [-130.0, 130.0], "y": [-130.0, 130.0], "z": [-150.0, 150.0]})
        self.assertEqual(p["window_generation"]["method"], "current FARIS random-ray FW-CADIS")
        self.assertTrue(p["sampling"]["estimated_campaign_fits_budget"])

    def test_plan_rejects_generator_seed_reuse(self):
        with self.assertRaisesRegex(ValueError, "reserved seeds"):
            CAMPAIGN.plan_record({"ready": True}, seeds={"analog": [CAMPAIGN.WG.MGXS_SEED, 2, 3, 4],
                                                          "windows": [5, 6, 7, 8]})

    def test_plan_rejects_cross_configuration_seed_reuse(self):
        with self.assertRaisesRegex(ValueError, "eight distinct"):
            CAMPAIGN.plan_record({"ready": True}, seeds={"analog": [1, 2, 3, 4], "windows": [1, 6, 7, 8]})

    def test_plan_rejects_nonintegral_batch_size(self):
        with self.assertRaisesRegex(ValueError, "divide evenly"):
            CAMPAIGN.plan_record({"ready": True}, histories={"analog": 101, "windows": 100},
                                 batches={"analog": 10, "windows": 10})


class CampaignAnalysisTests(unittest.TestCase):
    def analysis_rows(self, window_mean=1.01, window_se=0.0005):
        return [run(config, replicate_index,
                    mean=(window_mean if config == "windows" else 1.0),
                    se=(window_se if config == "windows" else 0.01))
                for config in ("analog", "windows") for replicate_index in range(4)]

    def test_unbiased_gain_and_lower_bound_pass_at_large_gain(self):
        p = plan()
        rows = []
        for replicate_index in range(4):
            rows.extend([run("analog", replicate_index),
                         run("windows", replicate_index, mean=1.01, se=0.0005)])
        result = CAMPAIGN.analyze_records(rows, p)
        self.assertGreater(result["pooled_fom_gain"], 100.0)
        self.assertGreater(result["gain_seed_lower97_5_one_sided_t_df3"], 100.0)
        self.assertGreater(result["gain_bootstrap_lower97_5_2_5th_percentile"], 100.0)
        self.assertTrue(result["unbiased_within_3_sigma"])
        self.assertTrue(result["nuc050_met"])

    def test_bias_fails_closed_and_unresolved_vov_is_reported(self):
        p = plan()
        rows = []
        for replicate_index in range(4):
            rows.extend([run("analog", replicate_index),
                         run("windows", replicate_index, mean=2.0, se=0.0005, high_vov=True)])
        result = CAMPAIGN.analyze_records(rows, p)
        self.assertFalse(result["unbiased_within_3_sigma"])
        self.assertFalse(result["vov_tail_resolved_diagnostic"])
        self.assertFalse(result["nuc050_met"])
        rows = []
        for replicate_index in range(4):
            rows.extend([run("analog", replicate_index),
                         run("windows", replicate_index, mean=1.01, se=0.0005, high_vov=True)])
        unresolved = CAMPAIGN.analyze_records(rows, p)
        self.assertTrue(unresolved["unbiased_within_3_sigma"])
        self.assertFalse(unresolved["nuc050_met"])
        self.assertTrue(unresolved["nuc050_outcome"].startswith("INCONCLUSIVE"))

    def test_requires_all_four_runs_for_both_configurations(self):
        p = plan()
        rows = [run(config, replicate_index)
                for config in ("analog", "windows") for replicate_index in range(3)]
        with self.assertRaisesRegex(ValueError, "exactly its four planned"):
            CAMPAIGN.analyze_records(rows, p)

    def test_response_hash_mismatch_is_rejected(self):
        p = plan()
        rows = self.analysis_rows()
        rows[0]["response_sha256"] = "not-the-plan-response"
        with self.assertRaisesRegex(ValueError, "response differs"):
            CAMPAIGN.analyze_records(rows, p)

    def test_history_count_must_match_frozen_configuration(self):
        rows = self.analysis_rows()
        rows[0]["histories"] = 30_000_000
        with self.assertRaisesRegex(ValueError, "history/batch/thread counts"):
            CAMPAIGN.analyze_records(rows, plan())

    def test_terminal_solver_status_must_be_exact_final_batch_code(self):
        for terminal in (0, 2, -1, None, "1"):
            rows = self.analysis_rows()
            if terminal is None:
                rows[0].pop("solver_terminal_status")
            else:
                rows[0]["solver_terminal_status"] = terminal
            with self.subTest(terminal=terminal), self.assertRaisesRegex(ValueError, "terminal OpenMC status"):
                CAMPAIGN.analyze_records(rows, plan())

    def test_reported_stats_and_fom_must_match_batch_recomputation(self):
        for field in ("stats", "stats.batches", "fom"):
            rows = self.analysis_rows()
            if field == "stats":
                rows[0]["stats"]["mean"] *= 1.1
            elif field == "stats.batches":
                rows[0]["stats"]["batches"] -= 1
            else:
                rows[0]["fom"] *= 1.1
            error = "stats.batches" if field == "stats.batches" else "contradicts recomputed"
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, error):
                CAMPAIGN.analyze_records(rows, plan())

    def test_unmeasured_or_excessive_losses_refuse_campaign_analysis(self):
        for loss in (
            {"status": "NOT_EVALUATED", "lost_particles": None,
             "histories": CAMPAIGN.DEFAULT_HISTORIES["analog"],
             "fraction_limit": CAMPAIGN.R3_STATS.LOST_FRACTION_LIMIT},
            {"status": "REJECTED", "lost_particles": 30,
             "histories": CAMPAIGN.DEFAULT_HISTORIES["analog"],
             "fraction_limit": CAMPAIGN.R3_STATS.LOST_FRACTION_LIMIT},
        ):
            rows = self.analysis_rows()
            rows[0]["lost_particle_assessment"] = loss
            rows[0]["solver_integrity"]["lost_particle_assessment"] = loss
            with self.subTest(loss=loss["status"]), self.assertRaisesRegex(ValueError, "lost-particle count"):
                CAMPAIGN.analyze_records(rows, plan())

    def test_solver_model_xml_must_still_match_the_recorded_identity(self):
        rows = self.analysis_rows()
        Path(rows[0]["solver_model_xml"]["path"]).write_text("changed after OpenMC run\n")
        with self.assertRaisesRegex(ValueError, "model XML hash"):
            CAMPAIGN.analyze_records(rows, plan())

    def test_nonfinite_batch_values_are_rejected(self):
        rows = self.analysis_rows()
        rows[0]["batch_values"][0] = float("nan")
        with self.assertRaisesRegex(ValueError, "one finite value"):
            CAMPAIGN.analyze_records(rows, plan())


if __name__ == "__main__":
    unittest.main()
