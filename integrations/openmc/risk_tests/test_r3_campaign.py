# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Lightweight contract tests for the serial R3 campaign collector (no OpenMC)."""
from __future__ import annotations

import json
import io
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
        xs.write_text("<cross_sections/>\n", encoding="utf-8")
        python = root / "python"
        python.write_text("", encoding="utf-8")
        python.chmod(0o755)
        return model, xs, python

    def plan(self, root: Path, *, replicates=2, seed_base=50000, out=None):
        model, xs, python = self.fixture(root)
        return campaign.make_plan(model_dir=model, cross_sections=xs, python=python,
                                  out_dir=out or root / "campaign", replicates=replicates,
                                  seed_base=seed_base, purpose="conditional-method-control")

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
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / "out"
            output.mkdir()
            (output / "too-large").write_bytes(b"x" * 11)
            result = campaign.execute_bounded(["mock"], cwd=temp, env={}, log_path=root / "task.log",
                                              task_output=output, timeout_s=10,
                                              output_limit_bytes=10, guard_prefix=["systemd-run"],
                                              popen=lambda *a, **k: FakeProcess(),
                                              killpg=lambda pid, sig: signals.append((pid, sig)))
        self.assertEqual(result["status"], "STOPPED_ON_OUTPUT_LIMIT")
        self.assertEqual(signals, [(1204, campaign.signal.SIGTERM)])

    def test_plan_refuses_one_replicate(self):
        with self.assertRaisesRegex(ValueError, "at least two"):
            campaign.seed_map(1, 100)

    def test_serial_matrix_records_commands_and_does_not_claim_verdict(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            calls = []

            def fake_execute(command, **kwargs):
                calls.append(command)
                state = json.loads((Path(plan["output_dir"]) / "campaign-state.json").read_text())
                self.assertEqual(state["status"], "RUNNING_TASK")
                self.assertGreater(state["current_task"]["deadline_s"], 0)
                self.assertIn("deadline_utc", state["current_task"])
                self.assertEqual(state["current_task"]["command"][0], plan["inputs"]["systemd_run"])
                self.assertEqual(state["current_task"]["status"], "STARTING")
                if "r3_d1s.py" in command[1]:
                    out = kwargs["task_output"]
                    out.mkdir(parents=True)
                    (out / "d1s_record.json").write_text("{}\n", encoding="utf-8")
                elif "r3_r2s.py" in command[1]:
                    out = kwargs["task_output"]
                    out.mkdir(parents=True)
                    (out / "r2s_record.json").write_text("{}\n", encoding="utf-8")
                else:
                    dest = kwargs["task_output"]
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_text("{}\n", encoding="utf-8")
                return {"status": "COMPLETED", "returncode": 0, "elapsed_s": 0.01,
                        "timeout_s": kwargs["timeout_s"], "output_limit_bytes": kwargs["output_limit_bytes"],
                        "log_limit_bytes": kwargs["log_limit_bytes"], "reason": None}

            state = campaign.run_campaign(plan, executor=fake_execute,
                                          disk_usage=lambda _path: SimpleNamespace(free=64 * campaign.GIB))
            self.assertEqual(state["status"], "ALL_COMMANDS_EXITED_ZERO")
            self.assertEqual(state["scientific_verdict"], "NOT_EVALUATED_BY_ORCHESTRATOR")
            self.assertEqual(len(calls), 14)  # 2 x (D1S + 3 x (R2S + comparison)).
            self.assertIn("--activation-cell-cm", calls[1])
            self.assertEqual(calls[1][calls[1].index("--activation-cell-cm") + 1], "100")
            saved = json.loads((Path(plan["output_dir"]) / "campaign-state.json").read_text())
            self.assertEqual(saved["tasks"][-1]["kind"], "compare")
            self.assertEqual(saved["tasks"][0]["record_sha256"], campaign.sha256(
                Path(plan["output_dir"]) / saved["tasks"][0]["record_path"]))

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
