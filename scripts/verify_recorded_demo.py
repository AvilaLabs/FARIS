#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Rehash and reopen a relocated recorded demo package, with a tamper control."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import tempfile
from typing import Any
SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
sys.dont_write_bytecode = True
from port_geometry_contract import validate_ownership_audits
from recorded_bundle_contract import validate_recorded_bundle
from recorded_archives import (extract_indexed_trees, MAX_EXPANDED_BYTES, MAX_MEMBERS,
                               MAX_TREE_BYTES, MAX_TREE_FILES, MAX_TREE_MEMBERS,
                               MAX_TREE_DIRECTORIES, MAX_EXPANDED_DIRECTORIES,
                               MAX_PATH_COMPONENTS, count_implicit_directories)

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


def bare_sha256(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label}: missing SHA-256 identity")
    result = value.removeprefix("sha256:")
    if len(result) != 64:
        raise ValueError(f"{label}: malformed SHA-256 identity")
    try:
        bytes.fromhex(result)
    except ValueError as error:
        raise ValueError(f"{label}: malformed SHA-256 identity") from error
    return result.lower()


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
    if index.get("schema_version") != "faris-recorded-demo-package/v0.4":
        raise ValueError("unsupported recorded demo package schema")
    if index.get("status") != "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED":
        raise ValueError("package status does not preserve the required NOT_EVALUATED scope")
    if digest(faris.resolve(strict=True)) != index.get("faris_cli_sha256"):
        raise ValueError("selected FARIS executable differs from the package pin")
    if digest(core.resolve(strict=True)) != index.get("core_executable_sha256"):
        raise ValueError("selected Core executable differs from the package pin")
    runtime = index.get("local_runtime")
    if (not isinstance(runtime, dict)
            or runtime.get("schema_version") != "faris-local-runtime/v0.1"
            or runtime.get("platform") != {"sys_platform": sys.platform, "machine": platform.machine()}):
        raise ValueError("package local runtime is missing or targets another platform")
    executables = runtime.get("executables")
    if not isinstance(executables, dict) or set(executables) != {"faris", "faris-app", "avila-core"}:
        raise ValueError("package executable manifest is malformed")
    expected_binaries = {"faris": index.get("faris_cli_sha256"),
                         "faris-app": index.get("faris_app_sha256"),
                         "avila-core": index.get("core_executable_sha256")}
    for name, record in executables.items():
        relative = record.get("path") if isinstance(record, dict) else None
        binary_path = safe_package_path(root, relative)
        if (digest(binary_path) != record.get("sha256")
                or digest(binary_path) != expected_binaries[name]
                or binary_path.stat().st_size != record.get("bytes")
                or not binary_path.stat().st_mode & 0o111):
            raise ValueError(f"package-pinned executable changed: {name}")
    for role in ("launcher", "verifier"):
        descriptor = runtime.get(role)
        launcher_path = safe_package_path(root, descriptor.get("path") if isinstance(descriptor, dict) else None)
        if digest(launcher_path) != descriptor.get("sha256"):
            raise ValueError(f"package {role} script digest mismatch")
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
    license_files = runtime.get("license_files")
    expected_license_files = {
        "licenses/faris-LICENSE", "licenses/core-LICENSE",
        "licenses/core-THIRD_PARTY_NOTICES.md",
        "licenses/faris-THIRD_PARTY_NOTICES.md",
        "licenses/core-RUNTIME_DEPENDENCY_NOTICES.md",
        "licenses/rust-1.98.1-COPYRIGHT-library.html",
    }
    if (not isinstance(license_files, dict)
            or not expected_license_files <= set(license_files)
            or not any(path.startswith("licenses/core-LICENSES/") for path in license_files)
            or any(path not in inventory or inventory[path].get("sha256") != expected_sha
                   for path, expected_sha in license_files.items())
            or "SOURCE_PROVENANCE.md" not in inventory):
        raise ValueError("package does not include its pinned source provenance and licenses")
    if any(Path(path).name == "DEMO_ACCEPTANCE.md" for path in inventory):
        raise ValueError("package contains a stale DEMO_ACCEPTANCE snapshot")
    sources = runtime.get("source_provenance")
    for role in ("faris", "core"):
        source = sources.get(role) if isinstance(sources, dict) else None
        if not isinstance(source, dict):
            raise ValueError(f"package source provenance is missing for {role}")
        commit = source.get("commit")
        if (not isinstance(source.get("repository"), str) or not source["repository"]
                or not isinstance(commit, str) or len(commit) != 40
                or any(character not in "0123456789abcdef" for character in commit.lower())):
            raise ValueError(f"package source provenance is malformed for {role}")
    if total_bytes > 512 * 1024 * 1024:
        raise ValueError("package exceeds the declared 512 MiB bound")
    if (index.get("package_file_count") != len(files)
            or index.get("package_bytes") != total_bytes):
        raise ValueError("package index total file count/byte measurement is incorrect")
    if (index.get("expanded_size_cap_bytes") != MAX_EXPANDED_BYTES
            or index.get("expanded_file_count_cap") != MAX_MEMBERS
            or index.get("expanded_archive_member_count_cap") != MAX_MEMBERS
            or index.get("expanded_directory_count_cap") != MAX_EXPANDED_DIRECTORIES
            or index.get("per_tree_expanded_size_cap_bytes") != MAX_TREE_BYTES
            or index.get("per_tree_file_count_cap") != MAX_TREE_FILES
            or index.get("per_tree_archive_member_count_cap") != MAX_TREE_MEMBERS
            or index.get("per_tree_directory_count_cap") != MAX_TREE_DIRECTORIES
            or index.get("archive_path_component_count_cap") != MAX_PATH_COMPONENTS):
        raise ValueError("package expansion caps differ from the bounded extractor")
    expanded_bytes = 0
    expanded_files = 0
    expanded_members = 0
    expanded_directories = 0
    compressed_archives = 0
    for pair in index.get("scenario_pairs", []):
        for arrangement in pair.get("arrangements", []):
            descriptor_path = safe_package_path(root, arrangement.get("saved_study_descriptor"))
            if digest(descriptor_path) != arrangement.get("saved_study_descriptor_sha256"):
                raise ValueError("saved-study archive descriptor digest mismatch")
            descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
            if (descriptor.get("schema_version") != "faris-saved-study-archive/v0.1"
                    or descriptor.get("execution_report_member") != "execution-report.json"):
                raise ValueError("saved-study archive descriptor is malformed")
            for kind in ("case", "workspace"):
                archive = arrangement.get(f"{kind}_archive")
                if not isinstance(archive, dict) or descriptor.get(f"{kind}_archive") != archive:
                    raise ValueError(f"{kind} archive descriptor mismatch")
                archive_path = safe_package_path(root, archive.get("path"))
                manifest_path = safe_package_path(root, archive.get("manifest_path"))
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                if (digest(archive_path) != archive.get("sha256")
                        or digest(manifest_path) != archive.get("manifest_sha256")
                        or archive_path.stat().st_size != archive.get("bytes")
                        or manifest.get("archive_sha256") != archive.get("sha256")
                        or manifest.get("archive_bytes") != archive.get("bytes")
                        or manifest.get("expanded_bytes") != archive.get("expanded_bytes")
                        or manifest.get("file_count") != archive.get("file_count")
                        or manifest.get("archive_member_count") != archive.get("archive_member_count")
                        or manifest.get("directory_count") != archive.get("directory_count")
                        or manifest.get("directory_count") != count_implicit_directories(manifest.get("members"))):
                    raise ValueError(f"{kind} archive manifest identity/size mismatch")
                caps = (("bytes", MAX_FILE_BYTES), ("expanded_bytes", MAX_TREE_BYTES),
                        ("file_count", MAX_TREE_FILES),
                        ("archive_member_count", MAX_TREE_MEMBERS),
                        ("directory_count", MAX_TREE_DIRECTORIES))
                if any(not isinstance(archive.get(key), int) or isinstance(archive.get(key), bool)
                       or archive[key] < 0 or archive[key] > maximum
                       for key, maximum in caps):
                    raise ValueError(f"{kind} archive exceeds individual Core tree caps")
                expanded_bytes += int(archive.get("expanded_bytes", -1))
                expanded_files += int(archive.get("file_count", -1))
                expanded_members += int(archive.get("archive_member_count", -1))
                expanded_directories += int(archive.get("directory_count", -1))
                compressed_archives += int(archive.get("bytes", -1))
    if (expanded_bytes > MAX_EXPANDED_BYTES or expanded_files > MAX_MEMBERS
            or expanded_members > MAX_MEMBERS or expanded_directories > MAX_EXPANDED_DIRECTORIES
            or index.get("expanded_case_workspace_bytes") != expanded_bytes
            or index.get("expanded_case_workspace_file_count") != expanded_files
            or expanded_members > MAX_MEMBERS
            or index.get("expanded_case_workspace_member_count") != expanded_members
            or index.get("expanded_case_workspace_directory_count") != expanded_directories
            or index.get("compressed_case_workspace_archive_bytes") != compressed_archives):
        raise ValueError("expanded case/workspace totals exceed or differ from their declared limits")
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
    support_ref = index.get("support")
    support_path = safe_package_path(root, support_ref.get("path") if isinstance(support_ref, dict) else None)
    if digest(support_path) != support_ref.get("sha256"):
        raise ValueError("support manifest digest mismatch")
    support_manifest = json.loads(support_path.read_text(encoding="utf-8"))
    support_files = support_manifest.get("files")
    if (support_manifest.get("schema_version") != "faris-recorded-demo-support/v0.1"
            or support_manifest.get("demo_acceptance_snapshot_included") is not False
            or support_ref.get("demo_acceptance_snapshot_included") is not False
            or not isinstance(support_files, list)
            or support_ref.get("file_count") != len(support_files)):
        raise ValueError("support manifest is malformed or includes a stale acceptance snapshot")
    if digest(safe_package_path(root, "support/README.md")) != support_manifest.get("support_readme_sha256"):
        raise ValueError("support README digest mismatch")
    for item in support_files:
        relative = item.get("package_path") if isinstance(item, dict) else None
        if (not isinstance(relative, str)
                or relative.endswith("/DEMO_ACCEPTANCE.md")):
            raise ValueError("support manifest contains an unsafe or forbidden acceptance path")
        packaged = safe_package_path(root, relative)
        source_sha = item.get("source_sha256")
        source_sha_bare = source_sha.removeprefix("sha256:") if isinstance(source_sha, str) else ""
        if (relative not in inventory
                or digest(packaged) != item.get("package_sha256")
                or len(source_sha_bare) != 64
                or any(character not in "0123456789abcdefABCDEF" for character in source_sha_bare)):
            raise ValueError(f"packaged scientific support identity mismatch: {relative}")
    return index, inventory


