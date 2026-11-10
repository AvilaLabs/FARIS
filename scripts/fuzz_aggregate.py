#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Combine independent per-target fuzz evidence for one exact source commit."""
import argparse
import json
from pathlib import Path
import sys


def targets(root: Path) -> list[str]:
    return sorted(p.stem for p in (root / "fuzz" / "fuzz_targets").glob("*.rs"))


def aggregate(root: Path, source: Path, output: Path, commit: str) -> dict:
    expected = targets(root)
    if not expected:
        raise ValueError("repository has no fuzz targets")
    found = sorted(path.name for path in source.glob("*.json"))
    required = sorted(f"{target}.json" for target in expected)
    if found != required:
        raise ValueError(f"evidence files differ from repository targets: found {found}")
    checks = []
    metadata = None
    finishes = []
    for target in expected:
        path = source / f"{target}.json"
        try:
            data = json.loads(path.read_text())
        except FileNotFoundError as exc:
            raise ValueError(f"missing evidence for target {target}") from exc
        if not isinstance(data, dict) or data.get("schema_version") != "faris-fuzz-evidence/v0.2":
            raise ValueError(f"{target}: invalid evidence object or schema")
        current = (data.get("commit"), data.get("driver"), data.get("dirty"))
        if current != (commit, "libfuzzer", False):
            raise ValueError(f"{target}: evidence commit/driver/tree is {current}, expected clean libfuzzer at {commit}")
        toolchain = data.get("toolchain")
        requested = data.get("seconds_per_target_requested")
        if not isinstance(toolchain, str) or not toolchain.strip() or type(requested) is not int or requested < 3600:
            raise ValueError(f"{target}: invalid toolchain or requested duration")
        if metadata is None:
            metadata = (toolchain, requested)
        elif metadata != (toolchain, requested):
            raise ValueError(f"{target}: toolchain or requested duration differs")
        finished = data.get("finished")
        if not isinstance(finished, str) or not finished:
            raise ValueError(f"{target}: missing finish timestamp")
        finishes.append(finished)
        entries = data.get("checks")
        if (not isinstance(entries, list) or len(entries) != 1 or
                not isinstance(entries[0], dict) or entries[0].get("name") != target):
            raise ValueError(f"{target}: expected exactly one matching check")
        check = entries[0]
        if (check.get("verdict") != "PASS" or type(check.get("crashes")) is not int or
                check["crashes"] != 0 or type(check.get("exit_status")) is not int or
                check["exit_status"] != 0 or type(check.get("seconds")) is not int or
                check["seconds"] < 3600 or type(check.get("cpu_seconds")) is not int or
                check["cpu_seconds"] < 3600):
            raise ValueError(f"{target}: check did not meet the 3600-second wall and CPU thresholds")
        checks.extend(entries)
    result = {
        "schema_version": "faris-fuzz-evidence/v0.2", "commit": commit, "dirty": False,
        "driver": "libfuzzer", "toolchain": metadata[0],
        "seconds_per_target_requested": metadata[1],
        "finished": max(finishes),
        "checks": checks,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + "\n")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--list-targets", action="store_true")
    parser.add_argument("--input-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--commit")
    args = parser.parse_args()
    if args.list_targets:
        print(json.dumps(targets(args.root)))
        return 0
    if not (args.input_dir and args.output and args.commit):
        parser.error("--input-dir, --output and --commit are required unless --list-targets is used")
    try:
        data = aggregate(args.root, args.input_dir, args.output, args.commit)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"fuzz aggregation failed: {exc}", file=sys.stderr)
        return 1
    print(f"aggregated {len(data['checks'])} target records for {args.commit}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
