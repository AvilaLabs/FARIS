#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Rehash and reopen a relocated recorded demo package, with a tamper control."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile
from typing import Any
SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
from port_geometry_contract import validate_ownership_audits

INDEX = "package-index.json"
CHECKSUM = "package-index.sha256"
MAX_FILE_BYTES = 64 * 1024 * 1024
FORBIDDEN_SUFFIXES = {".h5", ".hdf5", ".endf", ".zip"}


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return "sha256:" + hasher.hexdigest()


def safe_package_path(root: Path, relative: str, *, must_be_file: bool = True) -> Path:
    if not isinstance(relative, str) or "\\" in relative or ":" in relative:
        raise ValueError(f"unsafe package-relative path: {relative!r}")
    raw_parts = relative.split("/")
    if any(part in {"", ".", ".."} for part in raw_parts):
        raise ValueError(f"unsafe package-relative path: {relative!r}")
    rel = PurePosixPath(relative)
    if rel.is_absolute() or not rel.parts:
        raise ValueError(f"unsafe package-relative path: {relative!r}")
    path = root.joinpath(*rel.parts)
    cursor = root
    for part in rel.parts:
        cursor = cursor / part
        if cursor.is_symlink():
            raise ValueError(f"package path traverses a symlink: {relative!r}")
    resolved = path.resolve(strict=True)
    kind_matches = resolved.is_file() if must_be_file else resolved.is_dir()
    if not kind_matches or not resolved.is_relative_to(root):
        raise ValueError(f"package file is missing or escapes package: {relative!r}")
    return resolved


def verify_index(package: Path, faris: Path, core: Path) -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    root = package.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("package path must be a directory")
    index_path = safe_package_path(root, INDEX)
    checksum_path = safe_package_path(root, CHECKSUM)
    if index_path.stat().st_size > 16 * 1024 * 1024 or checksum_path.stat().st_size > 256:
        raise ValueError("package index or checksum exceeds its bounded size")
    expected_line = f"{digest(index_path)}  {INDEX}\n"
    if checksum_path.read_text(encoding="ascii") != expected_line:
        raise ValueError("package-index.sha256 does not match package-index.json")
    index = json.loads(index_path.read_text(encoding="utf-8"))
    if index.get("schema_version") != "faris-recorded-demo-package/v0.3":
        raise ValueError("unsupported recorded demo package schema")
    if index.get("status") != "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED":
        raise ValueError("package status does not preserve the required NOT_EVALUATED scope")
    if digest(faris.resolve(strict=True)) != index.get("faris_cli_sha256"):
        raise ValueError("selected FARIS executable differs from the package pin")
    if digest(core.resolve(strict=True)) != index.get("core_executable_sha256"):
        raise ValueError("selected Core executable differs from the package pin")
    files = index.get("files")
    if not isinstance(files, list) or not files or len(files) > 2048:
        raise ValueError("package index has an empty or excessive file inventory")
    inventory: dict[str, dict[str, Any]] = {}
    total_bytes = 0
    for item in files:
        relative = item.get("path")
        if not isinstance(relative, str) or relative in inventory:
            raise ValueError("package index repeats or omits a file path")
        path = safe_package_path(root, relative)
        observed = digest(path)
        size = path.stat().st_size
        if size > MAX_FILE_BYTES or Path(relative).suffix.lower() in FORBIDDEN_SUFFIXES:
            raise ValueError(f"indexed file exceeds limits or contains excluded nuclear data: {relative}")
        if observed != item.get("sha256") or size != item.get("bytes"):
            raise ValueError(f"indexed package file failed size or SHA-256 check: {relative}")
        total_bytes += size
        inventory[relative] = item
    if total_bytes > 512 * 1024 * 1024:
        raise ValueError("package exceeds the declared 512 MiB bound")
    actual = set()
    tree_entries = 0
    for path in root.rglob("*"):
        tree_entries += 1
        if tree_entries > 16_384 or path.is_symlink():
            raise ValueError("package tree has excessive entries or contains a symlink")
        relative = path.relative_to(root).as_posix()
        if path.is_file() and relative not in {INDEX, CHECKSUM}:
            actual.add(relative)
    if actual != set(inventory):
        missing = sorted(set(inventory) - actual)
        extra = sorted(actual - set(inventory))
        raise ValueError(f"package file inventory differs (missing={missing}, extra={extra})")
    return index, inventory


