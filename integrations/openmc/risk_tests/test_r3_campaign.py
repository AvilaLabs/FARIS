# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Lightweight contract tests for the serial R3 campaign collector (no OpenMC)."""
from __future__ import annotations

import json
import io
import contextlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

import r3_campaign as campaign


class R3CampaignTests(unittest.TestCase):
    def fixture(self, root: Path):
        model = root / "model"
        model.mkdir()
        for name, content in (("rm_m.h5m", b"h5m"), ("rm_m_roles.json", b"{}"),
                              ("rm_m_model_card.json", b"{}")):
            (model / name).write_bytes(content)
        xs = root / "cross_sections.xml"
        libraries = root / "library-data"
        libraries.mkdir()
        for name, payload in (("neutron.h5", b"synthetic neutron bytes"),
                              ("thermal.h5", b"synthetic thermal bytes"),
                              ("photon.h5", b"synthetic photon bytes")):
            (libraries / name).write_bytes(payload)
        xs.write_text("<cross_sections><directory>library-data</directory>"
                      "<library type='neutron' path='neutron.h5'/>"
                      "<library type='thermal' path='thermal.h5'/>"
                      "<library type='photon' path='photon.h5'/>"
                      "</cross_sections>\n", encoding="utf-8")
        chain = root / "chain.xml"
        chain.write_text("<depletion_chain><nuclide name='Fe56'/></depletion_chain>\n", encoding="utf-8")
        python = root / "python"
        python.write_text("", encoding="utf-8")
        python.chmod(0o755)
        return model, xs, python, chain

    def plan(self, root: Path, *, replicates=2, seed_base=50000, out=None,
             activation_groups=campaign.sc.ACTIVATION_GROUPS):
        model, xs, python, chain = self.fixture(root)
        with mock.patch.object(campaign, "CHAIN_PATH", chain):
            return campaign.make_plan(model_dir=model, cross_sections=xs, python=python,
                                      out_dir=out or root / "campaign", replicates=replicates,
                                      seed_base=seed_base, purpose="conditional-method-control",
                                      activation_groups=activation_groups)

    def fake_complete(self, command, **kwargs):
        """Write minimal hashed task receipts matching the campaign completion contract."""
        output = kwargs["task_output"]
        log = kwargs["log_path"]
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text("mock task completed\n", encoding="utf-8")
        script = Path(command[1]).name
        if script == "r3_d1s.py":
            output.mkdir(parents=True)
            artifact = output / "basis.bin"
            artifact.write_bytes(b"d1s basis")
            for time_index in campaign.sc.PHOTON_TIME_INDICES:
                (output / f"dose_time_{time_index}.npz").write_bytes(f"dose-{time_index}".encode())
            seed = int(command[command.index("--seed") + 1])
            receipt = {"kind": "r3-d1s", "seed": seed,
                       "run": {"stop_reason": "batch_limit", "integrity": {
                           "lost_particle_assessment": {"status": "PASS"}}},
                       "shutdown_dose": {label: {} for label in campaign.sc.COOLING_LABELS.values()},
                       "shutdown_dose_basis": {"geometry": {"path": "basis.bin",
                                                               "sha256": campaign.sha256(artifact)}}}
            (output / "d1s_record.json").write_text(json.dumps(receipt), encoding="utf-8")
        elif script == "r3_r2s.py":
            output.mkdir(parents=True)
            artifact = output / "microxs.bin"
            artifact.write_bytes(b"bounded microxs")
            for time_index in campaign.sc.PHOTON_TIME_INDICES:
                (output / f"dose_time_{time_index}.npz").write_bytes(f"dose-{time_index}".encode())
            seed = int(command[command.index("--seed") + 1])
            group = command[command.index("--activation-groups") + 1]
            cell = float(command[command.index("--activation-cell-cm") + 1])
            receipt = {"kind": "r3-r2s", "threads": 4, "activation_backend": "bounded",
                       "settings": {"activation_cell_cm": cell, "activation_groups": group, "seeds": [seed]},
                       "steps": {name: {"ok": True} for name in
                                 ("neutron_transport", "activation", "photon_transport")},
                       "photon": {label: {} for label in campaign.sc.COOLING_LABELS.values()},
                       "activation_microxs": {"path": "microxs.bin", "sha256": campaign.sha256(artifact)}}
            (output / "r2s_record.json").write_text(json.dumps(receipt), encoding="utf-8")
        else:
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps({"schema": "faris-r3/v1", "cooling_times": {}}), encoding="utf-8")
        return {"status": "COMPLETED", "returncode": 0, "elapsed_s": 0.01,
                "timeout_s": kwargs["timeout_s"], "output_limit_bytes": kwargs["output_limit_bytes"],
                "log_limit_bytes": kwargs["log_limit_bytes"], "reason": None}

    def test_seed_map_has_disjoint_stage_and_replicate_seeds(self):
        reps = campaign.seed_map(3, 60000)
        all_seeds = []
        for rep in reps:
            all_seeds.append(rep["d1s_seed"])
            for grid in rep["r2s"]:
                all_seeds.extend((grid["neutron"], grid["photon"]))
        self.assertEqual(len(all_seeds), len(set(all_seeds)))
        self.assertEqual([g["activation_cell_cm"] for g in reps[0]["r2s"]], [100, 50, 25])
        self.assertEqual([g["photon"] for g in reps[0]["r2s"]],
                         [g["cli_seed"] + 1 for g in reps[0]["r2s"]])

    def test_plan_predeclares_wall_memory_disk_and_log_bounds(self):
        with tempfile.TemporaryDirectory() as temp:
            plan = self.plan(Path(temp))
        limits = plan["execution_limits"]
        self.assertEqual(limits["memory_max_bytes"], 6 * campaign.GIB)
        self.assertEqual(limits["disk_reserve_bytes_before_each_task"], 8 * campaign.GIB)
        self.assertEqual(limits["log_max_bytes"], 32 * campaign.MIB)
        self.assertEqual(limits["task_limits"]["d1s"]["timeout_s"], 7600)
        self.assertEqual(limits["task_limits"]["r2s"]["timeout_s"], 2 * 86400 + 3 * 3000 + 900)
        self.assertEqual(limits["maximum_campaign_wall_s"], 2 * (7600 + 3 * (2 * 86400 + 3 * 3000 + 900) + 3 * 300))

    def test_plan_freezes_selected_thermal_basis_and_command(self):
        with tempfile.TemporaryDirectory() as temp:
            plan = self.plan(Path(temp), activation_groups="VITAMIN-J-175-plus-0-to-50MeV")
            self.assertEqual(plan["standard_settings"]["r2s"]["activation_groups"],
                             "VITAMIN-J-175-plus-0-to-50MeV")
            self.assertEqual(plan["convergence_dimensions"]["activation_energy_basis"]["group_count"], 177)
            self.assertEqual(plan["convergence_dimensions"]["activation_energy_basis"]["energy_edges_sha256"],
                             campaign.sc.ACTIVATION_GROUP_BASIS["VITAMIN-J-175-plus-0-to-50MeV"]["energy_edges_sha256"])

    def test_execute_bounded_terminates_process_group_on_deadline(self):
        class FakeProcess:
            pid = 1201
            stdout = io.BytesIO()

            def wait(self, timeout=None):
                return -15

        process = FakeProcess()
        signals = []
        stopped_scopes = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with mock.patch.object(campaign.time, "monotonic", side_effect=[0.0, 2.0, 3.0]):
                result = campaign.execute_bounded(["mock"], cwd=temp, env={}, log_path=root / "task.log",
                                                  task_output=root / "out", timeout_s=1.0,
                                                  output_limit_bytes=1024, guard_prefix=["systemd-run"],
                                                  popen=lambda *a, **k: process,
                                                  killpg=lambda pid, sig: signals.append((pid, sig)),
                                                  scope_unit="faris-r3-test-1.scope",
                                                  stop_scope=lambda: campaign._stop_named_scope(
                                                      "/usr/bin/systemctl", "faris-r3-test-1.scope",
                                                      runner=lambda command, **kwargs: stopped_scopes.append(
                                                          (command, kwargs))))
        self.assertEqual(result["status"], "TIMED_OUT")
        self.assertEqual(stopped_scopes[0][0],
                         ["/usr/bin/systemctl", "--user", "stop", "faris-r3-test-1.scope"])
        self.assertEqual(stopped_scopes[0][1]["timeout"], 10)
        self.assertEqual(signals, [(1201, campaign.signal.SIGTERM)])

    def test_execute_bounded_cancels_group_on_keyboard_interrupt(self):
        class FakeProcess:
            pid = 1202
            stdout = io.BytesIO()
            interrupted = False

            def wait(self, timeout=None):
                if not self.interrupted:
                    self.interrupted = True
                    raise KeyboardInterrupt
                return -15

        signals = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            result = campaign.execute_bounded(["mock"], cwd=temp, env={}, log_path=root / "task.log",
                                              task_output=root / "out", timeout_s=10,
                                              output_limit_bytes=1024, guard_prefix=["systemd-run"],
                                              popen=lambda *a, **k: FakeProcess(),
                                              killpg=lambda pid, sig: signals.append((pid, sig)))
        self.assertEqual(result["status"], "CANCELLED")
        self.assertEqual(signals, [(1202, campaign.signal.SIGTERM)])

    def test_execute_bounded_caps_log_and_stops_group(self):
        class FakeProcess:
            pid = 1203
            stdout = io.BytesIO(b"x" * 100)

            def wait(self, timeout=None):
                return 0

        signals = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            result = campaign.execute_bounded(["mock"], cwd=temp, env={}, log_path=root / "task.log",
                                              task_output=root / "out", timeout_s=10,
                                              output_limit_bytes=1024, log_limit_bytes=8,
                                              guard_prefix=["systemd-run"],
                                              popen=lambda *a, **k: FakeProcess(),
                                              killpg=lambda pid, sig: signals.append((pid, sig)))
            self.assertEqual((root / "task.log").stat().st_size, 8)
        self.assertEqual(result["status"], "STOPPED_ON_LOG_LIMIT")
        self.assertEqual(signals, [(1203, campaign.signal.SIGTERM)])

    def test_execute_bounded_stops_on_output_file_cap(self):
        class FakeProcess:
            pid = 1204
            stdout = io.BytesIO()

            def wait(self, timeout=None):
                return -15

        signals = []
        stopped_scopes = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / "out"
            output.mkdir()
            (output / "too-large").write_bytes(b"x" * 11)
            result = campaign.execute_bounded(["mock"], cwd=temp, env={}, log_path=root / "task.log",
                                              task_output=output, timeout_s=10,
                                              output_limit_bytes=10, guard_prefix=["systemd-run"],
                                              popen=lambda *a, **k: FakeProcess(),
                                              killpg=lambda pid, sig: signals.append((pid, sig)),
                                              scope_unit="faris-r3-output-limit.scope",
                                              stop_scope=lambda: stopped_scopes.append("stopped"))
        self.assertEqual(result["status"], "STOPPED_ON_OUTPUT_LIMIT")
        self.assertEqual(signals, [(1204, campaign.signal.SIGTERM)])
        self.assertEqual(stopped_scopes, ["stopped"])

    def test_plan_refuses_one_replicate(self):
        with self.assertRaisesRegex(ValueError, "at least two"):
            campaign.seed_map(1, 100)

    def test_serial_matrix_records_commands_and_does_not_claim_verdict(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root, activation_groups="VITAMIN-J-175-plus-0-to-50MeV")
            calls = []

            def fake_execute(command, **kwargs):
                calls.append(command)
                state = json.loads((Path(plan["output_dir"]) / "campaign-state.json").read_text())
                self.assertEqual(state["status"], "RUNNING_TASK")
                self.assertGreater(state["current_task"]["deadline_s"], 0)
                self.assertIn("deadline_utc", state["current_task"])
                self.assertEqual(state["current_task"]["command"][0], plan["inputs"]["systemd_run"])
                self.assertEqual(state["current_task"]["status"], "STARTING")
                return self.fake_complete(command, **kwargs)

            state = campaign.run_campaign(plan, executor=fake_execute,
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(state["status"], "ALL_COMMANDS_EXITED_ZERO")
            self.assertEqual(state["scientific_verdict"], "NOT_EVALUATED_BY_ORCHESTRATOR")
            self.assertEqual(len(calls), 14)  # 2 x (D1S + 3 x (R2S + comparison)).
            self.assertIn("--activation-cell-cm", calls[1])
            self.assertEqual(calls[1][calls[1].index("--activation-cell-cm") + 1], "100")
            self.assertEqual(calls[1][calls[1].index("--activation-groups") + 1],
                             "VITAMIN-J-175-plus-0-to-50MeV")
            saved = json.loads((Path(plan["output_dir"]) / "campaign-state.json").read_text())
            self.assertEqual(saved["tasks"][-1]["kind"], "compare")
            self.assertEqual(saved["tasks"][0]["record_sha256"], campaign.sha256(
                Path(plan["output_dir"]) / saved["tasks"][0]["record_path"]))

    def test_plan_only_bounded_batch_resumes_after_verified_task_without_repeating_it(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            plan_path = campaign.write_plan_only(plan)
            calls = []

            def run_one(command, **kwargs):
                calls.append(command)
                return self.fake_complete(command, **kwargs)

            first = campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                          resume_python=Path(plan["inputs"]["python"]),
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(first["status"], "IN_PROGRESS")
            self.assertEqual(first["scientific_verdict"], "NOT_EVALUATED_BY_ORCHESTRATOR")
            self.assertEqual(first["completed_task_count"], 1)
            self.assertEqual(first["remaining_task_count"], 13)
            self.assertEqual(Path(plan["output_dir"]) / "campaign-plan.json", plan_path)
            self.assertEqual(Path(calls[0][1]).name, "r3_d1s.py")

            resumed_plan = json.loads(plan_path.read_text(encoding="utf-8"))
            second = campaign.run_campaign(resumed_plan, executor=run_one, max_tasks=1, resume=True,
                                           resume_python=Path(resumed_plan["inputs"]["python"]),
                                           disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(second["status"], "IN_PROGRESS")
            self.assertEqual(second["completed_task_count"], 2)
            self.assertEqual(len(calls), 2)
            self.assertEqual(Path(calls[0][1]).name, "r3_d1s.py")
            self.assertEqual(Path(calls[1][1]).name, "r3_r2s.py")
            self.assertEqual(second["tasks"][0]["task_index"], 1)
            self.assertEqual(second["tasks"][1]["task_index"], 2)

    def test_cli_plan_only_and_resume_options(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            args = ["--model-dir", plan["inputs"]["model_dir"],
                    "--cross-sections", plan["inputs"]["cross_sections_xml"]["path"],
                    "--python", plan["inputs"]["python"], "--out-dir", plan["output_dir"],
                    "--replicates", "2", "--seed-base", "50000", "--purpose", "conditional-method-control",
                    "--plan-only"]
            with mock.patch.object(campaign, "make_plan", return_value=plan), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(campaign.main(args), 0)
            plan_path = Path(plan["output_dir"]) / "campaign-plan.json"
            self.assertTrue(plan_path.is_file())
            state = {"status": "IN_PROGRESS", "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR",
                     "tasks": [{}], "remaining_task_count": 13}
            with mock.patch.object(campaign, "run_campaign", return_value=state) as resume, \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(campaign.main(["--resume-from-existing-plan", str(plan_path), "--max-tasks", "1",
                                                "--python", plan["inputs"]["python"]]), 0)
            self.assertEqual(resume.call_args.kwargs, {"max_tasks": 1, "resume": True,
                                                       "resume_python": Path(plan["inputs"]["python"])})

    def test_resume_rejects_tampered_task_receipt_and_output_artifact(self):
        for target in ("receipt", "artifact"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                plan = self.plan(root)
                campaign.write_plan_only(plan)
                calls = []

                def run_one(command, **kwargs):
                    calls.append(command)
                    return self.fake_complete(command, **kwargs)

                campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                      resume_python=Path(plan["inputs"]["python"]),
                                      disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
                task_dir = Path(plan["output_dir"]) / "rep-01" / "d1s"
                if target == "receipt":
                    (task_dir / "d1s_record.json").write_text('{"tampered":true}\n', encoding="utf-8")
                    message = "completed task receipt changed"
                else:
                    (task_dir / "basis.bin").write_bytes(b"tampered output")
                    message = "receipt artifact is missing or changed"
                with self.assertRaisesRegex(ValueError, message):
                    campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                          resume_python=Path(plan["inputs"]["python"]),
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
                self.assertEqual(len(calls), 1)

    def test_resume_revalidates_cross_section_and_helper_identities(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            calls = []
            def run_one(command, **kwargs):
                calls.append(command)
                return self.fake_complete(command, **kwargs)
            campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                  resume_python=Path(plan["inputs"]["python"]),
                                  disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            xs = Path(plan["inputs"]["cross_sections_xml"]["path"])
            xs.write_text("<changed/>", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "cross-section index changed"):
                campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                      resume_python=Path(plan["inputs"]["python"]),
                                      disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(len(calls), 1)

    def test_malicious_imported_plan_cannot_select_python_or_helper_before_state_write(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            marker = root / "python-was-run"
            sentinel = root / "sentinel-python"
            sentinel.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
            sentinel.chmod(0o755)
            plan["inputs"]["python"] = str(sentinel.resolve())
            plan["inputs"]["python_sha256"] = campaign.sha256(sentinel)
            calls = []
            with self.assertRaisesRegex(ValueError, "resume Python executable path"):
                campaign.run_campaign(plan, executor=lambda *args, **kwargs: calls.append(args), resume=True)
            self.assertEqual(calls, [])
            self.assertFalse(marker.exists())
            self.assertFalse((Path(plan["output_dir"]) / "campaign-state.json").exists())

            plan["inputs"]["python"] = str(Path(plan["inputs"]["python"]).parent / "python")
            plan["inputs"]["implementation_sha256"]["../sentinel.py"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "canonical helper set"):
                campaign.run_campaign(plan, executor=lambda *args, **kwargs: calls.append(args), resume=True)
            self.assertEqual(calls, [])
            self.assertFalse((Path(plan["output_dir"]) / "campaign-state.json").exists())

    def test_default_executor_binds_hyphenated_systemd_commands_to_current_path(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            first_bin = root / "bin-one"
            second_bin = root / "bin-two"
            first_bin.mkdir()
            second_bin.mkdir()
            for directory, marker in ((first_bin, "one"), (second_bin, "two")):
                for name in ("systemd-run", "systemctl"):
                    executable = directory / name
                    executable.write_text(f"#!/bin/sh\necho {marker}\n", encoding="utf-8")
                    executable.chmod(0o755)
            with mock.patch.dict(os.environ, {"PATH": str(first_bin)}):
                plan = self.plan(root)
                calls = []
                with mock.patch.object(campaign, "execute_bounded", side_effect=lambda command, **kwargs:
                                       (calls.append(command), self.fake_complete(command, **kwargs))[1]):
                    first = campaign.run_campaign(plan, max_tasks=1,
                                                  disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
                self.assertEqual(first["status"], "IN_PROGRESS")
                self.assertEqual(len(calls), 1)
                with mock.patch.dict(os.environ, {"PATH": str(second_bin)}):
                    with self.assertRaisesRegex(ValueError, "systemd_run runtime changed"):
                        campaign.run_campaign(plan, max_tasks=1, resume=True,
                                              resume_python=Path(plan["inputs"]["python"]),
                                              disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
                self.assertEqual(len(calls), 1)

    def test_referenced_cross_section_asset_change_is_rejected_with_unchanged_xml(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            xml = Path(plan["inputs"]["cross_sections_xml"]["path"])
            thermal = xml.parent / "library-data" / "thermal.h5"
            thermal.write_bytes(b"changed thermal data")
            calls = []
            with self.assertRaisesRegex(ValueError, "cross-section data assets changed"):
                campaign.run_campaign(plan, executor=lambda *a, **k: calls.append(a), resume=True,
                                      resume_python=Path(plan["inputs"]["python"]))
            self.assertEqual(calls, [])
            self.assertFalse((Path(plan["output_dir"]) / "campaign-state.json").exists())
            self.assertEqual(xml.read_text(encoding="utf-8").count("library type="), 3)

    def test_cross_section_library_path_must_resolve_to_file(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            _model, xs, _python, _chain = self.fixture(root)
            xs.write_text("<cross_sections><directory>library-data</directory>"
                          "<library type='neutron' path='.'/></cross_sections>\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "library path must resolve to a file"):
                campaign._cross_section_asset_manifest(xs)

    def test_chain_change_before_resume_is_rejected_before_executor_or_state_write(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            Path(plan["inputs"]["activation_chain"]["path"]).write_text("<different-chain/>\n", encoding="utf-8")
            calls = []
            with self.assertRaisesRegex(ValueError, "shared activation/decay chain changed"):
                campaign.run_campaign(plan, executor=lambda *a, **k: calls.append(a), resume=True,
                                      resume_python=Path(plan["inputs"]["python"]))
            self.assertEqual(calls, [])
            self.assertFalse((Path(plan["output_dir"]) / "campaign-state.json").exists())

    def test_chain_change_during_task_prevents_completion_mark(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            chain = Path(plan["inputs"]["activation_chain"]["path"])

            def mutate_chain(command, **kwargs):
                result = self.fake_complete(command, **kwargs)
                chain.write_text("<depletion_chain><nuclide name='Fe56'><decay target='Co56'/></nuclide></depletion_chain>\n",
                                 encoding="utf-8")
                return result

            state = campaign.run_campaign(plan, executor=mutate_chain, max_tasks=1, resume=True,
                                          resume_python=Path(plan["inputs"]["python"]),
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(state["status"], "STOPPED_ON_FROZEN_INPUT_CHANGE")
            self.assertEqual(state["tasks"], [])
            self.assertEqual(state["current_task"]["status"], "COMPLETED_BUT_FROZEN_INPUT_CHANGED")
            self.assertEqual(state["scientific_verdict"], "NOT_EVALUATED_BY_ORCHESTRATOR")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            campaign.write_plan_only(plan)
            calls = []
            def run_one(command, **kwargs):
                calls.append(command)
                return self.fake_complete(command, **kwargs)
            campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                  resume_python=Path(plan["inputs"]["python"]),
                                  disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            original_sha = campaign.sha256
            changed_helper = (campaign.RISK_DIR / "r3_r2s.py").resolve()
            def changed_digest(path):
                if Path(path).resolve() == changed_helper:
                    return "0" * 64
                return original_sha(path)
            with mock.patch.object(campaign, "sha256", side_effect=changed_digest):
                with self.assertRaisesRegex(ValueError, "implementation changed"):
                    campaign.run_campaign(plan, executor=run_one, max_tasks=1, resume=True,
                                          resume_python=Path(plan["inputs"]["python"]),
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(len(calls), 1)

    def test_failed_stage_stops_later_runs(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            calls = []

            def fail_first(command, **kwargs):
                calls.append(command)
                return {"status": "FAILED", "returncode": 2, "elapsed_s": 0.01,
                        "timeout_s": kwargs["timeout_s"], "output_limit_bytes": kwargs["output_limit_bytes"],
                        "log_limit_bytes": kwargs["log_limit_bytes"], "reason": "test"}

            state = campaign.run_campaign(plan, executor=fail_first,
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(state["status"], "STOPPED_ON_TASK_LIMIT_OR_FAILURE")
            self.assertEqual(len(calls), 1)

    def test_campaign_keyboard_interrupt_persists_cancelled_current_task(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            def interrupt(command, **kwargs):
                state = json.loads((Path(plan["output_dir"]) / "campaign-state.json").read_text())
                self.assertEqual(state["current_task"]["status"], "STARTING")
                raise KeyboardInterrupt
            state = campaign.run_campaign(plan, executor=interrupt,
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(state["status"], "CANCELLED")
            self.assertEqual(state["current_task"]["status"], "CANCELLED")

    def test_campaign_disk_reserve_failure_precedes_external_command(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            called = []
            state = campaign.run_campaign(plan, executor=lambda *a, **k: called.append(a),
                                          disk_usage=lambda _path: SimpleNamespace(free=1))
            self.assertEqual(state["status"], "STOPPED_ON_DISK_RESERVE")
            self.assertEqual(called, [])

    def test_existing_campaign_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            out = root / "campaign"
            out.mkdir()
            sentinel = out / "keep.txt"
            sentinel.write_text("keep", encoding="utf-8")
            plan = self.plan(root, out=out)
            called = []
            with self.assertRaises(FileExistsError):
                campaign.run_campaign(plan, executor=lambda *a, **k: called.append(a))
            self.assertEqual(called, [])
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep")

    def test_implementation_change_after_plan_refuses_before_execution(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            original = campaign.RISK_DIR / "r3_r2s.py"
            old_digest = plan["inputs"]["implementation_sha256"]["r3_r2s.py"]
            plan["inputs"]["implementation_sha256"]["r3_r2s.py"] = "0" * 64
            called = []
            with self.assertRaisesRegex(ValueError, "implementation changed"):
                campaign.run_campaign(plan, executor=lambda *a, **k: called.append(a))
            self.assertEqual(called, [])
            self.assertFalse(Path(plan["output_dir"]).exists())
            self.assertNotEqual(campaign.sha256(original), "0" * 64)
            self.assertEqual(len(old_digest), 64)

    def test_input_mutation_after_plan_refuses_before_execution(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            xs = Path(plan["inputs"]["cross_sections_xml"]["path"])
            xs.write_text("<changed/>\n", encoding="utf-8")
            called = []
            with self.assertRaisesRegex(ValueError, "cross-section index changed"):
                campaign.run_campaign(plan, executor=lambda *a, **k: called.append(a))
            self.assertEqual(called, [])
            self.assertFalse(Path(plan["output_dir"]).exists())


if __name__ == "__main__":
    unittest.main()
