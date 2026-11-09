#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run the pinned ACTINV FNS-96 Fe case and seal results for FARIS scoring.

This is a prescribed-spectrum activation/decay calculation, not a transport
simulation. It requires explicit upstream and ACTINV data paths. The only
execution is ``actinv run`` of the pinned 21-step case with a 180-second
timeout; no data download, fallback input generation, or stale-output reuse is
performed.
"""
from __future__ import annotations

import argparse
import csv
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT))
from validation.manifest import check_manifest, load_manifest, manifest_sha256, verify_files  # noqa: E402
from validation.scoring import RUN_KIND, seal_run_record  # noqa: E402

CASE_ID = "fns-decayheat-fe-1996-5min"
SOURCE_COMMIT = "1914ed418b597398d758e7c6fa11e83158e2ca21"
SPEC_REL = "jade_open_benchmarks/inputs/FNS-DecayHeat/Fe-1996-5min/actinv/spec.json"
CSV_REL = "jade_open_benchmarks/exp_results/FNS-DecayHeat/Fe-1996-5min Decay heat.csv"
ARCHIVE_SHA256 = "ba1dd6cb150a4aa3e0d81461054aec7d415ef19d946aba8b9886b31de218252d"
SOURCE_NOTE_SHA256 = "7eaf6669f0aa3942579541cdf566cf41d9ea167fb3eb3fcea759028882740e11"
LICENSE_SHA256 = "9e5f1b3c610b9c2da5c313bf81d577a7d1acec686bdb0384edefa6df0f90cd94"
MAX_RESULT_BYTES = 128 * 1024 * 1024
RUN_TIMEOUT_SECONDS = 180
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
LIBRARY_COMPONENTS = ("library", "library_index", "decay_primary", "decay_fallback")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def verify_archive_members(manifest: dict, source_note: bytes) -> None:
    """Bind the archive member identities to the hash-pinned upstream provenance table."""
    rows = re.findall(r"\|\s*`([^`]+)`\s*\|\s*`([0-9a-f]{64})`\s*\|", source_note.decode("utf-8"))
    expected = {f"fns/Fe/{name}": digest for name, digest in rows}
    require(len(expected) == 4 and len(rows) == 4,
            "pinned source note must contain the four original archive member identities")
    require(manifest["source"].get("original_archive_members") == expected,
            "original archive member hashes differ from the pinned upstream source note")


def load_inputs(upstream: Path, manifest_path: Path | None = None) -> tuple[dict, dict, bytes, bytes]:
    upstream = upstream.expanduser().resolve()
    path = manifest_path or HERE / "manifest.json"
    manifest = check_manifest(load_manifest(path))
    require(manifest["case_id"] == CASE_ID, f"manifest is not {CASE_ID}")
    require(manifest["source"]["commit"] == SOURCE_COMMIT, "manifest source commit changed")
    require(manifest["source"].get("original_archive_sha256") == ARCHIVE_SHA256,
            "manifest original CoNDERC archive identity changed")
    require(manifest["source"].get("processing_note", {}).get("sha256") == SOURCE_NOTE_SHA256,
            "manifest source processing-note identity changed")
    require(manifest["source"].get("license_file", {}).get("sha256") == LICENSE_SHA256,
            "manifest source license identity changed")
    problems = verify_files(manifest, upstream)
    require(not problems, "upstream files failed their pinned hashes: " + "; ".join(problems))
    commit_exists = subprocess.run(["git", "-C", str(upstream), "cat-file", "-e", f"{SOURCE_COMMIT}^{{commit}}"], capture_output=True)
    require(commit_exists.returncode == 0, f"upstream does not contain pinned source commit {SOURCE_COMMIT}")
    for entry in manifest["files"]:
        shown = subprocess.run(["git", "-C", str(upstream), "show", f"{SOURCE_COMMIT}:{entry['path_in_source']}"],
                               capture_output=True)
        require(shown.returncode == 0 and sha256_bytes(shown.stdout) == entry["sha256"],
                f"pinned source commit does not match {entry['path_in_source']}")
    for relative, expected in ((manifest["source"]["processing_note"]["path"], SOURCE_NOTE_SHA256),
                               (manifest["source"]["license_file"]["path"], LICENSE_SHA256)):
        shown = subprocess.run(["git", "-C", str(upstream), "show", f"{SOURCE_COMMIT}:{relative}"], capture_output=True)
        require(shown.returncode == 0 and sha256_bytes(shown.stdout) == expected,
                f"pinned source commit does not match provenance file {relative}")
        if relative == manifest["source"]["processing_note"]["path"]:
            verify_archive_members(manifest, shown.stdout)
    spec_path, csv_path = upstream / SPEC_REL, upstream / CSV_REL
    require(spec_path.stat().st_size <= 16_384, "ACTINV problem exceeds the 16384-byte intake bound")
    require(csv_path.stat().st_size <= 1_024, "measurement table exceeds the 1024-byte intake bound")
    spec_bytes, csv_bytes = spec_path.read_bytes(), csv_path.read_bytes()
    spec = json.loads(spec_bytes)
    measurement_times = verify_manifest_rows(csv_bytes, manifest)
    require(spec.get("spec") == "actinv-spec-1" and spec.get("projectile") == "neutron", "unexpected ACTINV problem schema")
    require(spec.get("material") == {"mass_g": 1.0, "basis": "wt_percent", "composition": {"FE": 100.0}},
            "ACTINV input is not the pinned one-gram iron sample")
    spectrum = spec.get("spectrum", {})
    require(spectrum.get("structure") == "fispact-709" and spectrum.get("descending") is True
            and len(spectrum.get("flux_per_group", [])) == 709,
            "ACTINV input does not carry the 709-group source spectrum")
    require(all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) and v >= 0
                for v in spectrum["flux_per_group"]), "ACTINV group fluxes must be finite and non-negative")
    require(spectrum.get("total") == 1.116e10, "ACTINV total flux differs from source-deck normalization")
    require(math.fsum(spectrum["flux_per_group"]) > 0.0, "ACTINV source spectrum has no positive shape")
    require(spec.get("library") == {"path": "catalog:tendl-2025-neutron-709g"}
            and spec.get("decay") == {"primary": "catalog:endfb-viii-0-decay", "fallback": "catalog:jeff-3-3-decay"},
            "ACTINV activation or decay-data references changed")
    schedule = spec.get("schedule", [])
    require(len(schedule) == 21 and schedule[0].get("dt") == "300 s" and schedule[0].get("flux") == 1.0,
            "ACTINV input must retain the 300-second full-flux irradiation step")
    require(all(step.get("flux") == 0.0 for step in schedule[1:]), "ACTINV cooling steps must have zero flux")
    elapsed = Decimal(0)
    for index, (step, cooling_time) in enumerate(zip(schedule[1:], measurement_times, strict=True), start=1):
        raw_duration = step.get("dt", "").split()
        require(len(raw_duration) == 2 and raw_duration[1] == "s", f"ACTINV schedule step {index} has unsupported time units")
        try:
            elapsed += Decimal(raw_duration[0])
        except InvalidOperation as exc:
            raise ValueError(f"ACTINV schedule step {index} has an invalid duration") from exc
        require(math.isclose(float(elapsed), cooling_time, rel_tol=0.0, abs_tol=1e-9),
                f"ACTINV schedule step {index} does not end at the published cooling time")
    return manifest, spec, spec_bytes, csv_bytes


def verify_manifest_rows(csv_bytes: bytes, manifest: dict) -> list[float]:
    try:
        reader = csv.DictReader(io.StringIO(csv_bytes.decode("utf-8-sig")))
    except UnicodeDecodeError as exc:
        raise ValueError("measurement CSV is not UTF-8") from exc
    require(reader.fieldnames == ["time", "Value", "Error"], "measurement CSV header changed")
    rows = list(reader)
    detectors = manifest["detectors"]
    require(len(rows) == len(detectors) == 20, "measurement/reference coverage must retain all 20 points")
    times = []
    for index, (raw, detector) in enumerate(zip(rows, detectors, strict=True), start=1):
        try:
            time_s, value, relative_error = (Decimal(raw[key]) for key in ("time", "Value", "Error"))
        except (KeyError, InvalidOperation, TypeError) as exc:
            raise ValueError(f"invalid measurement CSV row {index}") from exc
        require(all(x.is_finite() and x > 0 for x in (time_s, value, relative_error)), f"invalid CSV values at row {index}")
        ref = detector.get("reference", {})
        tv = detector.get("controlling_variable", {}).get("value")
        u_rel = detector.get("reported_error_relative")
        require(detector.get("id") == f"cooling-{index:02d}", f"unexpected detector order at measurement {index}")
        require(float(time_s) == tv and float(value) == ref.get("value") and float(relative_error) == u_rel,
                f"manifest reference does not match source CSV row {index}")
        require(math.isclose(ref.get("u", math.nan), float(value * relative_error), rel_tol=0.0, abs_tol=1e-14),
                f"manifest reference error conversion is wrong at row {index}")
        times.append(float(time_s))
    return times


def finite_nonnegative(value, field: str) -> float:
    require(isinstance(value, (int, float)) and not isinstance(value, bool), f"{field} must be numeric")
    value = float(value)
    require(math.isfinite(value) and value >= 0, f"{field} must be finite and non-negative")
    return value


def parse_result(result: dict, manifest: dict) -> list[dict]:
    """Map actual ACTINV result steps to all source detector IDs; reject time shifts."""
    require(isinstance(result, dict), "ACTINV output must be a JSON object")
    require(result.get("entry_point") == "cli", "ACTINV result did not come from the CLI")
    certificate = result.get("certificate", {})
    solver = certificate.get("solver")
    require(isinstance(solver, str) and solver.startswith("actinv-core "), "ACTINV certificate lacks solver identity")
    require(result.get("mode") == "trace" and certificate.get("mode") == "trace",
            "FNS input must execute in the certified ACTINV trace mode")
    require(certificate.get("prune") == "rate", "ACTINV pruning method differs from the pinned FNS problem")
    steps = result.get("steps")
    require(isinstance(steps, list) and len(steps) == len(manifest["detectors"]) + 1,
            "ACTINV output must contain one EOI state and all 20 measurement endpoints")
    times: list[float] = []
    for index, step in enumerate(steps):
        require(isinstance(step, dict), f"ACTINV step {index} must be an object")
        require(step.get("step") == index + 1, f"ACTINV result step index is discontinuous at {index}")
        t_s = finite_nonnegative(step.get("t_s"), f"steps[{index}].t_s")
        require(not times or t_s > times[-1], "ACTINV absolute times must increase strictly")
        times.append(t_s)
        finite_nonnegative(step.get("flux"), f"steps[{index}].flux")
    require(math.isclose(times[0], 300.0, rel_tol=0.0, abs_tol=1e-6), "first ACTINV result must be the 300 s end-of-irradiation state")
    require(float(steps[0]["flux"]) > 0.0, "end-of-irradiation state must carry the irradiated flux")

    results = []
    for detector, step, t_s in zip(manifest["detectors"], steps[1:], times[1:], strict=True):
        cooling_s = detector["controlling_variable"]["value"]
        expected_t_s = 300.0 + float(cooling_s)
        require(math.isclose(t_s, expected_t_s, rel_tol=0.0, abs_tol=1e-6),
                f"{detector['id']}: ACTINV time {t_s} s does not match shutdown+cooling {expected_t_s} s")
        require(float(step["flux"]) == 0.0, f"{detector['id']}: cooling step has nonzero flux")
        heat = step.get("heat_W_per_g")
        require(isinstance(heat, dict), f"{detector['id']}: missing ACTINV heat_W_per_g")
        total = finite_nonnegative(heat.get("total"), f"{detector['id']}.heat.total")
        alpha = finite_nonnegative(heat.get("alpha"), f"{detector['id']}.heat.alpha")
        beta = finite_nonnegative(heat.get("beta"), f"{detector['id']}.heat.beta")
        gamma = finite_nonnegative(heat.get("gamma"), f"{detector['id']}.heat.gamma")
        require(math.isclose(total, math.fsum((alpha, beta, gamma)), rel_tol=1e-12, abs_tol=1e-20),
                f"{detector['id']}: ACTINV total heat does not close to alpha+beta+gamma")
        value = total * 1e6
        require(math.isfinite(value), f"{detector['id']}: W/g to microW/g conversion overflowed")
        results.append({
            "detector_id": detector["id"],
            "value": value,
            "u_mc": 0.0,
            "time_s": t_s,
            "source_step": step.get("step"),
            "unit": "microW/g",
            "conversion": "ACTINV heat_W_per_g.total multiplied by 1e6",
            "component_closure": "total equals alpha+beta+gamma within 1e-12 relative plus 1e-20 W/g",
        })
    return results


def data_identities(result: dict) -> dict[str, dict]:
    inputs = result.get("certificate", {}).get("inputs")
    require(isinstance(inputs, dict), "ACTINV certificate lacks resolved input data identities")
    identities = {}
    for name in LIBRARY_COMPONENTS:
        item = inputs.get(name)
        require(isinstance(item, dict), f"ACTINV certificate lacks {name} identity")
        digest = item.get("sha256")
        require(isinstance(digest, str) and SHA256_RE.fullmatch(digest) is not None,
                f"ACTINV certificate has invalid {name} SHA-256")
        identities[name] = {"sha256": digest, "path": item.get("path")}
    return identities


def bounded_cli(command: list[str], env: dict[str, str], log_path: Path, label: str) -> subprocess.CompletedProcess:
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=env)
    except subprocess.TimeoutExpired as exc:
        def text(value) -> str:
            if isinstance(value, bytes):
                return value.decode("utf-8", errors="replace")
            return value or ""
        log_path.write_text(text(exc.stdout) + text(exc.stderr), encoding="utf-8")
        raise ValueError(f"{label} exceeded the {RUN_TIMEOUT_SECONDS}-second timeout; see {log_path}") from exc
    log_path.write_text(completed.stdout + completed.stderr, encoding="utf-8")
    return completed


def make_run_record(run_id: str, manifest: dict, spec_bytes: bytes, raw_result: bytes,
                    result: dict, actinv_path: Path, results: list[dict]) -> dict:
    require(run_id.strip() != "", "run_id must be non-empty")
    data = data_identities(result)
    data_hashes = {name: data[name]["sha256"] for name in sorted(data)}
    bundle_hash = sha256_bytes(json.dumps(data_hashes, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8"))
    solver = result["certificate"]["solver"]
    record = {
        "kind": RUN_KIND,
        "schema": "faris-validation-run/1.0.0",
        "run_id": run_id,
        "case_id": CASE_ID,
        "run_date": datetime.now(timezone.utc).date().isoformat(),
        "identity": {"library_sha256": bundle_hash, "code_version": solver,
                     "adapter_sha256": sha256_file(Path(__file__))},
        "manifest_sha256": manifest_sha256(manifest),
        "results": results,
        "execution": {
            "adapter": "FARIS FNS-96 Fe prescribed-spectrum ACTINV runner",
            "actinv_binary_sha256": sha256_file(actinv_path),
            "spec_sha256": sha256_bytes(spec_bytes),
            "actinv_result_sha256": sha256_bytes(raw_result),
            "actinv_solver": solver,
            "actinv_mode": result.get("mode"),
            "resolved_data": data,
            "library_identity_basis": "SHA-256 of the canonical map of ACTINV activation library, index and primary/fallback decay-data hashes; installation paths are recorded but excluded from identity",
            "uncertainty": "ACTINV reports no sampling uncertainty for this deterministic run (u_mc=0); model and nuclear-data uncertainty are unquantified",
        },
    }
    return seal_run_record(record)


def command_run(args) -> int:
    upstream = args.upstream.expanduser().resolve()
    manifest, spec, spec_bytes, _ = load_inputs(upstream, args.manifest)
    actinv = args.actinv.expanduser().resolve()
    data_root = args.data_root.expanduser().resolve()
    work_dir = args.work_dir.expanduser().resolve()
    require(actinv.is_file() and os.access(actinv, os.X_OK), f"ACTINV executable is not executable: {actinv}")
    require(data_root.is_dir(), f"ACTINV data root does not exist: {data_root}")
    require(not work_dir.exists(), f"work directory already exists; refusing stale-output reuse: {work_dir}")
    work_dir.mkdir(parents=True)
    source_spec = upstream / SPEC_REL
    validate = bounded_cli([str(actinv), "validate", str(source_spec)],
                           {**os.environ, "ACTINV_DATA_DIR": str(data_root)}, work_dir / "validate.log", "ACTINV validation")
    require(validate.returncode == 0, f"ACTINV input validation failed; see {work_dir / 'validate.log'}")
    result_path = work_dir / "actinv-result.json"
    env = {**os.environ, "ACTINV_DATA_DIR": str(data_root), "RAYON_NUM_THREADS": "1",
           "OMP_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1"}
    command = [str(actinv), "run", str(source_spec), str(result_path)]
    completed = bounded_cli(command, env, work_dir / "run.log", "ACTINV run")
    require(completed.returncode == 0, f"ACTINV run failed; see {work_dir / 'run.log'}")
    require(result_path.is_file(), "ACTINV exited successfully but did not write result JSON")
    require(result_path.stat().st_size <= MAX_RESULT_BYTES, "ACTINV result exceeds the 128 MiB intake bound")
    raw_result = result_path.read_bytes()
    result = json.loads(raw_result)
    mapped = parse_result(result, manifest)
    record = make_run_record(args.run_id or f"actinv-{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')}",
                             manifest, spec_bytes, raw_result, result, actinv, mapped)
    record_path = args.record.expanduser() if args.record else work_dir / f"{CASE_ID}.run.json"
    require(not record_path.exists(), f"record already exists; refusing to replace evidence: {record_path}")
    record_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = record_path.with_suffix(record_path.suffix + ".tmp")
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    temporary.replace(record_path)
    print(json.dumps({"case_id": CASE_ID, "run_id": record["run_id"], "record": str(record_path),
                      "record_sha256": record["record_sha256"], "identity": record["identity"],
                      "verdict_policy": "unknown covariance; all scored rows remain INCONCLUSIVE"}, indent=2))
    return 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = root.add_subparsers(dest="command", required=True)
    plan = sub.add_parser("plan", help="verify pinned inputs and describe bounded ACTINV execution; no solver run")
    plan.add_argument("--upstream", type=Path, required=True)
    plan.add_argument("--manifest", type=Path, default=HERE / "manifest.json")
    run = sub.add_parser("run", help="run ACTINV with a 180-second timeout and write a sealed FARIS record")
    run.add_argument("--upstream", type=Path, required=True)
    run.add_argument("--manifest", type=Path, default=HERE / "manifest.json")
    run.add_argument("--actinv", type=Path, required=True)
    run.add_argument("--data-root", type=Path, required=True)
    run.add_argument("--work-dir", type=Path, required=True, help="new directory; existing directories are refused")
    run.add_argument("--record", type=Path, help="record path; defaults inside --work-dir")
    run.add_argument("--run-id")
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "plan":
            manifest, spec, spec_bytes, csv_bytes = load_inputs(args.upstream, args.manifest)
            row_count = len(manifest["detectors"])
            print(json.dumps({
                "case_id": CASE_ID,
                "source_commit": SOURCE_COMMIT,
                "manifest_sha256": manifest_sha256(manifest),
                "spec_sha256": sha256_bytes(spec_bytes),
                "measurement_table_sha256": sha256_bytes(csv_bytes),
                "measurements": row_count,
                "irradiation_seconds": 300,
                "total_flux_n_cm2_s": spec["spectrum"]["total"],
                "raw_group_sum_n_cm2_s": math.fsum(spec["spectrum"]["flux_per_group"]),
                "spectrum_scaling": "ACTINV scales the pinned group vector to declared total while preserving its shape",
                "groups": len(spec["spectrum"]["flux_per_group"]),
                "actinv_timeout_seconds": RUN_TIMEOUT_SECONDS,
                "solver_runs": 1,
                "measurement_error_policy": "preserve reported relative magnitude; no confidence/covariance assigned",
                "scoring": "C/E descriptive; unknown covariance forces INCONCLUSIVE",
            }, indent=2))
            return 0
        return command_run(args)
    except (OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