def inspect_cases(package: Path, index: dict[str, Any], faris: Path) -> list[dict[str, str]]:
    root = package.resolve(strict=True)
    pairs = index.get("scenario_pairs")
    if not isinstance(pairs, list) or len(pairs) != 2:
        raise ValueError("package must contain feature-free and port scenario pairs")
    pair_ids = {item.get("scenario_path", "").split("/", 1)[0] for item in pairs}
    if pair_ids != {"control", "port"}:
        raise ValueError("package scenario pairs must be exactly control and port")
    inspected: list[dict[str, str]] = []
    seen_variants: set[tuple[str, str]] = set()
    for pair in pairs:
        scenario_rel = pair.get("scenario_path")
        scenario_path = safe_package_path(root, scenario_rel)
        pair_id = scenario_rel.split("/", 1)[0]
        scenario_bytes = scenario_path.read_bytes()
        scenario = json.loads(scenario_bytes)
        scenario_sha = digest(scenario_path).removeprefix("sha256:")
        if (scenario_sha != pair.get("scenario_sha256")
                or scenario.get("id") != pair.get("scenario_id")):
            raise ValueError(f"scenario index identity mismatch for {pair_id}")
        if pair_id == "control":
            if pair.get("feature") != "feature_free_control" or scenario.get("penetration") is not None:
                raise ValueError("feature-free package scenario unexpectedly declares a penetration")
        else:
            penetration = scenario.get("penetration")
            if (pair.get("feature") != "finite_port" or not isinstance(penetration, dict)
                    or penetration.get("kind") != "outboard_rectangular_prism"):
                raise ValueError("port package scenario lacks its declared finite penetration")
        arrangements = pair.get("arrangements")
        if not isinstance(arrangements, list) or len(arrangements) != 2:
            raise ValueError(f"{pair_id} must contain exactly two arrangements")
        for arrangement in arrangements:
            variant = arrangement.get("variant_id")
            key = (pair_id, variant)
            if key in seen_variants or variant not in {"reference", "breeder-emphasis"}:
                raise ValueError("package repeats or contains an unexpected arrangement")
            seen_variants.add(key)
            descriptor_rel = arrangement.get("saved_study_descriptor")
            descriptor_path = safe_package_path(root, descriptor_rel)
            if digest(descriptor_path) != arrangement.get("saved_study_descriptor_sha256"):
                raise ValueError(f"saved-study descriptor digest mismatch: {descriptor_rel}")
            descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
            if set(descriptor) != {"case_directory", "execution_report", "execution_workspace"}:
                raise ValueError(f"unsupported saved-study descriptor schema: {descriptor_rel}")
            expected_case = f"{pair_id}/cases/{variant}"
            expected_report = arrangement.get("core_execution_report")
            expected_workspace = f"{pair_id}/core-workspaces/{variant}"
            if (descriptor.get("case_directory") != expected_case
                    or descriptor.get("execution_report") != expected_report
                    or descriptor.get("execution_workspace") != expected_workspace):
                raise ValueError(f"saved-study descriptor paths differ from indexed case for {pair_id}/{variant}")
            case = safe_package_path(root, descriptor["case_directory"], must_be_file=False)
            report = safe_package_path(root, descriptor["execution_report"])
            workspace = safe_package_path(root, descriptor["execution_workspace"], must_be_file=False)
            output = subprocess.run(
                [str(faris), "evidence", "inspect", "--case", str(case),
                 "--report", str(report), "--workspace", str(workspace)],
                check=False, capture_output=True, text=True, timeout=180,
            )
            if output.returncode != 0:
                raise ValueError(f"FARIS evidence inspect rejected {pair_id}/{variant}: {output.stderr[-2000:]}")
            try:
                fresh = json.loads(output.stdout)
            except json.JSONDecodeError as exc:
                raise ValueError(f"FARIS evidence inspect returned invalid JSON for {pair_id}/{variant}") from exc
            expected_inspection_path = safe_package_path(root, arrangement["saved_case_inspection"])
            if digest(expected_inspection_path) != arrangement.get("saved_case_inspection_sha256"):
                raise ValueError(f"saved inspection report digest mismatch for {pair_id}/{variant}")
            saved = json.loads(expected_inspection_path.read_text(encoding="utf-8"))
            if fresh != saved:
                raise ValueError(f"relocated saved-case inspection differs for {pair_id}/{variant}")
            expected_scenario = pair.get("scenario_sha256")
            if (fresh.get("record_integrity") != "UNSIGNED_IDENTITY_REVALIDATED"
                    or fresh.get("schema_version") != "faris-saved-case-inspection/v0.2"
                    or fresh.get("scenario_sha256") != f"sha256:{expected_scenario}"
                    or fresh.get("variant_id") != variant
                    or fresh.get("execution_status") != "executed"
                    or fresh.get("binding_status") != "verified"
                    or not fresh.get("compiler_id")
                    or not fresh.get("semantic_profile")
                    or fresh.get("compiler_executable_sha256") != fresh.get("core_executable_sha256")
                    or not fresh.get("steps")
                    or fresh.get("verified_receipt_count") != len(fresh["steps"])):
                raise ValueError(f"relocated Core evidence did not revalidate for {pair_id}/{variant}")
            verdicts = fresh.get("requirement_verdicts", [])
            if (arrangement.get("scientific_qualification") != "NOT_EVALUATED"
                    or not verdicts
                    or any(item.get("status") != "not_evaluated" for item in verdicts)
                    or not arrangement.get("core_requirement_verdicts")
                    or any(item != "not_evaluated"
                           for item in arrangement.get("core_requirement_verdicts", []))):
                raise ValueError(f"{pair_id}/{variant} contains an evaluated physical verdict")
            if pair_id == "port":
                volume_path = arrangement.get("port_volume_report", {}).get("path")
                volume_report_path = safe_package_path(root, volume_path)
                if digest(volume_report_path) != arrangement["port_volume_report"].get("sha256"):
                    raise ValueError(f"port volume report digest mismatch for {variant}")
                volume_report = json.loads(volume_report_path.read_text(encoding="utf-8"))
                if (volume_report.get("geometry_check") != "PASS"
                        or volume_report.get("transport_volume_check") != "PASS"
                        or volume_report.get("scientific_qualification") != "NOT_EVALUATED"
                        or volume_report.get("variant_id") != variant
                        or volume_report.get("scenario_sha256") != scenario_sha
                        or volume_report.get("run_record_sha256")
                        != str(arrangement.get("run_record_sha256", "")).removeprefix("sha256:")):
                    raise ValueError(f"port volume report is not a passing, correctly bound geometry audit for {variant}")
                ownership_ref = arrangement.get("port_geometry_ownership_report")
                if not isinstance(ownership_ref, dict):
                    raise ValueError(f"port ownership audit is missing from package index for {variant}")
                ownership_path = safe_package_path(root, ownership_ref.get("path"))
                if digest(ownership_path) != ownership_ref.get("sha256"):
                    raise ValueError(f"port ownership report digest mismatch for {variant}")
                ownership = json.loads(ownership_path.read_text(encoding="utf-8"))
                if (ownership.get("schema_version") != "faris-packaged-port-geometry-ownership/v0.1"
                        or ownership.get("scenario_sha256") != scenario_sha
                        or ownership.get("variant_id") != variant
                        or ownership.get("run_record_sha256") != arrangement.get("run_record_sha256")
                        or ownership.get("transport_artifact_sha256") != volume_report.get("transport_artifact_sha256")
                        or ownership.get("worker_result_sha256").removeprefix("sha256:")
                        != volume_report.get("worker_result_sha256")
                        or ownership.get("worker_result_sha256") != ownership_ref.get("worker_result_sha256")
                        or ownership.get("input_sha256") != volume_report.get("input_sha256")):
                    raise ValueError(f"port ownership report has inconsistent input/artifact bindings for {variant}")
                try:
                    validate_ownership_audits(
                        ownership.get("geometry_ownership_audit"),
                        ownership.get("penetration_volume_audit"),
                        scenario_sha256=scenario_sha, variant_id=variant,
                        input_sha256=ownership["input_sha256"],
                        component_materials=ownership.get("component_materials"))
                except (TypeError, ValueError, KeyError) as error:
                    raise ValueError(f"port ownership audit failed strict replay for {variant}: {error}") from error
            inspected.append({"pair": pair_id, "variant": variant,
                              "case_id": fresh["case_id"], "record_integrity": fresh["record_integrity"]})
    if seen_variants != {(pair, variant) for pair in ("control", "port")
                         for variant in ("reference", "breeder-emphasis")}:
        raise ValueError("package does not contain the complete four-case matrix")
    return inspected


