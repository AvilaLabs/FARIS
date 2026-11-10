#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Serial, predeclared R3 whole-workflow replicates across activation grids.

This is an evidence collector. It does not calculate a new convergence rule or
scientific verdict. Each replicate contains one D1S run and bounded R2S runs at
100, 50, and 25 cm, followed by the existing pairwise comparison reports.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import threading
import time
from datetime import datetime, timedelta, timezone

import r3_scenario as sc

ROOT = Path(__file__).resolve().parents[3]
RISK_DIR = Path(__file__).resolve().parent
GRIDS_CM = (100, 50, 25)
GIB = 1024 ** 3
MIB = 1024 ** 2
TASK_LIMITS = {
    "d1s": {"timeout_s": 7600, "output_bytes": 8 * GIB},
    # Existing internal maxima: 24 h neutron, 24 h bounded activation, and
    # three photon-time runs at 3000 s each, plus 15 min for setup/finalization.
    "r2s": {"timeout_s": 2 * 86400 + 3 * 3000 + 900, "output_bytes": 8 * GIB},
    "compare": {"timeout_s": 300, "output_bytes": 32 * MIB},
}
MEMORY_MAX_BYTES = 6 * GIB  # Existing R6 systemd scope guard.
DISK_RESERVE_BYTES = 8 * GIB  # Existing bounded R2S reserve.
LOG_MAX_BYTES = 32 * MIB  # Existing local transport log ceiling.
KILL_GRACE_S = 5
POLL_INTERVAL_S = 1


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def seed_map(replicates: int, seed_base: int) -> list[dict]:
    """Return disjoint D1S, R2S-neutron, and R2S-photon seeds per replicate/grid."""
    if replicates < 2:
        raise ValueError("at least two whole-workflow replicates are required")
    if seed_base < 1:
        raise ValueError("seed base must be a positive integer")
    entries = []
    used: set[int] = set()
    for replicate in range(replicates):
        offset = seed_base + replicate * 10
        d1s = offset
        if d1s in used:
            raise ValueError("seed mapping collides")
        used.add(d1s)
        r2s = []
        for grid_index, cell_cm in enumerate(GRIDS_CM):
            neutron = offset + 1 + grid_index * 2
            photon = neutron + 1  # r3_r2s.py uses seed + 1 for photon transport.
            if neutron in used or photon in used:
                raise ValueError("seed mapping collides")
            used.update((neutron, photon))
            r2s.append({"activation_cell_cm": cell_cm, "neutron": neutron, "photon": photon,
                        "cli_seed": neutron})
        entries.append({"replicate": replicate + 1, "d1s_seed": d1s, "r2s": r2s})
    return entries