def verify_outage_duration_study(root: Path, index: dict[str, Any]) -> None:
    ref = index.get("outage_duration_sensitivity")
    if (not isinstance(ref, dict) or ref.get("case_count") != 12
            or ref.get("multipliers") != [0.5, 1.0, 2.0]
            or ref.get("duration_days") != [15, 30, 60]):
        raise ValueError("package does not identify the complete frozen outage-duration axis")
    summary_path = safe_package_path(root, ref.get("path"))
    if digest(summary_path) != ref.get("sha256"):
        raise ValueError("outage-duration summary digest mismatch")
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    if (summary.get("schema_version") != "faris-outage-duration-study/v0.1"
            or summary.get("status") != "COMPLETED_AUTHORED_SCENARIO_PROBES_NOT_PHYSICAL_UNCERTAINTY"
            or summary.get("interpretation") != "AUTHORED_SCENARIO_PROBE"
            or summary.get("not_probability_distribution") is not True
            or summary.get("not_physical_uncertainty") is not True
            or summary.get("not_availability_estimate") is not True
            or summary.get("axis", {}).get("multipliers") != [0.5, 1.0, 2.0]
            or summary.get("axis", {}).get("base_outage_duration_days") != 30
            or summary.get("axis", {}).get("resulting_duration_days") != [15, 30, 60]
            or summary.get("axis", {}).get("fixed") != [
                "outage start times", "outage spacing", "all non-duration assumptions",
                "transport source rates", "scenario", "variant", "operating history horizon"]
            or summary.get("axis", {}).get("maximum_outage_below_annual_spacing") is not True):
        raise ValueError("outage-duration summary has incomplete scope or axis metadata")
    records = summary.get("records")
    if not isinstance(records, list) or len(records) != 12:
        raise ValueError("outage-duration summary must have exactly 12 records")
    base_sha = index.get("operating_assumptions_sha256")
    base_path = safe_package_path(root, "operating-assumptions.json")
    if digest(base_path) != base_sha:
        raise ValueError("base operating assumptions digest mismatch for outage-duration axis")
    base = json.loads(base_path.read_text(encoding="utf-8"))
    base_outages = base.get("planned_outages")
    if not isinstance(base_outages, list) or not base_outages:
        raise ValueError("base operating assumptions lack the annual outage schedule")
    for outage_index, outage in enumerate(base_outages):
        if (outage.get("end_s") - outage.get("start_s") != 30 * 86400
                or (outage_index > 0 and outage.get("start_s") - base_outages[outage_index - 1].get("start_s")
                    != 365.25 * 86400)):
            raise ValueError("base assumptions do not match the frozen 30-day annual outage axis")
    arrangements: dict[tuple[str, str], dict] = {}
    for pair in index.get("scenario_pairs", []):
        pair_id = pair.get("scenario_path", "").split("/", 1)[0]
        for arrangement in pair.get("arrangements", []):
            arrangements[(pair_id, arrangement.get("variant_id"))] = arrangement
    seen: set[tuple[str, str, float]] = set()
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("malformed outage-duration record")
        pair_id, variant, multiplier = (record.get("pair_id"), record.get("variant_id"),
                                        record.get("duration_multiplier"))
        key = (pair_id, variant, multiplier)
        arrangement = arrangements.get((pair_id, variant))
        if (key in seen or isinstance(multiplier, bool)
                or not isinstance(multiplier, (int, float)) or not math.isfinite(multiplier)
                or multiplier not in [0.5, 1.0, 2.0] or arrangement is None):
            raise ValueError("outage-duration record has duplicate or unknown run identity")
        seen.add(key)
        paths = ("assumptions", "history", "rates", "provenance")
        loaded = {}
        for name in paths:
            path = safe_package_path(root, record.get(f"{name}_path"))
            if digest(path) != record.get(f"{name}_sha256"):
                raise ValueError(f"outage-duration {name} digest mismatch")
            loaded[name] = json.loads(path.read_text(encoding="utf-8"))
        adjusted = loaded["assumptions"]
        outages = adjusted.get("planned_outages")
        if not isinstance(outages, list) or len(outages) != len(base_outages or []):
            raise ValueError("outage-duration assumptions change outage count")
        expected = json.loads(json.dumps(base))
        for source, target in zip(base_outages, outages, strict=True):
            duration = source["end_s"] - source["start_s"]
            if (target.get("start_s") != source.get("start_s")
                    or target.get("end_s") != source.get("start_s") + duration * multiplier):
                raise ValueError("outage-duration assumptions change start times or use the wrong duration")
            if target.get("end_s") - target.get("start_s") >= 365.25 * 86400:
                raise ValueError("outage-duration axis contains an overlapping annual outage")
        expected["planned_outages"] = outages
        if adjusted != expected:
            raise ValueError("outage-duration assumptions changed a non-duration input")
        history, rates, provenance = loaded["history"], loaded["rates"], loaded["provenance"]
        raw = arrangement.get("raw_artifact_sha256")
        if (history.get("schema_version") != "faris-history-result/v0.1"
                or history.get("assumptions") != adjusted
                or bare_sha256(history.get("driving_rates", {}).get("scenario_sha256"), str(summary_path))
                != arrangement.get("scenario_sha256")
                or bare_sha256(history.get("driving_rates", {}).get("transport_artifact_sha256"), str(summary_path))
                != raw
                or bare_sha256(rates.get("transport_artifact_sha256"), str(summary_path)) != raw
                or not history.get("snapshots")):
            raise ValueError("outage-duration history does not match the bound transport run")
        if (provenance.get("schema_version") != "faris-outage-duration-provenance/v0.1"
                or provenance.get("pair_id") != pair_id
                or provenance.get("variant_id") != variant
                or provenance.get("duration_multiplier") != multiplier
                or provenance.get("run_record_sha256") != arrangement.get("run_record_sha256")
                or provenance.get("raw_artifact_sha256") != raw
                or provenance.get("input_sha256") != arrangement.get("input_sha256")
                or provenance.get("sampling") != arrangement.get("sampling")
                or provenance.get("scenario_sha256") != arrangement.get("scenario_sha256")
                or provenance.get("adjusted_assumptions_sha256") != record.get("assumptions_sha256")
                or provenance.get("history_sha256") != record.get("history_sha256")
                or provenance.get("rates_sha256") != record.get("rates_sha256")
                or provenance.get("base_operating_assumptions_sha256") != base_sha
                or provenance.get("interpretation") != "AUTHORED_SCENARIO_PROBE"
                or provenance.get("not_probability_distribution") is not True
                or provenance.get("not_physical_uncertainty") is not True
                or provenance.get("not_availability_estimate") is not True):
            raise ValueError("outage-duration provenance is not bound to its exact run and inputs")
    expected = {(pair_id, variant, multiplier)
                for pair_id, variant in arrangements for multiplier in [0.5, 1.0, 2.0]}
    if seen != expected:
        raise ValueError("outage-duration records do not cover every pair, variant, and level")


