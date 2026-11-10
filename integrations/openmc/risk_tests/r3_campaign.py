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
import xml.etree.ElementTree as ET

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
IMPLEMENTATION_FILES = (
    "r3_campaign.py", "r3_d1s.py", "r3_r2s.py", "r3_compare.py",
    "r3_scenario.py", "r3_dose_statistics.py", "r3_shutdown_dose_export.py",
    "r3_bounded_microxs.py", "r3_activation_coverage.py", "rm_m_openmc.py",
)
CHAIN_PATH = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
MAX_XML_LIBRARY_REFERENCES = 10_000


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _cross_section_asset_manifest(xml_path: Path) -> list[dict]:
    """Hash every file referenced by every OpenMC library entry in the XML."""
    xml_path = Path(xml_path).resolve(strict=True)
    root = ET.parse(xml_path).getroot()
    directory_text = (root.findtext("directory") or "").strip()
    directory = Path(directory_text) if directory_text else xml_path.parent
    if not directory.is_absolute():
        directory = xml_path.parent / directory
    directory = directory.resolve()
    libraries = root.findall("library")
    if len(libraries) > MAX_XML_LIBRARY_REFERENCES:
        raise ValueError("cross_sections.xml has too many library references for a bounded campaign manifest")
    assets: dict[str, dict] = {}
    for library in libraries:
        kind = (library.get("type") or "unspecified").strip()
        reference = library.get("path")
        if not reference:
            raise ValueError(f"cross_sections.xml {kind} library entry has no path")
        path = Path(reference)
        if not path.is_absolute():
            path = directory / path
        path = path.resolve(strict=True)
        if not path.is_file():
            raise ValueError(f"cross_sections.xml library path must resolve to a file: {path}")
        asset = path
        key = str(asset)
        if key not in assets:
            assets[key] = {"path": key, "sha256": sha256(asset), "size_bytes": asset.stat().st_size,
                           "library_types": []}
        if kind not in assets[key]["library_types"]:
            assets[key]["library_types"].append(kind)
    if not assets:
        raise ValueError("cross_sections.xml contains no library assets to bind")
    return [dict(assets[path], library_types=sorted(assets[path]["library_types"]))
            for path in sorted(assets)]


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
    chain_path = Path(CHAIN_PATH).resolve(strict=True)
    cross_section_assets = _cross_section_asset_manifest(cross_sections)
    scripts = {name: RISK_DIR / name for name in IMPLEMENTATION_FILES}
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
                   "cross_section_data_assets": cross_section_assets,
                   "activation_chain": {"path": str(chain_path), "sha256": sha256(chain_path)},
                   "python": str(python), "python_sha256": sha256(python),
            "systemd_run": shutil.which("systemd-run"),
            "systemd_run_sha256": sha256(Path(shutil.which("systemd-run")))
                if shutil.which("systemd-run") else None,
            "systemctl": shutil.which("systemctl"),
            "systemctl_sha256": sha256(Path(shutil.which("systemctl")))
                if shutil.which("systemctl") else None,
            "implementation_sha256": script_hashes},
        "output_dir": str(out_dir),
        "comparison_note": "r3_compare.py is run unchanged for each replicate/grid pair. It reports unknown D1S/R2S covariance as inconclusive; no campaign-level uncertainty or pass gate is added.",
    }


def _write_json(path: Path, value: dict) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(path)


def write_plan_only(plan: dict) -> Path:
    """Persist an immutable campaign plan without launching a task."""
    out = Path(plan["output_dir"])
    if out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {out}")
    out.mkdir(parents=True)
    path = out / "campaign-plan.json"
    with path.open("x", encoding="utf-8") as stream:
        json.dump(plan, stream, indent=2, sort_keys=True)
        stream.write("\n")
    return path