def make_plan(*, model_dir: Path, cross_sections: Path, python: Path, out_dir: Path,
              replicates: int, seed_base: int, purpose: str,
              activation_groups: str = sc.ACTIVATION_GROUPS) -> dict:
    model_dir = model_dir.resolve()
    cross_sections = cross_sections.resolve()
    python = python.resolve()
    out_dir = out_dir.resolve()
    for label, path in (("Python executable", python), ("cross-section index", cross_sections)):
        if not path.is_file():
            raise FileNotFoundError(f"{label} is missing: {path}")
    if not os.access(python, os.X_OK):
        raise PermissionError(f"Python executable is not executable: {python}")
    if activation_groups not in sc.ACTIVATION_GROUP_BASIS:
        raise ValueError(f"unsupported R3 activation energy basis: {activation_groups}")
    dependencies = sc.rm_m_geometry_dependencies(model_dir)
    scripts = {name: RISK_DIR / name for name in
               ("r3_campaign.py", "r3_d1s.py", "r3_r2s.py", "r3_compare.py",
                "r3_scenario.py", "r3_dose_statistics.py", "r3_shutdown_dose_export.py",
                "r3_bounded_microxs.py", "r3_activation_coverage.py", "rm_m_openmc.py")}
    script_hashes = {name: sha256(path) for name, path in scripts.items()}
    return {
        "schema": "faris-r3-whole-workflow-campaign/v1",
        "status": "PREDECLARED",
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "purpose": purpose,
        "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR",
        "protocol": sc.FULL_EMISSION_PROTOCOL,
        "convergence_dimensions": {
            "activation_mesh_cm": list(GRIDS_CM),
            "activation_energy_basis": {"selected": activation_groups,
                                        **sc.ACTIVATION_GROUP_BASIS[activation_groups],
                                        "comparison_note": "energy-basis comparisons require separate predeclared campaigns; no additional threshold is applied"},
            "response_mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM),
                              "upper_right_cm": list(sc.MESH_UPPER_CM),
                              "dimension": list(sc.MESH_DIMENSION)},
            "note": "Activation-grid convergence evidence is separate from source-response mesh convergence; this runner does not evaluate either.",
        },
        "replicate_count": replicates,
        "seed_policy": "Within each replicate D1S and every R2S neutron and photon stage use unique seeds; replicate seed blocks are disjoint. The R2S CLI seed is neutron seed; photon seed is CLI seed + 1.",
        "replicates": seed_map(replicates, seed_base),
        "standard_settings": {
            "d1s": {"particles_per_batch": 5000, "max_batches": 100000, "budget_s": 7000.0},
            "r2s": {"neutron_particles_per_batch": 20000, "neutron_batches": 50,
                    "photon_particles_per_batch": 100000, "photon_max_batches": 5000,
                    "photon_budget_s_per_time": 3000.0, "activation_backend": "bounded",
                    "activation_groups": activation_groups, "photon_indices": list(sc.PHOTON_TIME_INDICES)},
            "threads": 4,
        },
        "execution_limits": {
            "memory_max_bytes": MEMORY_MAX_BYTES,
            "memory_guard": "systemd-run user scope MemoryMax=6G, MemorySwapMax=0 (same guard recipe as R6)",
            "disk_reserve_bytes_before_each_task": DISK_RESERVE_BYTES,
            "log_max_bytes": LOG_MAX_BYTES,
            "task_limits": TASK_LIMITS,
            "process_group": "new session; SIGTERM then SIGKILL on deadline, output/log limit, or KeyboardInterrupt",
            "maximum_campaign_wall_s": replicates * (TASK_LIMITS["d1s"]["timeout_s"] +
                3 * TASK_LIMITS["r2s"]["timeout_s"] + 3 * TASK_LIMITS["compare"]["timeout_s"]),
        },
        "inputs": {"model_dir": str(model_dir), "model_dependencies": dependencies,
                   "cross_sections_xml": {"path": str(cross_sections), "sha256": sha256(cross_sections)},
                   "python": str(python), "python_sha256": sha256(python),
            "systemd_run": shutil.which("systemd-run"),
            "systemd_run_sha256": sha256(Path(shutil.which("systemd-run")))
                if shutil.which("systemd-run") else None,
            "systemctl": shutil.which("systemctl"),
            "implementation_sha256": script_hashes},
        "output_dir": str(out_dir),
        "comparison_note": "r3_compare.py is run unchanged for each replicate/grid pair. It reports unknown D1S/R2S covariance as inconclusive; no campaign-level uncertainty or pass gate is added.",
    }


def _write_json(path: Path, value: dict) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(path)


def _tree_bytes(path: Path) -> int:
    if not path.exists():
        return 0
    if path.is_file():
        return path.stat().st_size
    total = 0
    for root, _dirs, files in os.walk(path, followlinks=False):
        for name in files:
            item = Path(root) / name
            try:
                if not item.is_symlink():
                    total += item.stat().st_size
            except FileNotFoundError:
                continue
    return total