def inspect_cases(package: Path, index: dict[str, Any], faris: Path,
                  core: Path, extracted: dict[tuple[str, str], tuple[Path, Path]]) -> list[dict[str, str]]:
    root = package.resolve(strict=True)
    event_ref = index.get("event_assumptions")
    grid_ref = index.get("sensitivity_grid")
    event_path = safe_package_path(root, event_ref.get("path") if isinstance(event_ref, dict) else None)
    grid_path = safe_package_path(root, grid_ref.get("path") if isinstance(grid_ref, dict) else None)
    if digest(event_path) != event_ref.get("sha256") or digest(grid_path) != grid_ref.get("sha256"):
        raise ValueError("event assumptions or sensitivity grid digest mismatch")
    event_assumptions = json.loads(event_path.read_text(encoding="utf-8"))
    sensitivity_grid = json.loads(grid_path.read_text(encoding="utf-8"))
    if grid_ref.get("points_per_run") != 27:
        raise ValueError("package does not identify the complete 27-point sensitivity grid")
    main_assumptions = safe_package_path(root, "operating-assumptions.json")
    if digest(main_assumptions) != index.get("operating_assumptions_sha256"):
        raise ValueError("base operating assumptions digest mismatch")
    main_assumptions_data = json.loads(main_assumptions.read_text(encoding="utf-8"))
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
            if (set(descriptor) != {"schema_version", "case_archive", "workspace_archive",
                                   "execution_report_member"}
                    or descriptor.get("schema_version") != "faris-saved-study-archive/v0.1"):
                raise ValueError(f"unsupported saved-study archive descriptor: {descriptor_rel}")
            if (descriptor.get("case_archive") != arrangement.get("case_archive")
                    or descriptor.get("workspace_archive") != arrangement.get("workspace_archive")
                    or descriptor.get("execution_report_member") != "execution-report.json"):
                raise ValueError(f"saved-study archive paths differ from indexed case for {pair_id}/{variant}")
            case, workspace = extracted[(pair_id, variant)]
            report = case / "execution-report.json"
            if (not report.is_file()
                    or digest(report) != arrangement.get("core_execution_report_sha256")
                    or arrangement.get("core_execution_report_member") != "execution-report.json"):
                raise ValueError(f"extracted Core execution report is missing or changed for {pair_id}/{variant}")
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
            export_report_rel = arrangement.get("core_export_report")
            export_report_path = safe_package_path(root, export_report_rel)
            if digest(export_report_path) != arrangement.get("core_export_report_sha256"):
                raise ValueError(f"Core export report digest mismatch for {pair_id}/{variant}")
            stored_export_report = json.loads(export_report_path.read_text(encoding="utf-8"))
            with tempfile.TemporaryDirectory(prefix="faris-export-recheck-") as temporary:
                export_dir = Path(temporary) / "export"
                export = subprocess.run(
                    [str(core), "export", str(case), "--source-root", f"case={case}",
                     "--out", str(export_dir)],
                    check=False, capture_output=True, text=True, timeout=180,
                )
                if export.returncode != 0:
                    raise ValueError(f"Core export recheck failed for {pair_id}/{variant}: {export.stderr[-2000:]}")
                try:
                    reconstructed_export = json.loads(export.stdout)
                    written_export = json.loads((export_dir / "export-report.json").read_text(encoding="utf-8"))
                except (json.JSONDecodeError, OSError) as exc:
                    raise ValueError(f"Core export recheck returned invalid report for {pair_id}/{variant}") from exc
            if (reconstructed_export != stored_export_report
                    or written_export != stored_export_report
                    or stored_export_report.get("export_sha256") != arrangement.get("core_export_sha256")):
                raise ValueError(f"Core export report no longer reconstructs from canonical case for {pair_id}/{variant}")
            verdicts = fresh.get("requirement_verdicts", [])
            if (arrangement.get("scientific_qualification") != "NOT_EVALUATED"
                    or not verdicts
                    or any(item.get("status") != "not_evaluated" for item in verdicts)
                    or not arrangement.get("core_requirement_verdicts")
                    or any(item != "not_evaluated"
                           for item in arrangement.get("core_requirement_verdicts", []))):
                raise ValueError(f"{pair_id}/{variant} contains an evaluated physical verdict")
            bundle_rel = arrangement.get("transport_bundle")
            bundle_path = safe_package_path(root, bundle_rel)
            if digest(bundle_path) != arrangement.get("transport_bundle_sha256"):
                raise ValueError(f"recorded transport bundle digest mismatch for {pair_id}/{variant}")
            bundle_summary = validate_recorded_bundle(
                json.loads(bundle_path.read_text(encoding="utf-8")),
                scenario_sha256=scenario_sha, variant_id=variant,
                expected_run_sha256=arrangement.get("run_record_sha256"),
                expected_raw_artifact_sha256=arrangement.get("raw_artifact_sha256"),
                mesh_nonzero_flux_bin_count=arrangement.get("mesh_nonzero_flux_bin_count"))
            if bundle_summary != arrangement.get("offline_field_and_spectrum_identity"):
                raise ValueError(f"field/spectrum identity summary mismatch for {pair_id}/{variant}")
            event_ref = arrangement.get("event_history")
            sensitivity_ref = arrangement.get("sensitivity_study")
            if not isinstance(event_ref, dict) or not isinstance(sensitivity_ref, dict):
                raise ValueError(f"missing event/sensitivity artifact references for {pair_id}/{variant}")
            event_history_path = safe_package_path(root, event_ref.get("history_path"))
            rates_path = safe_package_path(root, event_ref.get("rates_path"))
            event_provenance_path = safe_package_path(root, event_ref.get("provenance_path"))
            if (digest(event_history_path) != event_ref.get("history_sha256")
                    or digest(rates_path) != event_ref.get("rates_sha256")
                    or digest(event_provenance_path) != event_ref.get("provenance_sha256")):
                raise ValueError(f"event history content digest mismatch for {pair_id}/{variant}")
            event_history = json.loads(event_history_path.read_text(encoding="utf-8"))
            event_rates = json.loads(rates_path.read_text(encoding="utf-8"))
            event_provenance = json.loads(event_provenance_path.read_text(encoding="utf-8"))
            if (event_history.get("schema_version") != "faris-history-result/v0.1"
                    or event_history.get("assumptions") != event_assumptions
                    or event_history.get("driving_rates", {}).get("scenario_sha256")
                    != event_provenance.get("scenario_sha256")
                    or event_history.get("driving_rates", {}).get("transport_artifact_sha256")
                    != arrangement.get("raw_artifact_sha256")
                    or event_rates.get("transport_artifact_sha256") != arrangement.get("raw_artifact_sha256")
                    or not {"planned_outage_started", "planned_outage_ended"}
                    <= {event.get("kind") for event in event_history.get("events", [])}
                    or event_provenance.get("schema_version") != "faris-packaged-event-history-provenance/v0.1"
                    or event_provenance.get("history_sha256") != event_ref.get("history_sha256")
                    or event_provenance.get("rates_sha256") != event_ref.get("rates_sha256")
                    or event_provenance.get("run_record_sha256") != arrangement.get("run_record_sha256")
                    or event_provenance.get("raw_artifact_sha256") != arrangement.get("raw_artifact_sha256")
                    or event_provenance.get("variant_id") != variant
                    or event_provenance.get("scenario_sha256") != scenario_sha
                    or event_provenance.get("assumptions_sha256") != event_ref.get("assumptions_sha256")):
                raise ValueError(f"event history is not bound to the indexed run/assumptions for {pair_id}/{variant}")
            sensitivity_path = safe_package_path(root, sensitivity_ref.get("sensitivity_path"))
            sensitivity_provenance_path = safe_package_path(root, sensitivity_ref.get("provenance_path"))
            if (digest(sensitivity_path) != sensitivity_ref.get("sensitivity_sha256")
                    or digest(sensitivity_provenance_path) != sensitivity_ref.get("provenance_sha256")):
                raise ValueError(f"sensitivity content digest mismatch for {pair_id}/{variant}")
            sensitivity = json.loads(sensitivity_path.read_text(encoding="utf-8"))
            sensitivity_provenance = json.loads(sensitivity_provenance_path.read_text(encoding="utf-8"))
            if (sensitivity.get("schema_version") != "faris-history-sensitivity/v0.1"
                    or sensitivity.get("grid") != sensitivity_grid
                    or sensitivity.get("base_assumptions") != main_assumptions_data
                    or len(sensitivity.get("points", [])) != 27
                    or sensitivity.get("driving_rates", {}).get("scenario_sha256") != scenario_sha
                    or sensitivity.get("driving_rates", {}).get("transport_artifact_sha256")
                    != arrangement.get("raw_artifact_sha256")
                    or sensitivity_provenance.get("schema_version")
                    != "faris-packaged-history-sensitivity-provenance/v0.1"
                    or sensitivity_provenance.get("sensitivity_sha256") != sensitivity_ref.get("sensitivity_sha256")
                    or sensitivity_provenance.get("run_record_sha256") != arrangement.get("run_record_sha256")
                    or sensitivity_provenance.get("raw_artifact_sha256") != arrangement.get("raw_artifact_sha256")
                    or sensitivity_provenance.get("grid_sha256") != grid_ref.get("sha256")
                    or sensitivity_provenance.get("assumptions_sha256") != index.get("operating_assumptions_sha256")
                    or sensitivity_provenance.get("variant_id") != variant
                    or sensitivity_provenance.get("scenario_sha256") != scenario_sha):
                raise ValueError(f"sensitivity study is not bound to its run/grid for {pair_id}/{variant}")
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
        comparison_ref = pair.get("paired_history_comparison")
        if not isinstance(comparison_ref, dict):
            raise ValueError(f"paired history comparison missing for {pair_id}")
        comparison_path = safe_package_path(root, comparison_ref.get("comparison_path"))
        comparison_provenance_path = safe_package_path(root, comparison_ref.get("provenance_path"))
        if (digest(comparison_path) != comparison_ref.get("comparison_sha256")
                or digest(comparison_provenance_path) != comparison_ref.get("provenance_sha256")):
            raise ValueError(f"paired comparison digest mismatch for {pair_id}")
        comparison = json.loads(comparison_path.read_text(encoding="utf-8"))
        comparison_provenance = json.loads(comparison_provenance_path.read_text(encoding="utf-8"))
        run_by_variant = {item["variant_id"]: item for item in arrangements}
        if (comparison.get("schema_version") != "faris-history-comparison/v0.1"
                or comparison.get("left_label") != "reference"
                or comparison.get("right_label") != "breeder-emphasis"
                or comparison_provenance.get("schema_version") != "faris-packaged-history-comparison-provenance/v0.1"
                or comparison_provenance.get("comparison_sha256") != comparison_ref.get("comparison_sha256")
                or comparison_provenance.get("assumptions_sha256") != index.get("operating_assumptions_sha256")
                or comparison_provenance.get("scenario_sha256") != scenario_sha):
            raise ValueError(f"paired comparison is invalid for {pair_id}")
        for side, variant in (("left", "reference"), ("right", "breeder-emphasis")):
            arrangement = run_by_variant[variant]
            rates = comparison.get(side, {}).get("driving_rates", {})
            provenance = comparison_provenance.get(side, {})
            if (rates.get("scenario_sha256") != scenario_sha
                    or rates.get("transport_artifact_sha256") != arrangement.get("raw_artifact_sha256")
                    or provenance.get("scenario_sha256") != scenario_sha
                    or provenance.get("variant_id") != variant
                    or provenance.get("run_record_sha256") != arrangement.get("run_record_sha256")
                    or provenance.get("raw_artifact_sha256") != arrangement.get("raw_artifact_sha256")):
                raise ValueError(f"paired comparison source identity mismatch for {pair_id}/{variant}")
    if seen_variants != {(pair, variant) for pair in ("control", "port")
                         for variant in ("reference", "breeder-emphasis")}:
        raise ValueError("package does not contain the complete four-case matrix")
    return inspected


