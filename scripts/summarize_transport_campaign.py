#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Summarize a transport campaign from its run records, for the release support files.

For each run: identity hashes, sampling plan, wall time, and the mean, standard
error and relative standard error of the headline responses. Optional port
volume checks and history ensembles are attached by run label and must bind to
that run's transport artifact. Reads only; refuses a run that did not succeed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from datetime import date
from pathlib import Path

SCHEMA = "faris-transport-campaign-summary/v1"
HEADLINE_RESPONSES = (
    "total-tritium-production",
    "blanket-tritium",
    "heating-total-whole-model",
    "magnets-fast-flux",
    "magnets-inboard-fast-flux",
    "magnets-outboard-fast-flux",
    "magnets-port-sector-fast-flux",
)
SCOPE = ("Monte Carlo sampling precision of recorded cold-data transport runs only. Standard errors are "
         "batch statistics of the sampled means; they do not include nuclear data, geometry, source or "
         "model-form uncertainty. Research screening only; not a licensing, safety or design basis.")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def labelled(items: list[str], flag: str) -> dict[str, Path]:
    out: dict[str, Path] = {}
    for item in items:
        label, sep, raw = item.partition("=")
        if not sep or not label or not raw:
            raise SystemExit(f"{flag} must use LABEL=PATH")
        if label in out:
            raise SystemExit(f"{flag} repeats label {label}")
        out[label] = Path(raw)
    return out


def headline(results: list[dict]) -> dict:
    found = {}
    for entry in results:
        rid = entry.get("response_id")
        if rid in HEADLINE_RESPONSES and entry.get("domain", {}).get("kind") != "mesh":
            if rid in found:
                raise SystemExit(f"response {rid} appears twice outside the mesh")
            mean, se = entry["mean"], entry["standard_error"]
            found[rid] = {
                "domain": entry["domain"].get("kind"),
                "mean": mean,
                "standard_error": se,
                "unit": entry.get("unit"),
                "relative_standard_error": se / mean if mean else None,
            }
    return found


def summarize_run(label: str, path: Path) -> dict:
    run = json.loads(path.read_text(encoding="utf-8"))
    execution = run.get("execution") or {}
    if execution.get("execution_status") != "SUCCEEDED" or execution.get("exit_code") != 0:
        raise SystemExit(f"run {label} did not succeed: {path}")
    normalized = run["normalized"]
    sampling = run["sampling"]
    return {
        "label": label,
        "run_json_sha256": sha256(path),
        "scenario_id": normalized.get("scenario_id"),
        "scenario_sha256": run["scenario_sha256"],
        "variant_id": run["variant_id"],
        "physics_sha256": run["physics_sha256"],
        "raw_artifact_sha256": run["raw_artifact_sha256"],
        "solver": normalized.get("solver", {}).get("name") + " " + normalized.get("solver", {}).get("version"),
        "histories": normalized["histories"],
        "sampling": sampling,
        "elapsed_seconds": execution.get("elapsed_seconds"),
        "headline_responses": headline(normalized["results"]),
        "scientific_qualification": run.get("scientific_qualification"),
    }


def attach_volume_check(summary: dict, path: Path) -> None:
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("transport_artifact_sha256") != summary["raw_artifact_sha256"]:
        raise SystemExit(f"volume check {path} is bound to another transport artifact")
    summary["volume_check"] = {
        "report_sha256": sha256(path),
        "geometry_check": report.get("geometry_check"),
        "transport_volume_check": report.get("transport_volume_check"),
        "source_containment": report.get("source_containment"),
    }


def attach_ensemble(summary: dict, path: Path) -> None:
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("transport_artifact_sha256") != summary["raw_artifact_sha256"]:
        raise SystemExit(f"ensemble {path} is bound to another transport artifact")
    requested, rejections = report.get("samples_requested"), report.get("rejections")
    summary["history_ensemble"] = {
        "report_sha256": sha256(path),
        "method": report.get("method"),
        "status": report.get("status"),
        "samples_requested": requested,
        "samples_accepted": report.get("samples_accepted"),
        "rejections": rejections,
        "rejected_fraction_of_draws": (rejections / (requested + rejections)
                                       if isinstance(requested, int) and isinstance(rejections, int) else None),
        "seed": report.get("seed"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--run", action="append", default=[], required=True, metavar="LABEL=RUN_JSON")
    parser.add_argument("--volume-check", action="append", default=[], metavar="LABEL=JSON")
    parser.add_argument("--ensemble", action="append", default=[], metavar="LABEL=JSON")
    parser.add_argument("--note", default="", help="one-line campaign description")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise SystemExit(f"{args.output} exists")
    runs = labelled(args.run, "--run")
    volumes = labelled(args.volume_check, "--volume-check")
    ensembles = labelled(args.ensemble, "--ensemble")
    for label in [*volumes, *ensembles]:
        if label not in runs:
            raise SystemExit(f"no --run with label {label}")
    summaries = []
    for label, path in runs.items():
        summary = summarize_run(label, path)
        if label in volumes:
            attach_volume_check(summary, volumes[label])
        if label in ensembles:
            attach_ensemble(summary, ensembles[label])
        summaries.append(summary)
    report = {"schema_version": SCHEMA, "created": date.today().isoformat(), "note": args.note,
              "headline_responses": list(HEADLINE_RESPONSES), "runs": summaries, "scope": SCOPE}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