def _stop_named_scope(systemctl: str, unit_name: str, *, runner=subprocess.run) -> None:
    runner([systemctl, "--user", "stop", unit_name], timeout=10, check=False,
           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _stop_process_group(proc, *, killpg=os.killpg, stop_scope=None,
                       grace_s: float = KILL_GRACE_S) -> int:
    # A systemd-run --scope client is not the scope itself. Stop the named
    # cgroup first; then terminate the client process group as a fallback.
    if stop_scope is not None:
        try:
            stop_scope()
        except (OSError, subprocess.SubprocessError):
            pass
    try:
        killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        return proc.wait(timeout=grace_s)
    except subprocess.TimeoutExpired:
        try:
            killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        return proc.wait()


def execute_bounded(command: list[str], *, cwd: str, env: dict, log_path: Path,
                    task_output: Path, timeout_s: float, output_limit_bytes: int,
                    log_limit_bytes: int = LOG_MAX_BYTES, guard_prefix: list[str] | None = None,
                    memory_max_bytes: int = MEMORY_MAX_BYTES,
                    popen=subprocess.Popen, killpg=os.killpg, scope_unit: str | None = None,
                    stop_scope=None) -> dict:
    """Run one task in a guarded process group with wall, output, and log bounds."""
    if not guard_prefix:
        raise RuntimeError("R3 campaign requires the predeclared systemd memory guard")
    if scope_unit is not None and stop_scope is None:
        raise ValueError("named systemd scope requires an explicit scope stop callback")
    if memory_max_bytes != MEMORY_MAX_BYTES:
        raise ValueError("R3 campaign memory cap must match the existing 6 GiB R6 guard")
    full_command = guard_prefix + ["--"] + [str(item) for item in command]
    started = time.monotonic()
    log_path.parent.mkdir(parents=True, exist_ok=True)
    status = "FAILED"
    returncode = None
    reason = None
    with log_path.open("wb") as log:
        overflow = threading.Event()
        proc = popen(full_command, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                     stderr=subprocess.STDOUT, start_new_session=True)

        def copy_bounded_log():
            written = 0
            pipe = proc.stdout
            while True:
                block = pipe.read1(65536) if hasattr(pipe, "read1") else pipe.read(65536)
                if not block:
                    break
                keep = min(len(block), max(0, log_limit_bytes - written))
                if keep:
                    log.write(block[:keep])
                    written += keep
                if keep < len(block):
                    overflow.set()
            log.flush()
            pipe.close()

        reader = threading.Thread(target=copy_bounded_log, daemon=True)
        reader.start()
        try:
            while True:
                elapsed = time.monotonic() - started
                if elapsed >= timeout_s:
                    status, reason = "TIMED_OUT", "task wall deadline exceeded"
                    returncode = _stop_process_group(proc, killpg=killpg, stop_scope=stop_scope)
                    break
                if log_path.stat().st_size > log_limit_bytes:
                    status, reason = "STOPPED_ON_LOG_LIMIT", "task log exceeded declared byte limit"
                    returncode = _stop_process_group(proc, killpg=killpg, stop_scope=stop_scope)
                    break
                if overflow.is_set():
                    status, reason = "STOPPED_ON_LOG_LIMIT", "task log exceeded declared byte limit"
                    returncode = _stop_process_group(proc, killpg=killpg, stop_scope=stop_scope)
                    break
                if _tree_bytes(task_output) > output_limit_bytes:
                    status, reason = "STOPPED_ON_OUTPUT_LIMIT", "task output exceeded declared byte limit"
                    returncode = _stop_process_group(proc, killpg=killpg)
                    break
                try:
                    returncode = proc.wait(timeout=min(POLL_INTERVAL_S, max(0.01, timeout_s - elapsed)))
                    status = "COMPLETED" if returncode == 0 else "FAILED"
                    if log_path.stat().st_size > log_limit_bytes:
                        status, reason = "STOPPED_ON_LOG_LIMIT", "task log exceeded declared byte limit"
                    elif _tree_bytes(task_output) > output_limit_bytes:
                        status, reason = "STOPPED_ON_OUTPUT_LIMIT", "task output exceeded declared byte limit"
                    break
                except subprocess.TimeoutExpired:
                    continue
        except KeyboardInterrupt:
            status, reason = "CANCELLED", "campaign interrupted by KeyboardInterrupt"
            returncode = _stop_process_group(proc, killpg=killpg, stop_scope=stop_scope)
        reader.join(timeout=KILL_GRACE_S)
        if reader.is_alive():
            if stop_scope is not None:
                try:
                    stop_scope()
                except (OSError, subprocess.SubprocessError):
                    pass
            try:
                killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            reader.join(timeout=KILL_GRACE_S)
            if status == "COMPLETED":
                status, reason = "STOPPED_ON_DESCENDANT", "process group retained the captured output pipe after command exit"
        if overflow.is_set():
            status, reason = "STOPPED_ON_LOG_LIMIT", "task log exceeded declared byte limit"
    return {"status": status, "returncode": returncode,
            "elapsed_s": time.monotonic() - started, "reason": reason,
            "timeout_s": timeout_s, "output_limit_bytes": output_limit_bytes,
            "log_limit_bytes": log_limit_bytes}


def run_campaign(plan: dict, *, executor=None, disk_usage=shutil.disk_usage) -> dict:
    out = Path(plan["output_dir"])
    if out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {out}")
    if sc.rm_m_geometry_dependencies(Path(plan["inputs"]["model_dir"])) != plan["inputs"]["model_dependencies"]:
        raise ValueError("RM-M model dependencies changed after campaign predeclaration")
    xs = Path(plan["inputs"]["cross_sections_xml"]["path"])
    if not xs.is_file() or sha256(xs) != plan["inputs"]["cross_sections_xml"]["sha256"]:
        raise ValueError("cross-section index changed after campaign predeclaration")
    for name, digest in plan["inputs"]["implementation_sha256"].items():
        path = RISK_DIR / name
        if not path.is_file() or sha256(path) != digest:
            raise ValueError(f"campaign implementation changed after predeclaration: {path}")
    systemd_run = plan["inputs"].get("systemd_run")
    if (not systemd_run or not Path(systemd_run).is_file() or
            sha256(Path(systemd_run)) != plan["inputs"].get("systemd_run_sha256")):
        raise RuntimeError("R3 campaign requires systemd-run for the existing 6 GiB memory/swap guard")
    python = Path(plan["inputs"]["python"])
    if not python.is_file() or sha256(python) != plan["inputs"]["python_sha256"]:
        raise ValueError("Python executable changed after campaign predeclaration")
    if executor is None:
        executor = execute_bounded
    out.mkdir(parents=True)
    plan_path = out / "campaign-plan.json"
    with plan_path.open("x", encoding="utf-8") as stream:
        json.dump(plan, stream, indent=2, sort_keys=True)
        stream.write("\n")
    env = os.environ.copy()
    env["PYTHONPATH"] = str(RISK_DIR) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    env["FARIS_CROSS_SECTIONS"] = plan["inputs"]["cross_sections_xml"]["path"]
    env["OMP_NUM_THREADS"] = "4"
    tasks = []
    d1s_script = str(RISK_DIR / "r3_d1s.py")
    r2s_script = str(RISK_DIR / "r3_r2s.py")
    compare_script = str(RISK_DIR / "r3_compare.py")
    model = plan["inputs"]["model_dir"]
    activation_groups = plan["standard_settings"]["r2s"]["activation_groups"]
    if activation_groups not in sc.ACTIVATION_GROUP_BASIS:
        raise ValueError(f"campaign plan names an unsupported R2S energy basis: {activation_groups}")
    for rep in plan["replicates"]:
        rep_name = f"rep-{rep['replicate']:02d}"
        d1s_dir = out / rep_name / "d1s"
        tasks.append({"kind": "d1s", "replicate": rep["replicate"], "out_dir": d1s_dir,
                      "command": [str(python), d1s_script, "--model-dir", model, "--out-dir", str(d1s_dir),
                                  "--threads", "4", "--seed", str(rep["d1s_seed"])]})
        for grid in rep["r2s"]:
            grid_dir = out / rep_name / f"r2s-{grid['activation_cell_cm']}cm"
            tasks.append({"kind": "r2s", "replicate": rep["replicate"], "activation_cell_cm": grid["activation_cell_cm"],
                          "out_dir": grid_dir,
                          "command": [str(python), r2s_script, "--model-dir", model, "--out-dir", str(grid_dir),
                                      "--activation-cell-cm", str(grid["activation_cell_cm"]),
                                      "--activation-groups", activation_groups,
                                      "--activation-backend", "bounded", "--threads", "4", "--seed", str(grid["cli_seed"])]})
            comparison = out / rep_name / f"comparison-{grid['activation_cell_cm']}cm.json"
            tasks.append({"kind": "compare", "replicate": rep["replicate"], "activation_cell_cm": grid["activation_cell_cm"],
                          "out_file": comparison,
                          "command": [str(python), compare_script, "--r2s", str(grid_dir), "--d1s", str(d1s_dir),
                                      "--out", str(comparison)]})
    guard_prefix = [systemd_run, "--user", "--scope", "--quiet", "-p", "MemoryMax=6G",
                    "-p", "MemorySwapMax=0"]
    systemctl = plan["inputs"].get("systemctl")
    if not systemctl or not Path(systemctl).is_file():
        raise RuntimeError("R3 campaign requires systemctl to stop named task scopes")
    state = {"schema": "faris-r3-whole-workflow-campaign-state/v1", "status": "RUNNING",
             "campaign_plan_sha256": sha256(plan_path),
             "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR", "tasks": []}
    state_path = out / "campaign-state.json"
    _write_json(state_path, state)
    logs = out / "logs"
    logs.mkdir()
    for index, task in enumerate(tasks, 1):
        timeout_s = TASK_LIMITS[task["kind"]]["timeout_s"]
        output_limit = TASK_LIMITS[task["kind"]]["output_bytes"]
        reserve_bytes = plan["execution_limits"]["disk_reserve_bytes_before_each_task"]
        required_free = output_limit + LOG_MAX_BYTES + reserve_bytes
        if disk_usage(out).free < required_free:
            state["status"] = "STOPPED_ON_DISK_RESERVE"
            state["failed_task_index"] = index
            state["error"] = f"requires {required_free} free bytes before task; current campaign volume is below reserve"
            _write_json(state_path, state)
            return state
        if not xs.is_file() or sha256(xs) != plan["inputs"]["cross_sections_xml"]["sha256"]:
            state["status"] = "STOPPED_ON_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = "cross-section index changed after campaign predeclaration"
            _write_json(state_path, state)
            return state
        if sc.rm_m_geometry_dependencies(Path(plan["inputs"]["model_dir"])) != plan["inputs"]["model_dependencies"]:
            state["status"] = "STOPPED_ON_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = "RM-M model dependencies changed after campaign predeclaration"
            _write_json(state_path, state)
            return state
        if not python.is_file() or sha256(python) != plan["inputs"]["python_sha256"]:
            state["status"] = "STOPPED_ON_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = "Python executable changed after campaign predeclaration"
            _write_json(state_path, state)
            return state
        if not Path(systemd_run).is_file() or sha256(Path(systemd_run)) != plan["inputs"]["systemd_run_sha256"]:
            state["status"] = "STOPPED_ON_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = "systemd-run executable changed after campaign predeclaration"
            _write_json(state_path, state)
            return state
        for name, digest in plan["inputs"]["implementation_sha256"].items():
            path = RISK_DIR / name
            if not path.is_file() or sha256(path) != digest:
                state["status"] = "STOPPED_ON_IMPLEMENTATION_CHANGE"
                state["failed_task_index"] = index
                state["error"] = f"campaign implementation changed after predeclaration: {path}"
                _write_json(state_path, state)
                return state
        log = logs / f"{index:03d}-{task['kind']}-rep{task['replicate']:02d}.log"
        task_output = task["out_dir"] if task["kind"] in ("d1s", "r2s") else task["out_file"]
        scope_unit = f"faris-r3-{state['campaign_plan_sha256'][:12]}-{os.getpid()}-{index}.scope"
        task_guard = guard_prefix[:1] + ["--unit", scope_unit] + guard_prefix[1:]
        started_at = datetime.now(timezone.utc)
        launch_record = {"task_index": index, "kind": task["kind"], "replicate": task["replicate"],
                         "command": task_guard + ["--"] + task["command"],
                         "scope_unit": scope_unit,
                         "scope_stop_command": [systemctl, "--user", "stop", scope_unit],
                         "child_command": task["command"], "log": str(log.relative_to(out)),
                         "task_output": str(task_output.relative_to(out)), "status": "STARTING",
                         "started_utc": started_at.isoformat(),
                         "deadline_utc": (started_at + timedelta(seconds=timeout_s)).isoformat(),
                         "deadline_s": timeout_s,
                         "output_limit_bytes": output_limit, "log_limit_bytes": LOG_MAX_BYTES,
                         "memory_max_bytes": MEMORY_MAX_BYTES, "disk_reserve_bytes": reserve_bytes,
                         "python_sha256": plan["inputs"]["python_sha256"],
                         "model_dependencies": plan["inputs"]["model_dependencies"],
                         "cross_sections_sha256": plan["inputs"]["cross_sections_xml"]["sha256"]}
        state["status"] = "RUNNING_TASK"
        state["current_task"] = launch_record
        _write_json(state_path, state)
        try:
            result = executor(task["command"], cwd=str(ROOT), env=env, log_path=log,
                              task_output=task_output, timeout_s=timeout_s,
                              output_limit_bytes=output_limit, guard_prefix=task_guard,
                              memory_max_bytes=MEMORY_MAX_BYTES, log_limit_bytes=LOG_MAX_BYTES,
                              scope_unit=scope_unit,
                              stop_scope=lambda: _stop_named_scope(systemctl, scope_unit))
        except KeyboardInterrupt:
            # execute_bounded normally handles this; an injected executor still leaves honest state.
            state["current_task"].update({"status": "CANCELLED", "finished_utc": datetime.now(timezone.utc).isoformat()})
            state["status"] = "CANCELLED"
            _write_json(state_path, state)
            return state
        row = {key: value for key, value in task.items() if key not in ("command", "out_dir", "out_file")}
        row.update({"command": task["command"], "log": str(log.relative_to(out)), **result})
        state["current_task"].update({"status": result["status"], "finished_utc": datetime.now(timezone.utc).isoformat(),
                                      "elapsed_s": result.get("elapsed_s"), "returncode": result.get("returncode"),
                                      "reason": result.get("reason")})
        if task["kind"] in ("d1s", "r2s"):
            record = task["out_dir"] / ("d1s_record.json" if task["kind"] == "d1s" else "r2s_record.json")
            row["record_path"] = str(record.relative_to(out))
            row["record_sha256"] = sha256(record) if record.is_file() else None
        else:
            row["comparison_path"] = str(task["out_file"].relative_to(out))
            row["comparison_sha256"] = sha256(task["out_file"]) if task["out_file"].is_file() else None
        state["tasks"].append(row)
        if result["status"] == "CANCELLED":
            state["status"] = "CANCELLED"
            state["failed_task_index"] = index
            _write_json(state_path, state)
            return state
        if result["status"] != "COMPLETED":
            state["status"] = "STOPPED_ON_TASK_LIMIT_OR_FAILURE"
            state["failed_task_index"] = index
            _write_json(state_path, state)
            return state
        if (task["kind"] in ("d1s", "r2s") and row["record_sha256"] is None) or \
                (task["kind"] == "compare" and row["comparison_sha256"] is None):
            state["status"] = "STOPPED_ON_MISSING_OUTPUT"
            state["failed_task_index"] = index
            _write_json(state_path, state)
            return state
        _write_json(state_path, state)
    state["status"] = "ALL_COMMANDS_EXITED_ZERO"
    _write_json(state_path, state)
    return state


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--cross-sections", required=True, type=Path)
    ap.add_argument("--python", required=True, type=Path, help="pinned OpenMC 0.15.3 Python executable")
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--replicates", required=True, type=int)
    ap.add_argument("--seed-base", required=True, type=int)
    ap.add_argument("--purpose", required=True, help="for example conditional-method-control")
    ap.add_argument("--activation-groups", choices=sc.ACTIVATION_GROUP_OPTIONS, default=sc.ACTIVATION_GROUPS,
                    help="frozen R2S activation energy basis; default preserves historical VITAMIN-J-42")
    args = ap.parse_args(argv)
    if args.out_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {args.out_dir}")
    plan = make_plan(model_dir=args.model_dir, cross_sections=args.cross_sections, python=args.python,
                     out_dir=args.out_dir, replicates=args.replicates, seed_base=args.seed_base,
                     purpose=args.purpose, activation_groups=args.activation_groups)
    result = run_campaign(plan)
    print(json.dumps({"status": result["status"], "scientific_verdict": result["scientific_verdict"],
                      "task_count": len(result["tasks"]), "output_dir": str(args.out_dir.resolve())}, indent=2))
    return 0 if result["status"] == "ALL_COMMANDS_EXITED_ZERO" else 1


if __name__ == "__main__":
    raise SystemExit(main())
