#!/usr/bin/env python3
"""Run bounded, separate-process PROCESS and bluemira LifeCycle peer cases.

FARIS is not replaced by this code. The peer programs produce their own output;
this module validates provenance and launches the documented peer entrypoints.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import select
import signal
if os.name == "posix":
    import resource
else:  # pragma: no cover - external fusion tool support is currently Linux-first
    resource = None
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any


SCHEMA = "faris-peer-lifecycle-comparison/0.1.0"
SUPPORTED_TOOL_KEYS = {"process", "bluemira"}
MAX_CAPTURE = 64_000


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def relative_file(base: Path, value: Any, expected_hash: Any, label: str) -> Path:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} path must be a non-empty relative path")
    rel = Path(value)
    if rel.is_absolute() or ".." in rel.parts:
        raise ValueError(f"{label} path must stay relative to the manifest")
    path = (base / rel).resolve()
    if not path.is_relative_to(base.resolve()) or not path.is_file():
        raise ValueError(f"{label} is missing or outside the manifest directory")
    if not isinstance(expected_hash, str) or sha256(path) != expected_hash:
        raise ValueError(f"{label} SHA-256 mismatch")
    return path


def validate_source(value: Any, label: str) -> None:
    if not isinstance(value, dict) or set(value) != {"kind", "reference"}:
        raise ValueError(f"{label} needs source metadata with kind and reference")
    if value["kind"] not in {"authored", "literature", "measurement", "FARIS-output", "peer-output"}:
        raise ValueError(f"{label} has an unknown provenance kind")
    if not isinstance(value["reference"], str) or not value["reference"].strip():
        raise ValueError(f"{label} provenance reference must be non-empty")


def validate_quantity(value: Any, expected_unit: str, label: str) -> None:
    if not isinstance(value, dict) or set(value) != {"value", "unit", "source"}:
        raise ValueError(f"{label} must contain value, unit and source")
    raw = value["value"]
    if isinstance(raw, bool) or not isinstance(raw, (int, float)) or not math.isfinite(raw):
        raise ValueError(f"{label} value must be a finite number")
    if value["unit"] != expected_unit:
        raise ValueError(f"{label} unit must be {expected_unit}")
    validate_source(value["source"], label)


def validate_downtime_mapping(value: Any, label: str) -> None:
    """Require explicit mappings; never turn an absent model feature into zero."""
    categories = {"planned_replacement", "pulse_ramp_and_recharge", "unplanned", "other"}
    if not isinstance(value, dict) or set(value) != categories:
        raise ValueError(f"{label} must explicitly map all downtime categories: {sorted(categories)}")
    for category, mapping in value.items():
        if not isinstance(mapping, dict) or set(mapping) != {"disposition", "native_setting", "note"}:
            raise ValueError(f"{label}.{category} needs disposition, native_setting and note")
        if mapping["disposition"] not in {"matched", "excluded_by_native_setting", "unmapped"}:
            raise ValueError(f"{label}.{category} has unsupported disposition")
        if mapping["disposition"] != "unmapped":
            native = mapping["native_setting"]
            if not isinstance(native, dict) or set(native) != {"field", "value", "unit"}:
                raise ValueError(f"{label}.{category} needs a machine-checkable native field, value and unit")
            if not isinstance(native["field"], str) or not native["field"].strip() or not isinstance(native["unit"], str) or not native["unit"].strip():
                raise ValueError(f"{label}.{category} native field and unit must be non-empty")
        if mapping["disposition"] == "unmapped" and not str(mapping["note"]).strip():
            raise ValueError(f"{label}.{category} needs a visible mapping limitation")


def validate_native_mapping(value: Any, label: str, unit: str | None = None, expected: Any = None) -> None:
    required = {"disposition", "native_field", "value", "unit", "note"}
    if not isinstance(value, dict) or set(value) != required:
        raise ValueError(f"{label} needs disposition, native_field, value, unit and note")
    if value["disposition"] not in {"matched", "unmapped"}:
        raise ValueError(f"{label} disposition must be matched or unmapped")
    if value["disposition"] == "unmapped":
        if not isinstance(value["note"], str) or not value["note"].strip():
            raise ValueError(f"{label} needs a visible reason for its unmapped status")
        if value["native_field"] is not None or value["value"] is not None or value["unit"] is not None:
            raise ValueError(f"{label} unmapped fields must not claim a native setting")
        return
    if not isinstance(value["native_field"], str) or not value["native_field"].strip():
        raise ValueError(f"{label} mapped disposition needs an actual native field or JSON pointer")
    if value["unit"] != unit:
        raise ValueError(f"{label} mapped unit must be {unit}")
    if isinstance(expected, (int, float)):
        actual = value["value"]
        if isinstance(actual, bool) or not isinstance(actual, (int, float)) or not math.isclose(float(actual), float(expected), rel_tol=0, abs_tol=1e-12):
            raise ValueError(f"{label} mapped value does not equal the matched input")
    elif value["value"] != expected:
        raise ValueError(f"{label} mapped value does not equal the matched input")
    if not isinstance(value["note"], str) or not value["note"].strip():
        raise ValueError(f"{label} needs a note identifying the native correspondence")


def process_input_assignments(path: Path) -> dict[str, float]:
    import re
    assignments: dict[str, float] = {}
    pattern = re.compile(r"^\s*([A-Za-z_]\w*)\s*=\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[EeDd][+-]?\d+)?)")
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = pattern.match(line)
        if match:
            assignments[match.group(1).casefold()] = float(match.group(2).replace("D", "E").replace("d", "e"))
    return assignments


def json_pointer(document: Any, pointer: str) -> Any:
    if not pointer.startswith("/"):
        raise ValueError("bluemira native_field must be a JSON pointer beginning with '/'")
    current = document
    for part in pointer[1:].split("/"):
        key = part.replace("~1", "/").replace("~0", "~")
        if isinstance(current, list):
            current = current[int(key)]
        else:
            current = current[key]
    return current


def verify_native_input_file(value: Any, expected: Any, unit: str, input_path: Path, tool: str, label: str) -> None:
    validate_native_mapping(value, label, unit, expected)
    if value["disposition"] == "unmapped":
        return
    fields = process_input_assignments(input_path) if tool == "process" else None
    if fields is not None:
        native = value["native_field"].casefold()
        actual = fields.get(native)
        if actual is None or not math.isclose(actual, float(expected), rel_tol=0, abs_tol=1e-12):
            raise ValueError(f"{label} does not match PROCESS IN.DAT field {value['native_field']}")
    else:
        config = read_json(input_path)
        try:
            actual = json_pointer(config, value["native_field"])
        except (KeyError, IndexError, TypeError, ValueError) as error:
            raise ValueError(f"{label} cannot resolve bluemira config field {value['native_field']}: {error}") from error
        if isinstance(actual, bool) or not isinstance(actual, (int, float)) or not math.isclose(float(actual), float(expected), rel_tol=0, abs_tol=1e-12):
            raise ValueError(f"{label} does not match bluemira config field {value['native_field']}")


def verify_setting(native: dict[str, Any], input_path: Path, tool: str, label: str) -> None:
    if tool == "process":
        actual = process_input_assignments(input_path).get(native["field"].casefold())
        if actual is None or isinstance(native["value"], bool) or not isinstance(native["value"], (int, float)) or not math.isclose(actual, float(native["value"]), rel_tol=0, abs_tol=1e-12):
            raise ValueError(f"{label} does not match PROCESS IN.DAT field {native['field']}")
    else:
        try:
            actual = json_pointer(read_json(input_path), native["field"])
        except (KeyError, IndexError, TypeError, ValueError) as error:
            raise ValueError(f"{label} cannot resolve bluemira field {native['field']}: {error}") from error
        expected = native["value"]
        if isinstance(expected, (int, float)) and not isinstance(expected, bool):
            if not isinstance(actual, (int, float)) or isinstance(actual, bool) or not math.isclose(float(actual), float(expected), rel_tol=0, abs_tol=1e-12):
                raise ValueError(f"{label} does not match bluemira field {native['field']}")
        elif actual != expected:
            raise ValueError(f"{label} does not match bluemira field {native['field']}")


def native_input_limitations(map_entry: dict[str, Any], tool: str, matched: dict[str, Any]) -> list[str]:
    limitations = []
    fields = map_entry["native_inputs"]
    for key, label, unit in (("plant_horizon_years", "plant horizon", "year"),
                             ("net_electric_power_mw", "net electric power", "MW(e)"),
                             ("parallel_remote_handling_systems", "parallel remote-handling systems", "count")):
        value = fields[key]
        validate_native_mapping(value, f"mapping.{tool}.{key}", unit, matched[key]["value"])
        if value["disposition"] == "unmapped":
            limitations.append(f"{tool}.{label}: {value['note']}")
    components = fields["component_lifetimes_years"]
    expected = {entry["component"]: entry for entry in matched["component_lifetimes_years"]}
    if not isinstance(components, list) or {c.get("component") for c in components if isinstance(c, dict)} != set(expected):
        raise ValueError(f"mapping.{tool}.component_lifetimes_years must account for exactly the included components")
    for component in components:
        name = component["component"]
        life = component.get("lifetime")
        replacement = component.get("replacement_duration")
        if not isinstance(life, dict) or not isinstance(replacement, dict):
            raise ValueError(f"mapping.{tool}.{name} must map its lifetime and replacement duration separately")
        validate_native_mapping(life, f"mapping.{tool}.{name}.lifetime", "year", expected[name]["lifetime"]["value"])
        validate_native_mapping(replacement, f"mapping.{tool}.{name}.replacement_duration", "day", expected[name]["replacement_duration_days"]["value"])
        if life["disposition"] == "unmapped":
            limitations.append(f"{tool}.{name} lifetime: {life['note']}")
        if replacement["disposition"] == "unmapped":
            limitations.append(f"{tool}.{name} replacement duration: {replacement['note']}")
    return limitations


def validate_manifest(path: Path) -> tuple[dict[str, Any], dict[str, Path], list[str]]:
    manifest = read_json(path)
    if not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA:
        raise ValueError(f"manifest schema must be {SCHEMA}")
    required = {"schema", "comparison_id", "matched_inputs", "mapping", "faris", "peers", "limits"}
    if set(manifest) != required:
        raise ValueError(f"manifest must contain exactly {sorted(required)}")
    if not isinstance(manifest["comparison_id"], str) or not manifest["comparison_id"].strip():
        raise ValueError("comparison_id must be non-empty")
    matched = manifest["matched_inputs"]
    if not isinstance(matched, dict) or set(matched) != {"plant_horizon_years", "net_electric_power_mw", "component_lifetimes_years", "parallel_remote_handling_systems", "assumptions"}:
        raise ValueError("matched_inputs is incomplete")
    validate_quantity(matched["plant_horizon_years"], "year", "plant_horizon_years")
    validate_quantity(matched["net_electric_power_mw"], "MW(e)", "net_electric_power_mw")
    if matched["plant_horizon_years"]["value"] <= 0 or matched["net_electric_power_mw"]["value"] <= 0:
        raise ValueError("horizon and net electric power must be positive")
    if not isinstance(matched["component_lifetimes_years"], list) or not matched["component_lifetimes_years"]:
        raise ValueError("component_lifetimes_years must list each included component explicitly")
    for i, component in enumerate(matched["component_lifetimes_years"]):
        if not isinstance(component, dict) or set(component) != {"component", "lifetime", "replacement_duration_days", "source"}:
            raise ValueError(f"component_lifetimes_years[{i}] is incomplete")
        if not isinstance(component["component"], str) or not component["component"].strip():
            raise ValueError(f"component_lifetimes_years[{i}] needs a component name")
        validate_quantity(component["lifetime"], "year", f"component {component['component']} lifetime")
        validate_quantity(component["replacement_duration_days"], "day", f"component {component['component']} replacement duration")
        validate_source(component["source"], f"component {component['component']}")
    validate_quantity(matched["parallel_remote_handling_systems"], "count", "parallel_remote_handling_systems")
    if matched["parallel_remote_handling_systems"]["value"] < 1 or int(matched["parallel_remote_handling_systems"]["value"]) != matched["parallel_remote_handling_systems"]["value"]:
        raise ValueError("parallel_remote_handling_systems must be a positive integer")
    if not isinstance(matched["assumptions"], list) or not matched["assumptions"]:
        raise ValueError("assumptions must explicitly list duty cycle, outage, and power assumptions")
    for i, assumption in enumerate(matched["assumptions"]):
        if not isinstance(assumption, dict) or set(assumption) != {"name", "value", "unit", "source"}:
            raise ValueError(f"assumptions[{i}] needs name, value, unit and source")
        validate_source(assumption["source"], f"assumptions[{i}]")

    mapping = manifest["mapping"]
    if not isinstance(mapping, dict) or set(mapping) != SUPPORTED_TOOL_KEYS:
        raise ValueError("mapping must contain PROCESS and bluemira")
    for key in SUPPORTED_TOOL_KEYS:
        if not isinstance(mapping[key], dict) or set(mapping[key]) != {"native_inputs", "downtime"}:
            raise ValueError(f"mapping.{key} needs native_inputs and downtime")
        validate_downtime_mapping(mapping[key]["downtime"], f"mapping.{key}.downtime")
    limitations: list[str] = []

    base = path.resolve().parent
    files: dict[str, Path] = {}
    faris = manifest["faris"]
    if not isinstance(faris, dict) or set(faris) != {"fixed", "computed"}:
        raise ValueError("faris must reference fixed and computed native-run evidence")
    for case in ("fixed", "computed"):
        entry = faris[case]
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "scope"}:
            raise ValueError(f"faris.{case} evidence needs path, sha256 and scope")
        if not isinstance(entry["scope"], str) or not entry["scope"].strip():
            raise ValueError(f"faris.{case}.scope must describe the authoritative FARIS output")
        files[f"faris.{case}"] = relative_file(base, entry["path"], entry["sha256"], f"faris.{case}")

    peers = manifest["peers"]
    if not isinstance(peers, dict) or set(peers) != SUPPORTED_TOOL_KEYS:
        raise ValueError("peers must contain PROCESS and bluemira")
    process = peers["process"]
    if not isinstance(process, dict) or set(process) != {"version", "executable", "input", "python"}:
        raise ValueError("peers.process needs version, executable, input and python")
    if not isinstance(process["version"], str) or not process["version"]:
        raise ValueError("PROCESS version must be pinned")
    files["process.input"] = relative_file(base, process["input"]["path"], process["input"]["sha256"], "PROCESS input")
    if not files["process.input"].name.endswith("IN.DAT"):
        raise ValueError("PROCESS input name must end in IN.DAT per its documented CLI")
    if not isinstance(process["executable"], str) or not process["executable"]:
        raise ValueError("PROCESS executable must be configured")
    if not isinstance(process["python"], str) or not process["python"]:
        raise ValueError("PROCESS Python interpreter must be configured for the official MFILE parser")

    bluemira = peers["bluemira"]
    if not isinstance(bluemira, dict) or set(bluemira) != {"version", "python", "config", "rng_seed"}:
        raise ValueError("peers.bluemira needs version, python, config and rng_seed")
    if not isinstance(bluemira["version"], str) or not bluemira["version"]:
        raise ValueError("bluemira version must be pinned")
    if not isinstance(bluemira["python"], str) or not bluemira["python"]:
        raise ValueError("bluemira Python interpreter must be configured")
    if isinstance(bluemira["rng_seed"], bool) or not isinstance(bluemira["rng_seed"], int):
        raise ValueError("bluemira rng_seed must be an explicit integer")
    files["bluemira.config"] = relative_file(base, bluemira["config"]["path"], bluemira["config"]["sha256"], "bluemira lifecycle config")

    for tool, native_path in (("process", files["process.input"]), ("bluemira", files["bluemira.config"])):
        limitations.extend(native_input_limitations(mapping[tool], tool, matched))
        native_fields = mapping[tool]["native_inputs"]
        for key, unit in (("plant_horizon_years", "year"), ("net_electric_power_mw", "MW(e)"),
                          ("parallel_remote_handling_systems", "count")):
            verify_native_input_file(native_fields[key], matched[key]["value"], unit, native_path, tool,
                                     f"mapping.{tool}.{key}")
        expected_components = {entry["component"]: entry for entry in matched["component_lifetimes_years"]}
        for component in native_fields["component_lifetimes_years"]:
            expected = expected_components[component["component"]]
            verify_native_input_file(component["lifetime"], expected["lifetime"]["value"], "year", native_path,
                                     tool, f"mapping.{tool}.{component['component']}.lifetime")
            verify_native_input_file(component["replacement_duration"], expected["replacement_duration_days"]["value"],
                                     "day", native_path, tool,
                                     f"mapping.{tool}.{component['component']}.replacement_duration")
        for category, item in mapping[tool]["downtime"].items():
            if item["disposition"] != "unmapped":
                verify_setting(item["native_setting"], native_path, tool, f"mapping.{tool}.downtime.{category}")
            else:
                limitations.append(f"{tool}.{category}: {item['note']}")

    limits = manifest["limits"]
    if not isinstance(limits, dict) or set(limits) != {"timeout_seconds", "output_bytes"}:
        raise ValueError("limits needs timeout_seconds and output_bytes")
    if (not isinstance(limits["timeout_seconds"], int) or not 1 <= limits["timeout_seconds"] <= 86_400
            or not isinstance(limits["output_bytes"], int) or not 1024 <= limits["output_bytes"] <= 2_000_000_000):
        raise ValueError("limits exceed supported bounds")
    return manifest, files, limitations


def bounded_run(argv: list[str], *, cwd: Path, timeout: int, output_bytes: int) -> dict[str, Any]:
    def limit_resources() -> None:
        resource.setrlimit(resource.RLIMIT_CPU, (timeout, timeout + 1))
        resource.setrlimit(resource.RLIMIT_FSIZE, (output_bytes, output_bytes))
        resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))

    process = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, shell=False, start_new_session=(os.name == "posix"),
                               preexec_fn=limit_resources if os.name == "posix" else None)
    assert process.stdout is not None and process.stderr is not None
    stdout_buffer, stderr_buffer = bytearray(), bytearray()
    streams = {process.stdout.fileno(): (process.stdout, stdout_buffer),
               process.stderr.fileno(): (process.stderr, stderr_buffer)}
    for fd in streams:
        os.set_blocking(fd, False)
    deadline = time.monotonic() + timeout

    def stop() -> None:
        if os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        elif process.poll() is None:
            process.terminate()
        if process.poll() is None:
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                else:
                    process.kill()
                process.wait()

    try:
        while streams:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                stop()
                raise subprocess.TimeoutExpired(argv, timeout)
            readable, _, _ = select.select(list(streams), [], [], min(remaining, 0.2))
            for fd in readable:
                stream, buffer = streams[fd]
                try:
                    block = os.read(fd, 8192)
                except BlockingIOError:
                    continue
                if not block:
                    stream.close()
                    del streams[fd]
                    continue
                buffer.extend(block)
                if sum(len(buf) for _, buf in streams.values()) > output_bytes:
                    stop()
                    raise ValueError("peer combined stdout and stderr exceeded configured output limit")
        result_code = process.wait(timeout=max(0.1, deadline - time.monotonic()))
    except BaseException:
        stop()
        for stream, _ in streams.values():
            stream.close()
        raise
    process.stdout.close()
    process.stderr.close()
    return {"argv": argv, "return_code": result_code,
            "stdout": bytes(stdout_buffer)[-MAX_CAPTURE:].decode("utf-8", "replace"),
            "stderr": bytes(stderr_buffer)[-MAX_CAPTURE:].decode("utf-8", "replace")}


def ensure_dir_size(directory: Path, limit: int, label: str) -> int:
    total = 0
    for path in directory.rglob("*"):
        if path.is_file():
            total += path.stat().st_size
            if total > limit:
                raise ValueError(f"{label} output files exceeded configured byte limit")
    return total


def run_manifest(manifest_path: Path, out_dir: Path) -> dict[str, Any]:
    manifest, files, limitations = validate_manifest(manifest_path)
    if out_dir.exists() and any(out_dir.iterdir()):
        raise ValueError(f"output directory already exists and is not empty: {out_dir}")
    out_dir.mkdir(parents=True, exist_ok=True)
    timeout = manifest["limits"]["timeout_seconds"]
    output_bytes = manifest["limits"]["output_bytes"]
    report: dict[str, Any] = {
        "schema": "faris-peer-lifecycle-report/0.1.0",
        "comparison_id": manifest["comparison_id"],
        "authority": "FARIS native engine remains authoritative; peers are code-to-code references only",
        "physical_validation": False,
        "mapping_limitations": limitations,
        "manifest_sha256": sha256(manifest_path),
        "peer_runs": {},
    }
    for case in ("fixed", "computed"):
        item = manifest["faris"][case]
        report["peer_runs"][f"faris_{case}"] = {
            "kind": "authoritative FARIS input reference",
            "path": str(files[f"faris.{case}"]),
            "sha256": item["sha256"],
            "scope": item["scope"],
        }

    process = manifest["peers"]["process"]
    process_dir = out_dir / "process"
    process_dir.mkdir()
    process_input = process_dir / files["process.input"].name
    shutil.copyfile(files["process.input"], process_input)
    if not process_input.name.endswith("IN.DAT"):
        raise ValueError("PROCESS input name must end with IN.DAT per PROCESS CLI contract")
    start = time.time()
    proc_record = bounded_run([process["executable"], "-i", str(process_input)], cwd=process_dir,
                              timeout=timeout, output_bytes=output_bytes)
    mfile = process_input.with_name(process_input.name[:-6] + "MFILE.DAT")
    if proc_record["return_code"] != 0 or not mfile.is_file():
        raise RuntimeError("PROCESS failed or did not produce its documented MFILE.DAT output")
    process_output_size = ensure_dir_size(process_dir, output_bytes, "PROCESS")
    proc_record.update({"version_expected": process["version"], "input_sha256": sha256(process_input),
                        "mfile_path": mfile.name, "mfile_sha256": sha256(mfile),
                        "output_bytes": process_output_size,
                        "wall_seconds": round(time.time() - start, 6)})
    parser_script = (Path(__file__).with_name("process_mfile_metrics.py")).resolve()
    metrics = bounded_run([process["python"], str(parser_script), process["version"], str(mfile)], cwd=process_dir,
                          timeout=min(timeout, 120), output_bytes=output_bytes)
    if metrics["return_code"] != 0:
        raise RuntimeError("PROCESS official MFILE reader failed: " + metrics["stderr"][-1000:])
    proc_record["metrics"] = json.loads(metrics["stdout"])
    report["peer_runs"]["process"] = proc_record

    blue = manifest["peers"]["bluemira"]
    blue_dir = out_dir / "bluemira"
    blue_dir.mkdir()
    output = blue_dir / "timeline.json"
    start = time.time()
    blue_record = bounded_run([blue["python"], str(Path(__file__).with_name("bluemira_worker.py")),
                               str(files["bluemira.config"]), str(output), str(blue["rng_seed"]), blue["version"]],
                              cwd=blue_dir, timeout=timeout, output_bytes=output_bytes)
    if blue_record["return_code"] != 0 or not output.is_file():
        raise RuntimeError("bluemira LifeCycle adapter failed or did not write its native timeline")
    blue_output_size = ensure_dir_size(blue_dir, output_bytes, "bluemira")
    blue_record.update({"version_expected": blue["version"], "config_sha256": sha256(files["bluemira.config"]),
                        "timeline_path": output.name, "timeline_sha256": sha256(output),
                        "output_bytes": blue_output_size,
                        "wall_seconds": round(time.time() - start, 6)})
    blue_record["metrics"] = json.loads(blue_record.pop("stdout"))
    report["peer_runs"]["bluemira"] = blue_record

    report["conditional_comparison"] = conditional_metrics(manifest, proc_record["metrics"], blue_record["metrics"])
    target = out_dir / "comparison.json"
    target.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return report


def conditional_metrics(manifest: dict[str, Any], process: dict[str, Any], bluemira: dict[str, Any]) -> dict[str, Any]:
    """Use native outputs only; report energy as conditional normalization, not a tool output."""
    horizon = manifest["matched_inputs"]["plant_horizon_years"]["value"]
    power = manifest["matched_inputs"]["net_electric_power_mw"]["value"]
    process_avail = process.get("plant_availability")
    process_capacity = process.get("capacity_factor")
    process_years = process.get("plant_lifetime_years")
    blue_fpy = bluemira.get("full_power_years")
    blue_total_years = bluemira.get("calendar_years")
    out: dict[str, Any] = {"unit": "conditional MWh(e)", "horizon_years": horizon,
                           "net_electric_power_mw": power, "normalization": "net MW(e) × full-power years × 8760 h/year; not a peer-native power-cycle result"}
    if (isinstance(process_avail, (int, float)) and math.isfinite(process_avail) and 0 <= process_avail <= 1
            and isinstance(process_capacity, (int, float)) and math.isfinite(process_capacity)
            and isinstance(process_years, (int, float)) and math.isfinite(process_years)
            and math.isclose(process_years, horizon, rel_tol=0, abs_tol=1e-6)):
        out["PROCESS"] = {"plant_availability_fraction": process_avail,
                          "capacity_factor": process_capacity,
                          "full_power_years": process_capacity * process_years,
                          "net_electric_energy_mwh_e": power * process_capacity * process_years * 8760}
    else:
        out["PROCESS"] = {"status": "not comparable", "reason": "MFILE capacity factor/lifetime is absent, non-finite, or the native lifetime differs from the matched horizon"}
    if (isinstance(blue_fpy, (int, float)) and math.isfinite(blue_fpy)
            and isinstance(blue_total_years, (int, float)) and math.isfinite(blue_total_years)
            and blue_total_years > 0 and math.isclose(blue_total_years, horizon, rel_tol=0, abs_tol=1e-6)):
        fraction = blue_fpy / blue_total_years
        out["bluemira"] = {"plant_availability_fraction": fraction, "full_power_years": blue_fpy,
                           "net_electric_energy_mwh_e": power * blue_fpy * 8760}
    else:
        out["bluemira"] = {"status": "not comparable", "reason": "LifeCycle native output lacks finite full-power/calendar years or native horizon differs from the matched horizon"}
    has_unmapped = False
    for tool_entry in manifest["mapping"].values():
        native = tool_entry["native_inputs"]
        has_unmapped |= any(native[key]["disposition"] == "unmapped" for key in
                            ("plant_horizon_years", "net_electric_power_mw", "parallel_remote_handling_systems"))
        has_unmapped |= any(c[key]["disposition"] == "unmapped" for c in native["component_lifetimes_years"]
                            for key in ("lifetime", "replacement_duration"))
        has_unmapped |= any(entry["disposition"] == "unmapped" for entry in tool_entry["downtime"].values())
    out["availability_comparison_allowed"] = not has_unmapped
    out["comparison_status"] = "conditional" if out["availability_comparison_allowed"] and all(
        out[key].get("status") is None for key in ("PROCESS", "bluemira")
    ) else "incomplete"
    out["interpretation"] = "Conditional code-to-code comparison only; unsupported mappings listed separately; not physical validation."
    return out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        run_manifest(args.manifest.resolve(), args.out.resolve())
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