def verify_package(package: Path, faris: Path, core: Path) -> dict[str, Any]:
    index, _ = verify_index(package, faris, core)
    inspected = inspect_cases(package, index, faris)
    return {"package_index_sha256": digest(package / INDEX), "indexed_file_count": len(index["files"]),
            "inspected_saved_case_count": len(inspected), "saved_cases": inspected}


def mutate_copy_for_negative_control(source: Path, faris: Path, core: Path) -> dict[str, str]:
    with tempfile.TemporaryDirectory(prefix="faris-package-tamper-test-") as temporary:
        target = Path(temporary) / "tampered-package"
        shutil.copytree(source, target, symlinks=True)
        index = json.loads((target / INDEX).read_text(encoding="utf-8"))
        if not index.get("files"):
            raise ValueError("cannot run tamper control on an empty file inventory")
        chosen = index["files"][0]["path"]
        victim = safe_package_path(target.resolve(strict=True), chosen)
        original_bytes_sha = digest(safe_package_path(source.resolve(strict=True), chosen))
        content = bytearray(victim.read_bytes())
        if not content:
            raise ValueError("cannot tamper with an empty indexed artifact")
        content[len(content) // 2] ^= 0x01
        victim.write_bytes(content)
        try:
            verify_package(target, faris, core)
        except ValueError as error:
            if "indexed package file failed size or SHA-256 check" not in str(error):
                raise
            if digest(safe_package_path(source.resolve(strict=True), chosen)) != original_bytes_sha:
                raise ValueError("tamper negative control unexpectedly modified the source package")
            return {"tamper_control": "EXPECTED_REJECTION", "tampered_copy_path": chosen,
                    "original_preserved": True}
        raise ValueError("tampered copy unexpectedly passed package verification")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True, type=Path)
    parser.add_argument("--faris", required=True, type=Path)
    parser.add_argument("--core", required=True, type=Path)
    parser.add_argument("--relocated-copy", required=True, type=Path,
                        help="new path where an unchanged relocated copy is created and inspected")
    args = parser.parse_args()
    source = args.package.resolve(strict=True)
    requested_relocated = args.relocated_copy.absolute()
    if requested_relocated.exists():
        raise SystemExit("relocated-copy must be a new path outside the source package")
    if not args.faris.is_file() or not args.core.is_file():
        raise SystemExit("selected FARIS and Core executables must be regular files")
    requested_relocated.parent.mkdir(parents=True, exist_ok=True)
    relocated = requested_relocated.parent.resolve(strict=True) / requested_relocated.name
    if relocated == source or source in relocated.parents or relocated in source.parents:
        raise SystemExit("relocated-copy must be outside and distinct from the source package")
    shutil.copytree(source, relocated, symlinks=True)
    try:
        result = verify_package(relocated, args.faris.resolve(strict=True), args.core.resolve(strict=True))
        result.update(mutate_copy_for_negative_control(relocated, args.faris.resolve(strict=True),
                                                        args.core.resolve(strict=True)))
    except Exception:
        shutil.rmtree(relocated, ignore_errors=True)
        raise
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