def _validate_frozen_inputs(plan: dict, *, resume_python: Path | None = None,
                            verify_launchers: bool = True) -> None:
    """Recheck the frozen model, data, helpers, and launcher runtimes."""
    if sc.rm_m_geometry_dependencies(Path(plan["inputs"]["model_dir"])) != plan["inputs"]["model_dependencies"]:
        raise ValueError("RM-M model dependencies changed after campaign predeclaration")
    xs = Path(plan["inputs"]["cross_sections_xml"]["path"])
    if not xs.is_file() or sha256(xs) != plan["inputs"]["cross_sections_xml"]["sha256"]:
        raise ValueError("cross-section index changed after campaign predeclaration")
    if "cross_section_data_assets" not in plan["inputs"] or "activation_chain" not in plan["inputs"]:
        raise ValueError("campaign plan predates referenced nuclear-data identity binding; create a new immutable plan")
    try:
        current_assets = _cross_section_asset_manifest(xs)
    except (OSError, ValueError, ET.ParseError) as exc:
        raise ValueError(f"cross-section data assets are unavailable after campaign predeclaration: {exc}") from exc
    if current_assets != plan["inputs"].get("cross_section_data_assets"):
        raise ValueError("cross-section data assets changed after campaign predeclaration")
    chain = plan["inputs"].get("activation_chain")
    if (not isinstance(chain, dict) or not isinstance(chain.get("path"), str) or
            not isinstance(chain.get("sha256"), str)):
        raise ValueError("campaign plan lacks the shared activation-chain identity")
    chain_path = Path(chain["path"])
    if not chain_path.is_file() or sha256(chain_path) != chain["sha256"]:
        raise ValueError("shared activation/decay chain changed after campaign predeclaration")
    implementation_hashes = plan["inputs"].get("implementation_sha256")
    if (not isinstance(implementation_hashes, dict) or
            set(implementation_hashes) != set(IMPLEMENTATION_FILES)):
        raise ValueError("campaign implementation identity list differs from the canonical helper set")
    for name in implementation_hashes:
        if Path(name).name != name or Path(name).is_absolute() or ".." in Path(name).parts:
            raise ValueError(f"campaign implementation identity contains a noncanonical helper name: {name}")
    for name, digest in implementation_hashes.items():
        path = RISK_DIR / name
        if not path.is_file() or sha256(path) != digest:
            raise ValueError(f"campaign implementation changed after predeclaration: {path}")
    if verify_launchers:
        for label, command in (("systemd_run", "systemd-run"), ("systemctl", "systemctl")):
            executable = plan["inputs"].get(label)
            expected = plan["inputs"].get(f"{label}_sha256")
            current = shutil.which(command)
            if not current or not executable or not expected:
                raise ValueError(f"campaign {label} runtime is unavailable after predeclaration")
            current_path = Path(current).resolve()
            frozen_path = Path(executable).resolve()
            if (current_path != frozen_path or not current_path.is_file() or
                    sha256(current_path) != expected):
                raise ValueError(f"campaign {label} runtime changed or is unavailable after predeclaration")
    python = Path(plan["inputs"]["python"]).resolve()
    if resume_python is not None and Path(resume_python).resolve() != python:
        raise ValueError("resume Python executable path differs from the frozen campaign plan")
    if not python.is_file() or sha256(python) != plan["inputs"]["python_sha256"]:
        raise ValueError("Python executable changed after campaign predeclaration")


