#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Generate the operating-history primary refinement report (schema v4).

For each of the four transport runs (control/port x reference/breeder) and for the
event demonstration (driven by the control-reference run) this script:

1. writes an assumptions file per integration step (600, 500, 250 s); the 600 s run
   uses the base assumptions file byte for byte, the finer steps are copies with only
   `maximum_step_s` changed;
2. runs `faris history from-run` (history and rates outputs);
3. runs `controls/check_history.py --engine-history H --normalized-run RUN`;
4. runs `controls/check_history.py --refinement` for 600->500 and 500->250.

Any failed command, failed check, failed gate or identity mismatch exits nonzero and
writes nothing to --output. --work-dir and --output must not already exist.

The packager (scripts/package_recorded_demo.py) re-runs the multiplier-1.0 outage
probe with `faris history from-run` and requires its history and rates bytes to match
the 600 s records here. The history and rates bytes were checked to be independent of
the run, scenario and assumptions file *paths* (the same run copied to another
directory gave identical bytes; neither output embeds a path). They do depend on the
run record and raw artifact contents, the scenario, the assumptions values and the
`faris` binary. Generate this report with the same `faris` binary that goes into the
package and from the same run records the packager is given.

Run, scenario and assumptions identities are checked against each other before any
history is generated. This is a numerical audit of the deterministic ledger on fixed
sampled-mean inputs; it carries no engineering qualification.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CHECKER = REPO / "controls" / "check_history.py"
STEP_GRID_S = (600, 500, 250)
SCHEMA_VERSION = "faris-operating-history-primary-refinement-v4"
PROCESSING_MODEL = "continuous-delayed-release-v2"
DRIVERS = ("control-reference", "control-breeder", "port-reference", "port-breeder")
PHASES = ("off-delayed-off", "off-delayed-on", "on-delayed-off", "on-delayed-on")
FROM_RUN_TIMEOUT_S = 1800
CHECKER_TIMEOUT_S = 3600

HISTORICAL_SUPERSESSION = (
    "Supersedes operating-history-primary-refinement-v3.json. v3 was bound to the 1M-history "
    "corrected-geometry transport runs and to the pre-fix release-window ledger; neither those runs "
    "nor the pre-fix history bytes are current. Earlier histories using midpoint cohorts, older "
    "restart logic, pre-correction transport geometry, or separately accumulated signed-net energy "
    "are likewise not current v4 results.")
STATISTICAL_SCOPE = (
    "Refinement compares deterministic ledger calculations from fixed mean rates; it does not reduce "
    "or propagate Monte Carlo standard errors, estimate event probabilities, or qualify transport/"
    "nuclear data/material limits. Zero-scored regional flux remains an observed sample mean, not "
    "proof of zero true flux.")
EVENT_SCOPE = (
    "Authored software/ledger event demonstration driven by the sampled-mean transport rates of the "
    "control-reference run. Service ceilings are illustrative software inputs, not qualified material "
    "limits or lifetime claims.")
CHECKER_METHOD = (
    "Python Decimal site-balance/source-rate/unit checks; closed-form delayed-process phases and "
    "delay boundaries; same-version refinement criteria")


class ReportError(RuntimeError):
    pass


