# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Lightweight contract tests for the serial R3 campaign collector (no OpenMC)."""
from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace

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

    def test_plan_refuses_one_replicate(self):
        with self.assertRaisesRegex(ValueError, "at least two"):
            campaign.seed_map(1, 100)

    def test_serial_matrix_records_commands_and_does_not_claim_verdict(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plan = self.plan(root)
            calls = []

            def fake_run(command, **kwargs):
                calls.append(command)
                if "r3_d1s.py" in command[1]:
                    out = Path(command[command.index("--out-dir") + 1])
                    out.mkdir(parents=True)
                    (out / "d1s_record.json").write_text("{}\n", encoding="utf-8")
                elif "r3_r2s.py" in command[1]:
                    out = Path(command[command.index("--out-dir") + 1])
                    out.mkdir(parents=True)
                    (out / "r2s_record.json").write_text("{}\n", encoding="utf-8")
                else:
                    dest = Path(command[command.index("--out") + 1])
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_text("{}\n", encoding="utf-8")
                return SimpleNamespace(returncode=0)

            state = campaign.run_campaign(plan, runner=fake_run)
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
                return SimpleNamespace(returncode=2)

            state = campaign.run_campaign(plan, runner=fail_first)
            self.assertEqual(state["status"], "STOPPED_ON_COMMAND_FAILURE")
            self.assertEqual(len(calls), 1)

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
                campaign.run_campaign(plan, runner=lambda *a, **k: called.append(a))
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
                campaign.run_campaign(plan, runner=lambda *a, **k: called.append(a))
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
                campaign.run_campaign(plan, runner=lambda *a, **k: called.append(a))
            self.assertEqual(called, [])
            self.assertFalse(Path(plan["output_dir"]).exists())


if __name__ == "__main__":
    unittest.main()