def verify_package(package: Path, faris: Path, core: Path) -> dict[str, Any]:
    index, _ = verify_index(package, faris, core)
    verify_outage_duration_study(package.resolve(strict=True), index)
    expanded = int(index["expanded_case_workspace_bytes"])
    directory_count = int(index["expanded_case_workspace_directory_count"])
    largest_case = max(int(item["case_archive"]["expanded_bytes"])
                       for pair in index["scenario_pairs"] for item in pair["arrangements"])
    temporary_parent = Path(tempfile.gettempdir())
    block_bytes = max(4096, os.statvfs(temporary_parent).f_frsize)
    required_scratch = expanded + largest_case + (directory_count + 16) * block_bytes + 64 * 1024 * 1024
    free = shutil.disk_usage(temporary_parent).free
    if free < required_scratch:
        raise ValueError(f"verification needs {required_scratch} bytes free for expansion and one Core export; have {free}")
    with tempfile.TemporaryDirectory(prefix="faris-package-extract-") as temporary:
        extracted = extract_indexed_trees(package.resolve(strict=True), index, Path(temporary) / "materialized")
        inspected = inspect_cases(package, index, faris, core, extracted)
    return {"package_index_sha256": digest(package / INDEX), "indexed_file_count": len(index["files"]),
            "inspected_saved_case_count": len(inspected), "saved_cases": inspected,
            "expanded_case_workspace_bytes": expanded,
            "archive_integrity_status": "EXPANDED_HASHES_AND_CORE_RECEIPTS_REVALIDATED"}