def _planned_tasks(plan: dict) -> list[dict]:
    tasks = []
    d1s_script = str(RISK_DIR / "r3_d1s.py")
    r2s_script = str(RISK_DIR / "r3_r2s.py")
    compare_script = str(RISK_DIR / "r3_compare.py")
    model = plan["inputs"]["model_dir"]
    python = Path(plan["inputs"]["python"])
    activation_groups = plan["standard_settings"]["r2s"]["activation_groups"]
    if activation_groups not in sc.ACTIVATION_GROUP_BASIS:
        raise ValueError(f"campaign plan names an unsupported R2S energy basis: {activation_groups}")
    out = Path(plan["output_dir"])
    for rep in plan["replicates"]:
        rep_name = f"rep-{rep['replicate']:02d}"
        d1s_dir = out / rep_name / "d1s"
        tasks.append({"kind": "d1s", "replicate": rep["replicate"], "seed": rep["d1s_seed"],
                      "out_dir": d1s_dir,
                      "command": [str(python), d1s_script, "--model-dir", model, "--out-dir", str(d1s_dir),
                                  "--threads", "4", "--seed", str(rep["d1s_seed"])]})
        for grid in rep["r2s"]:
            grid_dir = out / rep_name / f"r2s-{grid['activation_cell_cm']}cm"
            tasks.append({"kind": "r2s", "replicate": rep["replicate"],
                          "activation_cell_cm": grid["activation_cell_cm"], "seed": grid["cli_seed"],
                          "out_dir": grid_dir,
                          "command": [str(python), r2s_script, "--model-dir", model, "--out-dir", str(grid_dir),
                                      "--activation-cell-cm", str(grid["activation_cell_cm"]),
                                      "--activation-groups", activation_groups,
                                      "--activation-backend", "bounded", "--threads", "4", "--seed", str(grid["cli_seed"])]})
            comparison = out / rep_name / f"comparison-{grid['activation_cell_cm']}cm.json"
            tasks.append({"kind": "compare", "replicate": rep["replicate"],
                          "activation_cell_cm": grid["activation_cell_cm"], "seed": grid["cli_seed"],
                          "out_file": comparison,
                          "command": [str(python), compare_script, "--r2s", str(grid_dir), "--d1s", str(d1s_dir),
                                      "--out", str(comparison)]})
    return tasks


def _receipt_artifacts(record: dict, output_dir: Path) -> list[dict]:
    """Collect and validate receipt-declared files located inside a task output directory."""
    output_dir = output_dir.resolve()
    refs: dict[str, str] = {}

    def add(raw_path, expected):
        if not isinstance(raw_path, str) or not isinstance(expected, str) or len(expected) != 64:
            return
        path = Path(raw_path)
        absolute = path.is_absolute()
        resolved = path.resolve() if absolute else (output_dir / path).resolve()
        try:
            relative = resolved.relative_to(output_dir)
        except ValueError:
            if absolute:
                return  # Frozen source/data inputs are checked separately by the campaign plan.
            raise ValueError(f"receipt artifact path escapes task output directory: {raw_path}")
        if not resolved.is_file() or sha256(resolved) != expected:
            raise ValueError(f"completed task receipt artifact is missing or changed: {relative}")
        refs[relative.as_posix()] = expected

    def visit(value):
        if isinstance(value, dict):
            if "path" in value and "sha256" in value:
                add(value["path"], value["sha256"])
            if "batch_values_file" in value and "batch_values_sha256" in value:
                add(value["batch_values_file"], value["batch_values_sha256"])
            for item in value.values():
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)

    visit(record)
    return [{"path": name, "sha256": refs[name]} for name in sorted(refs)]


