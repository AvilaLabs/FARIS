# SPDX-License-Identifier: AGPL-3.0-only
"""Unit tests for make_history_refinement_report.py; faris and the checker are faked."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("make_history_refinement_report.py")
SPEC = importlib.util.spec_from_file_location("make_history_refinement_report", SCRIPT)
MOD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MOD)

PHASE_AUDITS = {p: {"status": "PASS"} for p in MOD.PHASES}
ENGINE_RESULT = {
    "processing_model": MOD.PROCESSING_MODEL,
    "event_count": 2,
    "snapshot_count": 3,
    "max_decimal_balance_residual_kg": "1E-9",
    "max_engine_balance_residual_kg": 1e-9,
    "independent_source_rate_production_burn_checks": "PASS",
    "energy_audit": {"status": "PASS", "meaning": "fixture"},
    "continuous_processing_phase_audits": PHASE_AUDITS,
    "processing_delay_boundary_audit": {"status": "PASS", "checked_boundaries": [{}, {}]},
    "restart_crossing_audit": {"status": "NOT_EVALUATED_NO_OFF_RESTART_CYCLE"},
    "transport_binding_audit": {"status": "PASS"},
}
HISTORY = {
    "outcome": "horizon_completed", "integration_segment_count": 7, "integration_segment_limit": 100,
    "events": [{"kind": "service_limit_reached", "component_id": "blanket"},
               {"kind": "replacement_completed", "component_id": "blanket"}],
}


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class FakeTools:
    """Stands in for `invoke`: fabricates faris outputs and canned checker reports."""

    def __init__(self, fail_gate=False, fail_check=False):
        self.fail_gate, self.fail_check = fail_gate, fail_check
        self.assumption_bytes = {}

    def __call__(self, command, timeout):
        if command[1:3] == ["history", "from-run"]:
            arg = lambda flag: Path(command[command.index(flag) + 1])  # noqa: E731
            assumptions = arg("--assumptions")
            self.assumption_bytes[assumptions.name] = assumptions.read_bytes()
            arg("--output").write_text(json.dumps(HISTORY))
            arg("--rates-output").write_text("{}")
            return ""
        if "--engine-history" in command:
            if self.fail_check:
                raise MOD.ReportError("checker failed")
            return json.dumps({"engine_results": [ENGINE_RESULT]})
        accept = "FAIL" if self.fail_gate else "PASS"
        return json.dumps({"refinement": {"acceptance": accept}})


class Fixture:
    def __init__(self, root: Path):
        self.root = root
        self.scenarios = {}
        for name in ("control", "port"):
            self.scenarios[name] = root / f"{name}.scenario.json"
            self.scenarios[name].write_text(json.dumps({"id": name}))
        self.runs = {}
        for driver in MOD.DRIVERS:
            run = root / f"{driver}-run.json"
            scenario = self.scenarios[driver.split("-")[0]]
            run.write_text(json.dumps({
                "scenario_sha256": sha(scenario.read_bytes()),
                "execution": {"execution_status": "SUCCEEDED", "exit_code": 0},
                "normalized": {"x": 1}, "input_sha256": "i", "mesh": {}, "physics_sha256": "p",
                "raw_artifact_sha256": "r", "sampling": {}, "scientific_qualification": "NOT_EVALUATED"}))
            self.runs[driver] = run
        # Deliberately odd formatting: the 600 s step must reuse these exact bytes.
        self.base = root / "base.json"
        self.base.write_bytes(b'{"maximum_step_s":600,  "horizon_s": 10}')
        self.event = root / "event.json"
        self.event.write_bytes(b'{ "maximum_step_s": 600, "e": 1 }\n')
        (root / "faris").write_text("binary")

    def argv(self, name="out"):
        r = self.root
        return ["--faris", str(r / "faris"),
                "--control-scenario", str(self.scenarios["control"]),
                "--port-scenario", str(self.scenarios["port"]),
                "--control-reference-run", str(self.runs["control-reference"]),
                "--control-breeder-run", str(self.runs["control-breeder"]),
                "--port-reference-run", str(self.runs["port-reference"]),
                "--port-breeder-run", str(self.runs["port-breeder"]),
                "--assumptions", str(self.base), "--event-assumptions", str(self.event),
                "--work-dir", str(r / f"{name}-work"), "--output", str(r / f"{name}.json")]


class HistoryRefinementReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fx = Fixture(Path(self.tmp.name))

    def run_main(self, tools, argv=None):
        original = MOD.invoke
        MOD.invoke = tools
        try:
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
                return MOD.main(argv or self.fx.argv())
        finally:
            MOD.invoke = original

    def test_refuses_existing_output_and_work_dir(self):
        for target in ("out.json", "out-work"):
            with self.subTest(target=target):
                path = self.fx.root / target
                path.mkdir() if target.endswith("work") else path.write_text("x")
                tools = FakeTools()
                self.assertEqual(self.run_main(tools), 2)
                self.assertEqual(tools.assumption_bytes, {})
                if target == "out.json":
                    self.assertEqual(path.read_text(), "x")
                else:
                    self.assertFalse(self.fx.root.joinpath("out.json").exists())
                path.rmdir() if target.endswith("work") else path.unlink()

    def test_success_report_shape_and_base_step_bytes(self):
        tools = FakeTools()
        self.assertEqual(self.run_main(tools), 0)
        report = json.loads((self.fx.root / "out.json").read_text())
        self.assertEqual(report["schema_version"], MOD.SCHEMA_VERSION)
        self.assertEqual(report["step_grid_s"], [600, 500, 250])
        self.assertEqual(len(report["baseline_refinement_gates"]), 8)
        self.assertEqual(len(report["event_demo"]["refinement_gates"]), 2)
        base_bytes = self.fx.base.read_bytes()
        self.assertEqual(report["baseline_assumptions"]["sha256"], sha(base_bytes))
        for driver in MOD.DRIVERS:
            outputs = report["primary_drivers"][driver]["history_outputs"]
            self.assertEqual(outputs["600"]["assumptions_sha256"], sha(base_bytes))
            self.assertEqual(tools.assumption_bytes[f"{driver}-600-assumptions.json"], base_bytes)
            self.assertNotEqual(outputs["500"]["assumptions_sha256"], sha(base_bytes))
            self.assertEqual(outputs["250"]["segments"], 7)
        self.assertEqual(tools.assumption_bytes["event-600-assumptions.json"], self.fx.event.read_bytes())
        for step in (500, 250):
            changed = json.loads(tools.assumption_bytes[f"control-reference-{step}-assumptions.json"])
            self.assertEqual(changed, {"maximum_step_s": step, "horizon_s": 10})
        event = report["event_demo"]["histories"]["250"]
        self.assertEqual(event["service_limit_counts"], {"blanket": 1})
        self.assertEqual(event["replacement_completion_count"], 1)

    def test_failed_gate_writes_no_output(self):
        self.assertEqual(self.run_main(FakeTools(fail_gate=True)), 1)
        self.assertFalse((self.fx.root / "out.json").exists())

    def test_failed_check_writes_no_output(self):
        self.assertEqual(self.run_main(FakeTools(fail_check=True)), 1)
        self.assertFalse((self.fx.root / "out.json").exists())

    def test_check_with_failing_phase_is_rejected(self):
        bad = copy.deepcopy(ENGINE_RESULT)
        bad["continuous_processing_phase_audits"]["on-delayed-on"] = {"status": "FAIL"}
        original = MOD.check_json
        MOD.check_json = lambda *a: {"engine_results": [bad]}
        try:
            with self.assertRaises(MOD.ReportError):
                MOD.audit(Path("h.json"), Path("run.json"))
        finally:
            MOD.check_json = original

    def test_run_from_another_scenario_is_rejected(self):
        run = self.fx.runs["port-reference"]
        data = json.loads(run.read_text())
        data["scenario_sha256"] = "0" * 64
        run.write_text(json.dumps(data))
        self.assertEqual(self.run_main(FakeTools()), 1)
        self.assertFalse((self.fx.root / "out.json").exists())

    def test_base_assumptions_must_use_the_coarse_step(self):
        self.fx.base.write_text(json.dumps({"maximum_step_s": 300}))
        self.assertEqual(self.run_main(FakeTools()), 1)
        self.assertFalse((self.fx.root / "out.json").exists())


if __name__ == "__main__":
    unittest.main()