def mutate_copy_for_negative_control(source: Path, faris: Path, core: Path) -> dict[str, str]:
    package_bytes = sum(item["bytes"] for item in json.loads(
        (source / INDEX).read_text(encoding="utf-8"))["files"])
    free = shutil.disk_usage(Path(tempfile.gettempdir())).free
    if free < package_bytes + 64 * 1024 * 1024:
        raise ValueError("tamper control lacks scratch space for a relocated package copy")
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
        victim.chmod(victim.stat().st_mode | stat.S_IWUSR)
        victim.write_bytes(content)
        try:
            verify_package(target, faris, core)
        except ValueError:
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
    source_index, _ = verify_index(source, args.faris.resolve(strict=True), args.core.resolve(strict=True))
    package_bytes = source_index["package_bytes"]
    expanded_bytes = source_index["expanded_case_workspace_bytes"]
    directory_count = source_index["expanded_case_workspace_directory_count"]
    largest_case = max(int(item["case_archive"]["expanded_bytes"])
                       for pair in source_index["scenario_pairs"] for item in pair["arrangements"])
    free = shutil.disk_usage(relocated.parent).free
    block_bytes = max(4096, os.statvfs(relocated.parent).f_frsize)
    required_scratch = package_bytes + expanded_bytes + largest_case + (directory_count + 16) * block_bytes + 64 * 1024 * 1024
    if free < required_scratch:
        raise SystemExit(f"relocation/replay needs {required_scratch} bytes free for compressed copy, expanded trees, and Core export; have {free}")
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