def _verify_task_output(task: dict, output_root: Path) -> tuple[str, list[dict]]:
    """Validate a completed task's receipt and every receipt-hashed local artifact."""
    output_root = output_root.resolve()
    if task["kind"] == "compare":
        path = task["out_file"]
        if not path.is_file():
            raise ValueError(f"completed comparison output is missing: {path}")
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"completed comparison output is not valid JSON: {path}") from exc
        if value.get("schema") != "faris-r3/v1":
            raise ValueError(f"completed comparison output has an unexpected schema: {path}")
        return sha256(path), []

    record_name = "d1s_record.json" if task["kind"] == "d1s" else "r2s_record.json"
    record_path = task["out_dir"] / record_name
    if not record_path.is_file():
        raise ValueError(f"completed task receipt is missing: {record_path}")
    try:
        record = json.loads(record_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"completed task receipt is not valid JSON: {record_path}") from exc
    if task["kind"] == "d1s":
        run = record.get("run", {})
        if (record.get("kind") != "r3-d1s" or record.get("seed") != task["seed"] or
                run.get("stop_reason") not in ("batch_limit", "time_budget") or
                run.get("integrity", {}).get("lost_particle_assessment", {}).get("status") != "PASS" or
                any(label not in record.get("shutdown_dose", {}) for label in sc.COOLING_LABELS.values())):
            raise ValueError(f"completed D1S receipt does not match its planned seed or a complete run: {record_path}")
    else:
        settings = record.get("settings", {})
        if (record.get("kind") != "r3-r2s" or record.get("threads") != 4 or
                settings.get("activation_cell_cm") != float(task["activation_cell_cm"]) or
                settings.get("seeds", [None])[0] != task["seed"] or
                settings.get("activation_groups") != plan_group_basis(task) or
                record.get("activation_backend") != "bounded" or
                any(record.get("steps", {}).get(name, {}).get("ok") is not True
                    for name in ("neutron_transport", "activation", "photon_transport")) or
                not record.get("activation_microxs") or
                record.get("activation_microxs", {}).get("status") == "NOT_RUN" or
                any(label not in record.get("photon", {}) for label in sc.COOLING_LABELS.values())):
            raise ValueError(f"completed R2S receipt does not match its planned grid/seed or complete workflow: {record_path}")
    artifact_refs = _receipt_artifacts(record, task["out_dir"])
    # These voxel arrays are the exact inputs consumed by the later comparison and
    # currently are not individually hash-listed in the scientific run record.
    for index in sc.PHOTON_TIME_INDICES:
        path = task["out_dir"] / f"dose_time_{index}.npz"
        if not path.is_file():
            raise ValueError(f"completed task comparison input is missing: {path}")
        artifact_refs.append({"path": path.relative_to(task["out_dir"]).as_posix(), "sha256": sha256(path)})
    deduped = {item["path"]: item["sha256"] for item in artifact_refs}
    return sha256(record_path), [{"path": name, "sha256": deduped[name]} for name in sorted(deduped)]


def plan_group_basis(task: dict) -> str:
    # The basis is a command argument and also appears in the generated R2S receipt.
    command = task["command"]
    return command[command.index("--activation-groups") + 1]


def _verify_completed_prefix(tasks: list[dict], state: dict, out: Path) -> None:
    rows = state.get("tasks", [])
    if len(rows) > len(tasks):
        raise ValueError("campaign state contains more completed tasks than the frozen plan")
    if state.get("current_task") and state["current_task"].get("status") not in ("COMPLETED", None):
        raise ValueError("campaign has an unfinished task; inspect its preserved output before resuming")
    for index, (task, row) in enumerate(zip(tasks, rows), 1):
        expected_task = {key: value for key, value in task.items() if key not in ("command", "out_dir", "out_file")}
        for key, value in expected_task.items():
            if row.get(key) != value:
                raise ValueError(f"campaign state task {index} does not match frozen task plan")
        if row.get("status") != "COMPLETED" or row.get("returncode") != 0:
            raise ValueError(f"campaign task {index} did not complete successfully; refusing to skip or overwrite it")
        if row.get("task_index") != index:
            raise ValueError(f"campaign state task index {index} does not match its position in the frozen plan")
        if row.get("command") != task["command"]:
            raise ValueError(f"campaign task {index} command changed from the frozen plan")
        log_path = out / row.get("log", "")
        if row.get("log") and not log_path.is_file():
            raise ValueError(f"campaign task {index} log is missing: {log_path}")
        if not row.get("log_sha256") or sha256(log_path) != row["log_sha256"]:
            raise ValueError(f"campaign task {index} log changed: {log_path}")
        if task["kind"] == "compare":
            path = task["out_file"]
            expected_hash = row.get("comparison_sha256")
            if not path.is_file() or not expected_hash or sha256(path) != expected_hash:
                raise ValueError(f"completed comparison output changed: {path}")
            _verify_task_output(task, out)
        else:
            path = task["out_dir"] / ("d1s_record.json" if task["kind"] == "d1s" else "r2s_record.json")
            expected_hash = row.get("record_sha256")
            if not path.is_file() or not expected_hash or sha256(path) != expected_hash:
                raise ValueError(f"completed task receipt changed: {path}")
            record_hash, artifacts = _verify_task_output(task, out)
            if record_hash != expected_hash or artifacts != row.get("receipt_artifacts"):
                raise ValueError(f"completed task output/receipt artifacts changed: {path.parent}")


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
                    returncode = _stop_process_group(proc, killpg=killpg, stop_scope=stop_scope)
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