def sha256_hex(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def invoke(command: list[str], timeout: int) -> str:
    """Run a command and return stdout; any nonzero exit raises."""
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise ReportError(f"command timed out after {timeout}s: {' '.join(command[:4])}") from error
    if done.returncode != 0:
        raise ReportError(f"command failed ({done.returncode}): {' '.join(command)}\n{done.stderr[-2000:]}")
    return done.stdout


def check_json(*arguments: str) -> dict:
    out = invoke([sys.executable, str(CHECKER), *arguments], CHECKER_TIMEOUT_S)
    try:
        return json.loads(out)
    except json.JSONDecodeError as error:
        raise ReportError("independent checker did not return JSON") from error


def repo_relative(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(REPO).as_posix()
    except ValueError:
        return str(path)


def write_assumptions(base: Path, step: int, base_step: int, target: Path) -> str:
    """The base-step file is the base file itself; other steps change only maximum_step_s."""
    if step == base_step:
        shutil.copyfile(base, target)
    else:
        content = json.loads(base.read_text(encoding="utf-8"))
        content["maximum_step_s"] = step
        target.write_text(json.dumps(content, indent=2) + "\n", encoding="utf-8")
    return sha256_hex(target)


def summarize_history(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def generate_history(faris: Path, scenario: Path, run: Path, assumptions: Path,
                     history: Path, rates: Path) -> None:
    invoke([str(faris), "history", "from-run", "--scenario", str(scenario), "--run", str(run),
            "--assumptions", str(assumptions), "--output", str(history),
            "--rates-output", str(rates)], FROM_RUN_TIMEOUT_S)
    if not history.is_file() or not rates.is_file():
        raise ReportError(f"faris did not write history/rates for {history}")


def audit(history: Path, run: Path) -> dict:
    report = check_json("--engine-history", str(history), "--normalized-run", str(run))
    results = report.get("engine_results")
    if not isinstance(results, list) or len(results) != 1:
        raise ReportError(f"checker returned an unexpected report for {history}")
    result = results[0]
    if (result.get("processing_model") != PROCESSING_MODEL
            or result.get("independent_source_rate_production_burn_checks") != "PASS"
            or result.get("energy_audit", {}).get("status") != "PASS"
            or result.get("processing_delay_boundary_audit", {}).get("status") != "PASS"
            or (result.get("transport_binding_audit") or {}).get("status") != "PASS"
            or any(result.get("continuous_processing_phase_audits", {}).get(p, {}).get("status") != "PASS"
                   for p in PHASES)):
        raise ReportError(f"independent checker did not pass every check for {history}")
    return result


def gate(coarse: Path, fine: Path, coarse_s: int, fine_s: int, case: str | None) -> dict:
    report = check_json("--refinement", str(coarse), str(fine))
    result = report.get("refinement")
    if not isinstance(result, dict) or result.get("acceptance") != "PASS":
        raise ReportError(f"refinement gate {coarse_s}->{fine_s} did not pass for {case or 'event demo'}")
    record = {"coarse_s": coarse_s, "fine_s": fine_s, "result": result}
    if case:
        record = {"case": case, **record}
    return record


def run_identity(run_path: Path, scenario_path: Path) -> dict:
    run = json.loads(run_path.read_text(encoding="utf-8"))
    execution = run.get("execution") or {}
    if execution.get("execution_status") != "SUCCEEDED" or execution.get("exit_code") != 0:
        raise ReportError(f"run did not succeed: {run_path}")
    if not run.get("normalized"):
        raise ReportError(f"run has no normalized transport result: {run_path}")
    scenario_sha = sha256_hex(scenario_path)
    if run.get("scenario_sha256") != scenario_sha:
        raise ReportError(f"run {run_path} was not produced from scenario {scenario_path}")
    return {
        "execution_status": execution["execution_status"],
        "exit_code": execution["exit_code"],
        "input_sha256": run["input_sha256"],
        "mesh": run["mesh"],
        "physics_sha256": run["physics_sha256"],
        "raw_artifact_sha256": run["raw_artifact_sha256"],
        "run_json_sha256": sha256_hex(run_path),
        "run_path": str(run_path),
        "sampling": run["sampling"],
        "scenario_sha256": run["scenario_sha256"],
        "scientific_qualification": run["scientific_qualification"],
    }


def driver_output(audit_result: dict, history_path: Path, rates_path: Path,
                  assumptions_sha: str) -> dict:
    history = summarize_history(history_path)
    boundaries = audit_result["processing_delay_boundary_audit"]
    return {
        "assumptions_sha256": assumptions_sha,
        "continuous_phase_checks": {
            phase: audit_result["continuous_processing_phase_audits"][phase]["status"] for phase in PHASES},
        # The checker raises when the Decimal residual exceeds the engine tolerance, so a
        # returned audit means the site balance passed.
        "decimal_site_balance": "PASS",
        "delay_boundary_count": len(boundaries["checked_boundaries"]),
        "delay_boundary_status": boundaries["status"],
        "energy_ledger": audit_result["energy_audit"],
        "events": audit_result["event_count"],
        "history_path": str(history_path),
        "history_sha256": sha256_hex(history_path),
        "max_engine_balance_residual_kg": audit_result["max_engine_balance_residual_kg"],
        "rates_path": str(rates_path),
        "rates_sha256": sha256_hex(rates_path),
        "restart_crossing_status": audit_result["restart_crossing_audit"]["status"],
        "segment_limit": history["integration_segment_limit"],
        "segments": history["integration_segment_count"],
        "snapshots": audit_result["snapshot_count"],
        "source_rate_production_burn": audit_result["independent_source_rate_production_burn_checks"],
        "transport_binding_status": audit_result["transport_binding_audit"]["status"],
    }


def event_output(audit_result: dict, history_path: Path, rates_path: Path,
                 assumptions_path: Path) -> dict:
    history = summarize_history(history_path)
    kinds: dict[str, int] = {}
    limits: dict[str, int] = {}
    for event in history["events"]:
        kinds[event["kind"]] = kinds.get(event["kind"], 0) + 1
        if event["kind"] == "service_limit_reached":
            limits[event["component_id"]] = limits.get(event["component_id"], 0) + 1
    return {
        "assumptions_path": str(assumptions_path),
        "assumptions_sha256": sha256_hex(assumptions_path),
        "event_count": audit_result["event_count"],
        "event_kind_counts": dict(sorted(kinds.items())),
        "history_path": str(history_path),
        "history_sha256": sha256_hex(history_path),
        "independent_decimal_audit": {
            "continuous_processing_phases": {
                phase: audit_result["continuous_processing_phase_audits"][phase]["status"] for phase in PHASES},
            "decimal_site_balance": "PASS",
            "delay_boundary_status": audit_result["processing_delay_boundary_audit"]["status"],
            "energy_ledger": audit_result["energy_audit"]["status"],
            "max_decimal_balance_residual_kg": audit_result["max_decimal_balance_residual_kg"],
            "max_engine_balance_residual_kg": audit_result["max_engine_balance_residual_kg"],
            "source_rate_production_burn": audit_result["independent_source_rate_production_burn_checks"],
            "status": "PASS",
            "transport_binding_status": audit_result["transport_binding_audit"]["status"],
        },
        "integration_segment_count": history["integration_segment_count"],
        "outcome": history["outcome"],
        "processing_model": audit_result["processing_model"],
        "rates_path": str(rates_path),
        "rates_sha256": sha256_hex(rates_path),
        "replacement_completion_count": kinds.get("replacement_completed", 0),
        "service_limit_counts": dict(sorted(limits.items())),
        "snapshot_count": audit_result["snapshot_count"],
    }


def sweep(faris: Path, scenario: Path, run: Path, base: Path, directory: Path, label: str):
    """Generate and audit the histories of one driver; returns (audits, history paths, assumptions)."""
    base_step = json.loads(base.read_text(encoding="utf-8")).get("maximum_step_s")
    if base_step != STEP_GRID_S[0]:
        raise ReportError(f"{base} must use maximum_step_s {STEP_GRID_S[0]} (found {base_step!r})")
    directory.mkdir(parents=True)
    audits, histories, rates, assumptions = {}, {}, {}, {}
    for step in STEP_GRID_S:
        assumptions[step] = directory / f"{label}-{step}-assumptions.json"
        write_assumptions(base, step, base_step, assumptions[step])
        histories[step] = directory / f"{label}-{step}-history.json"
        rates[step] = directory / f"{label}-{step}-rates.json"
        generate_history(faris, scenario, run, assumptions[step], histories[step], rates[step])
        audits[step] = audit(histories[step], run)
    if len({sha256_hex(path) for path in rates.values()}) != 1:
        raise ReportError(f"driving rates differ between integration steps for {label}")
    return audits, histories, rates, assumptions


def build_report(args) -> dict:
    faris = args.faris.resolve()
    work = args.work_dir
    runs = {"control-reference": args.control_reference_run, "control-breeder": args.control_breeder_run,
            "port-reference": args.port_reference_run, "port-breeder": args.port_breeder_run}
    scenarios = {"control": args.control_scenario, "port": args.port_scenario}
    base_sha = sha256_hex(args.assumptions)
    event_base_sha = sha256_hex(args.event_assumptions)
    drivers, gates = {}, []
    for driver in DRIVERS:
        run, scenario = runs[driver], scenarios[driver.split("-")[0]]
        record = run_identity(run, scenario)
        audits, histories, rates, assumptions = sweep(
            faris, scenario, run, args.assumptions, work / driver, driver)
        record["history_outputs"] = {
            str(step): driver_output(audits[step], histories[step], rates[step], sha256_hex(assumptions[step]))
            for step in STEP_GRID_S}
        if record["history_outputs"][str(STEP_GRID_S[0])]["assumptions_sha256"] != base_sha:
            raise ReportError(f"{driver}: the base-step run did not use the base assumptions bytes")
        for coarse, fine in zip(STEP_GRID_S, STEP_GRID_S[1:]):
            gates.append(gate(histories[coarse], histories[fine], coarse, fine, driver))
        drivers[driver] = record
    event_run, event_scenario = runs["control-reference"], scenarios["control"]
    audits, histories, rates, assumptions = sweep(
        faris, event_scenario, event_run, args.event_assumptions, work / "event-demo", "event")
    event_gates = [gate(histories[coarse], histories[fine], coarse, fine, None)
                   for coarse, fine in zip(STEP_GRID_S, STEP_GRID_S[1:])]
    event_histories = {str(step): event_output(audits[step], histories[step], rates[step], assumptions[step])
                       for step in STEP_GRID_S}
    if event_histories[str(STEP_GRID_S[0])]["assumptions_sha256"] != event_base_sha:
        raise ReportError("event demo: the base-step run did not use the base assumptions bytes")
    return {
        "baseline_assumptions": {"path": repo_relative(args.assumptions), "sha256": base_sha},
        "baseline_refinement_gates": gates,
        "calculation_cli": {"path": str(args.faris), "sha256": sha256_hex(faris)},
        "created_utc": datetime.now(timezone.utc).date().isoformat(),
        "event_demo": {
            "assumption_source": repo_relative(args.event_assumptions),
            "assumption_source_sha256": event_base_sha,
            "assumptions_sha256": event_base_sha,
            "histories": event_histories,
            "refinement_gates": event_gates,
            "run_path": str(event_run),
            "run_sha256": sha256_hex(event_run),
            "scope": EVENT_SCOPE,
        },
        "historical_supersession": HISTORICAL_SUPERSESSION,
        "independent_checker": {"method": CHECKER_METHOD, "path": repo_relative(CHECKER),
                                "sha256": sha256_hex(CHECKER)},
        "primary_drivers": drivers,
        "processing_model": PROCESSING_MODEL,
        "schema_version": SCHEMA_VERSION,
        "statistical_scope": STATISTICAL_SCOPE,
        "step_grid_s": list(STEP_GRID_S),
    }


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    for name in ("faris", "control-scenario", "port-scenario", "control-reference-run",
                 "control-breeder-run", "port-reference-run", "port-breeder-run",
                 "assumptions", "event-assumptions", "work-dir", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    if args.output.exists() or args.work_dir.exists():
        print("error: --output and --work-dir must not already exist", file=sys.stderr)
        return 2
    for name in ("faris", "control_scenario", "port_scenario", "control_reference_run",
                 "control_breeder_run", "port_reference_run", "port_breeder_run",
                 "assumptions", "event_assumptions"):
        if not getattr(args, name).is_file():
            print(f"error: missing input file for --{name.replace('_', '-')}: {getattr(args, name)}",
                  file=sys.stderr)
            return 2
    args.work_dir.mkdir(parents=True)
    try:
        report = build_report(args)
    except (ReportError, OSError, KeyError, ValueError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n"
    with tempfile.NamedTemporaryFile("w", dir=args.output.parent, delete=False,
                                     prefix=".history-refinement-", encoding="utf-8") as handle:
        handle.write(text)
    Path(handle.name).rename(args.output)
    print(f"wrote {args.output} (sha256 {sha256_hex(args.output)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