def run_campaign(plan: dict, *, executor=None, disk_usage=shutil.disk_usage,
                 max_tasks: int | None = None, resume: bool = False,
                 resume_python: Path | None = None) -> dict:
    if max_tasks is not None and max_tasks < 1:
        raise ValueError("--max-tasks must be a positive integer")
    out = Path(plan["output_dir"]).resolve()
    plan_path = out / "campaign-plan.json"
    state_path = out / "campaign-state.json"
    if resume:
        if not out.is_dir() or not plan_path.is_file():
            raise FileNotFoundError(f"resume requires the existing frozen campaign plan: {plan_path}")
    elif out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 campaign directory: {out}")
    if resume and resume_python is None:
        resume_python = Path(sys.executable)
    if executor is None:
        executor = execute_bounded
    _validate_frozen_inputs(plan, resume_python=resume_python if resume else None,
                            verify_launchers=executor is execute_bounded)
    if resume:
        if not out.is_dir() or not plan_path.is_file():
            raise FileNotFoundError(f"resume requires the existing frozen campaign plan: {plan_path}")
        try:
            stored_plan = json.loads(plan_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"existing campaign plan is unreadable: {plan_path}") from exc
        if stored_plan != plan:
            raise ValueError("resume plan differs from the immutable campaign-plan.json")
        plan_digest = sha256(plan_path)
        if state_path.is_file():
            try:
                state = json.loads(state_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                raise ValueError(f"existing campaign state is unreadable: {state_path}") from exc
            if state.get("campaign_plan_sha256") != plan_digest:
                raise ValueError("campaign state does not bind the existing frozen plan")
            if state.get("status") not in ("PREDECLARED", "IN_PROGRESS", "ALL_COMMANDS_EXITED_ZERO"):
                raise ValueError(f"campaign status {state.get('status')} is not resumable; preserve and inspect its failure")
        else:
            state = {"schema": "faris-r3-whole-workflow-campaign-state/v1", "status": "PREDECLARED",
                     "campaign_plan_sha256": plan_digest,
                     "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR", "tasks": []}
            _write_json(state_path, state)
    else:
        out.mkdir(parents=True)
        with plan_path.open("x", encoding="utf-8") as stream:
            json.dump(plan, stream, indent=2, sort_keys=True)
            stream.write("\n")
        state = {"schema": "faris-r3-whole-workflow-campaign-state/v1", "status": "PREDECLARED",
                 "campaign_plan_sha256": sha256(plan_path),
                 "scientific_verdict": "NOT_EVALUATED_BY_ORCHESTRATOR", "tasks": []}
        _write_json(state_path, state)

    tasks = _planned_tasks(plan)
    _verify_completed_prefix(tasks, state, out)
    if state["status"] == "ALL_COMMANDS_EXITED_ZERO":
        if len(state["tasks"]) != len(tasks):
            raise ValueError("completed campaign state does not contain the full frozen task matrix")
        return state

    systemd_run = plan["inputs"]["systemd_run"]
    systemctl = plan["inputs"]["systemctl"]
    python = Path(plan["inputs"]["python"])
    guard_prefix = [systemd_run, "--user", "--scope", "--quiet", "-p", "MemoryMax=6G",
                    "-p", "MemorySwapMax=0"]
    env = os.environ.copy()
    env["PYTHONPATH"] = str(RISK_DIR) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    env["FARIS_CROSS_SECTIONS"] = plan["inputs"]["cross_sections_xml"]["path"]
    env["OMP_NUM_THREADS"] = "4"
    logs = out / "logs"
    logs.mkdir(exist_ok=True)
    first_new_index = len(state["tasks"]) + 1
    tasks_this_call = 0
    for index, task in enumerate(tasks[first_new_index - 1:], first_new_index):
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
        try:
            _validate_frozen_inputs(plan, resume_python=resume_python if resume else None,
                                    verify_launchers=executor is execute_bounded)
        except (ValueError, RuntimeError, OSError) as exc:
            state["status"] = "STOPPED_ON_FROZEN_INPUT_CHANGE"
            state["failed_task_index"] = index
            state["error"] = str(exc)
            _write_json(state_path, state)
            return state
        log = logs / f"{index:03d}-{task['kind']}-rep{task['replicate']:02d}.log"
        task_output = task["out_dir"] if task["kind"] in ("d1s", "r2s") else task["out_file"]
        if task_output.exists():
            state["status"] = "STOPPED_ON_EXISTING_TASK_OUTPUT"
            state["failed_task_index"] = index
            state["error"] = f"refusing to overwrite unexpected task output: {task_output}"
            _write_json(state_path, state)
            return state
        if log.exists():
            state["status"] = "STOPPED_ON_EXISTING_TASK_LOG"
            state["failed_task_index"] = index
            state["error"] = f"refusing to overwrite unexpected task log: {log}"
            _write_json(state_path, state)
            return state
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
        if result.get("status") == "COMPLETED":
            try:
                _validate_frozen_inputs(plan, resume_python=resume_python if resume else None,
                                        verify_launchers=executor is execute_bounded)
            except (ValueError, RuntimeError, OSError) as exc:
                state["current_task"].update({
                    "status": "COMPLETED_BUT_FROZEN_INPUT_CHANGED",
                    "finished_utc": datetime.now(timezone.utc).isoformat(),
                    "elapsed_s": result.get("elapsed_s"), "returncode": result.get("returncode"),
                    "reason": str(exc)})
                state["status"] = "STOPPED_ON_FROZEN_INPUT_CHANGE"
                state["failed_task_index"] = index
                state["error"] = str(exc)
                _write_json(state_path, state)
                return state
        row = {key: value for key, value in task.items() if key not in ("command", "out_dir", "out_file")}
        row["task_index"] = index
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
        if log.is_file():
            row["log_sha256"] = sha256(log)
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
        if not log.is_file() or not row.get("log_sha256"):
            state["status"] = "STOPPED_ON_MISSING_OUTPUT"
            state["failed_task_index"] = index
            state["error"] = f"completed task did not produce its recorded log: {log}"
            _write_json(state_path, state)
            return state
        try:
            output_sha, artifacts = _verify_task_output(task, out)
            if task["kind"] == "compare":
                if output_sha != row["comparison_sha256"]:
                    raise ValueError("comparison output changed during task finalization")
            else:
                if output_sha != row["record_sha256"]:
                    raise ValueError("task receipt changed during finalization")
                row["receipt_artifacts"] = artifacts
        except ValueError as exc:
            state["status"] = "STOPPED_ON_INVALID_TASK_OUTPUT"
            state["failed_task_index"] = index
            state["error"] = str(exc)
            _write_json(state_path, state)
            return state
        _write_json(state_path, state)
        tasks_this_call += 1
        if max_tasks is not None and tasks_this_call >= max_tasks and index < len(tasks):
            state["status"] = "IN_PROGRESS"
            state["current_task"] = None
            state["completed_task_count"] = len(state["tasks"])
            state["remaining_task_count"] = len(tasks) - len(state["tasks"])
            state["last_batch_max_tasks"] = max_tasks
            state["last_batch_completed_tasks"] = tasks_this_call
            state["last_batch_finished_utc"] = datetime.now(timezone.utc).isoformat()
            _write_json(state_path, state)
            return state
    state["status"] = "ALL_COMMANDS_EXITED_ZERO"
    state["current_task"] = None
    state["completed_task_count"] = len(state["tasks"])
    state["remaining_task_count"] = 0
    _write_json(state_path, state)
    return state


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", type=Path)
    ap.add_argument("--cross-sections", type=Path)
    ap.add_argument("--python", type=Path, help="pinned OpenMC 0.15.3 Python executable")
    ap.add_argument("--out-dir", type=Path)
    ap.add_argument("--replicates", type=int)
    ap.add_argument("--seed-base", type=int)
    ap.add_argument("--purpose", help="for example conditional-method-control")
    ap.add_argument("--activation-groups", choices=sc.ACTIVATION_GROUP_OPTIONS,
                    help="frozen R2S activation energy basis; default preserves historical VITAMIN-J-42")
    ap.add_argument("--plan-only", action="store_true", help="write the immutable plan and exit before any external task")
    ap.add_argument("--max-tasks", type=int,
                    help="run at most this many new matrix tasks, then save IN_PROGRESS for a later resume")
    ap.add_argument("--resume-from-existing-plan", type=Path,
                    help="resume the output directory whose frozen campaign-plan.json this path names")
    args = ap.parse_args(argv)
    if args.max_tasks is not None and args.max_tasks < 1:
        ap.error("--max-tasks must be a positive integer")
    if args.resume_from_existing_plan is not None:
        if args.plan_only:
            ap.error("--plan-only cannot be combined with --resume-from-existing-plan")
        if any(value is not None for value in (args.model_dir, args.cross_sections, args.out_dir,
                                                args.replicates, args.seed_base, args.purpose, args.activation_groups)):
            ap.error("resume uses the frozen plan; omit new-plan options (use --python only to verify its pinned runtime)")
        plan_path = args.resume_from_existing_plan.resolve()
        if plan_path.name != "campaign-plan.json" or not plan_path.is_file():
            ap.error("--resume-from-existing-plan must name an existing campaign-plan.json")
        try:
            plan = json.loads(plan_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            ap.error(f"cannot read frozen plan: {exc}")
        expected_path = Path(plan.get("output_dir", "")).resolve() / "campaign-plan.json"
        if plan_path != expected_path:
            ap.error("resume plan must be the campaign-plan.json inside its own output directory")
        result = run_campaign(plan, max_tasks=args.max_tasks, resume=True,
                              resume_python=args.python or Path(sys.executable))
        output_dir = plan["output_dir"]
    else:
        missing = [flag for flag, value in (("--model-dir", args.model_dir),
                                            ("--cross-sections", args.cross_sections),
                                            ("--python", args.python), ("--out-dir", args.out_dir),
                                            ("--replicates", args.replicates),
                                            ("--seed-base", args.seed_base), ("--purpose", args.purpose))
                   if value is None]
        if missing:
            ap.error("new campaign requires " + ", ".join(missing))
        if args.plan_only and args.max_tasks is not None:
            ap.error("--max-tasks applies when executing or resuming, not during --plan-only")
        plan = make_plan(model_dir=args.model_dir, cross_sections=args.cross_sections, python=args.python,
                         out_dir=args.out_dir, replicates=args.replicates, seed_base=args.seed_base,
                         purpose=args.purpose, activation_groups=args.activation_groups or sc.ACTIVATION_GROUPS)
        if args.plan_only:
            path = write_plan_only(plan)
            print(json.dumps({"status": "PREDECLARED", "campaign_plan": str(path),
                              "scientific_verdict": plan["scientific_verdict"]}, indent=2))
            return 0
        result = run_campaign(plan, max_tasks=args.max_tasks)
        output_dir = str(args.out_dir.resolve())
    print(json.dumps({"status": result["status"], "scientific_verdict": result["scientific_verdict"],
                      "completed_task_count": len(result["tasks"]),
                      "remaining_task_count": result.get("remaining_task_count", 0),
                      "output_dir": str(output_dir)}, indent=2))
    return 0 if result["status"] in ("ALL_COMMANDS_EXITED_ZERO", "IN_PROGRESS") else 1


if __name__ == "__main__":
    raise SystemExit(main())
