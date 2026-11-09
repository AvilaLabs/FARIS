#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build the FNS Fe 1996 five-minute decay-heat case from pinned external bytes.

Example:
    python3 validation/cases/fns-decayheat-fe-1996-5min/build_manifest.py \\
        --upstream /path/to/AvilaLabs/open-benchmarks

The source spec and measurement table remain outside FARIS. The manifest carries
the small, attributed reference table and exact source identities.
"""
from __future__ import annotations

import argparse
import csv
from decimal import Decimal, InvalidOperation
import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT))
from validation.manifest import SCHEMA, check_manifest, verify_files  # noqa: E402

CASE_ID = "fns-decayheat-fe-1996-5min"
SOURCE_COMMIT = "1914ed418b597398d758e7c6fa11e83158e2ca21"
SOURCE_ROOT = "jade_open_benchmarks"
SPEC_REL = f"{SOURCE_ROOT}/inputs/FNS-DecayHeat/Fe-1996-5min/actinv/spec.json"
CSV_REL = f"{SOURCE_ROOT}/exp_results/FNS-DecayHeat/Fe-1996-5min Decay heat.csv"
METADATA_REL = f"{SOURCE_ROOT}/inputs/FNS-DecayHeat/benchmark_metadata.json"
SOURCE_NOTE_REL = f"{SOURCE_ROOT}/fns_decay_heat.md"
LICENSE_REL = "LICENSE"
PINNED_COMMIT_ONLY = {
    SOURCE_NOTE_REL: ("7eaf6669f0aa3942579541cdf566cf41d9ea167fb3eb3fcea759028882740e11", 3_419),
    LICENSE_REL: ("9e5f1b3c610b9c2da5c313bf81d577a7d1acec686bdb0384edefa6df0f90cd94", 18_656),
}
PINNED_FILES = {
    SPEC_REL: ("ACTINV irradiation and cooling specification", "72fb487dbc780c4fa3a316752615c934406b1e32e9549ee498b407e66748b558", 16_384),
    CSV_REL: ("published FNS iron decay-heat measurements", "98ed4d350b3c88335d86ed8b056de556af88cc207a80acb37d1cde0343ad74b8", 1_024),
    METADATA_REL: ("ACTINV benchmark metadata", "9a1b188ce8593a85df1a728fa03ea146b9ac5692ae022395a4cffafdf56c5c4b", 1_024),
}
ARCHIVE_URL = "https://www-nds.iaea.org/conderc/fusion/files/fns.zip"
ARCHIVE_SHA256 = "ba1dd6cb150a4aa3e0d81461054aec7d415ef19d946aba8b9886b31de218252d"
ARCHIVE_MEMBERS = {
    "fns/Fe/1996exp_5min.exp": "4eb4635d1d70498f691284d396d2a9276beb8ff161e1f568d1622256c80b6c64",
    "fns/Fe/1996exp_5min_fluxes": "6c2a23393bd71bc5ac957ee53ca6b6ea747b8b0ed87d19361053f37505937d66",
    "fns/Fe/TENDL-2017_1996exp_5min.i": "fdb943333dd293cdda057fad49df8bf152000cf3f48fa64b12afd54a57eda071",
    "fns/Fe/total_1996exp_5min.pdf": "d6ea06aadbf0fc9c2ad91452b32e8e0e64afe692c5624d07ccd03fb1673bb876",
}
TOTAL_FLUX = 1.116e10
IRRADIATION_SECONDS = Decimal("300")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_pinned(upstream: Path, relative: str) -> bytes:
    try:
        _, expected, max_bytes = PINNED_FILES[relative]
    except KeyError as exc:
        raise ValueError(f"unregistered source file {relative}") from exc
    path = upstream / relative
    if not path.is_file():
        raise ValueError(f"required source file is missing: {path}")
    if path.stat().st_size > max_bytes:
        raise ValueError(f"source file exceeds the {max_bytes}-byte limit: {relative}")
    data = path.read_bytes()
    if sha256(data) != expected:
        raise ValueError(f"SHA-256 mismatch for {relative}")
    return data


def verify_pinned_revision(upstream: Path) -> None:
    """Require the original packaging commit and its exact file blobs, not just a later HEAD."""
    exists = subprocess.run(
        ["git", "-C", str(upstream), "cat-file", "-e", f"{SOURCE_COMMIT}^{{commit}}"],
        capture_output=True,
    )
    if exists.returncode != 0:
        raise ValueError(f"source checkout does not contain pinned commit {SOURCE_COMMIT}")
    for relative, (_, expected, max_bytes) in PINNED_FILES.items():
        shown = subprocess.run(
            ["git", "-C", str(upstream), "show", f"{SOURCE_COMMIT}:{relative}"],
            capture_output=True,
            check=False,
        )
        if shown.returncode != 0 or len(shown.stdout) > max_bytes or sha256(shown.stdout) != expected:
            raise ValueError(f"pinned commit does not contain the expected bytes for {relative}")
    for relative, (expected, max_bytes) in PINNED_COMMIT_ONLY.items():
        shown = subprocess.run(
            ["git", "-C", str(upstream), "show", f"{SOURCE_COMMIT}:{relative}"],
            capture_output=True,
            check=False,
        )
        if shown.returncode != 0 or len(shown.stdout) > max_bytes or sha256(shown.stdout) != expected:
            raise ValueError(f"pinned commit does not contain the expected provenance bytes for {relative}")


def decimal_field(text: str, field: str, row_no: int) -> Decimal:
    try:
        value = Decimal(text)
    except (InvalidOperation, TypeError) as exc:
        raise ValueError(f"measurement row {row_no}: invalid {field}") from exc
    if not value.is_finite() or value <= 0:
        raise ValueError(f"measurement row {row_no}: {field} must be finite and positive")
    return value


def parse_measurements(data: bytes) -> list[dict]:
    """Read JADE's processed table; times are already seconds after shutdown."""
    try:
        text = data.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise ValueError("measurement table must be UTF-8") from exc
    reader = csv.DictReader(text.splitlines())
    if reader.fieldnames != ["time", "Value", "Error"]:
        raise ValueError("measurement header must be exactly time,Value,Error")
    rows: list[dict] = []
    previous = Decimal(0)
    for row_no, row in enumerate(reader, start=2):
        time_s = decimal_field(row["time"], "time", row_no)
        value = decimal_field(row["Value"], "Value", row_no)
        error_relative = decimal_field(row["Error"], "Error", row_no)
        if time_s <= previous:
            raise ValueError(f"measurement row {row_no}: cooling times must increase")
        previous = time_s
        # The JADE data definition says Error is an absolute reported error divided by Value.
        # Keep its magnitude in absolute response units without assigning confidence or covariance.
        rows.append({
            "cooling_time_s": float(time_s),
            "reported_heat_microW_per_g": float(value),
            "reported_error_relative": float(error_relative),
            "reported_error_microW_per_g": float(value * error_relative),
        })
    if len(rows) != 20:
        raise ValueError(f"expected all 20 FNS-96 Fe measurements, found {len(rows)}")
    return rows


def duration_seconds(text: str) -> Decimal:
    parts = text.split()
    if len(parts) != 2 or parts[1] != "s":
        raise ValueError(f"unsupported schedule duration {text!r}; expected '<seconds> s'")
    try:
        value = Decimal(parts[0])
    except InvalidOperation as exc:
        raise ValueError(f"invalid schedule duration {text!r}") from exc
    if not value.is_finite() or value <= 0:
        raise ValueError(f"schedule duration must be finite and positive: {text!r}")
    return value


def validate_problem(spec: dict, measurements: list[dict]) -> None:
    """Fail closed if the pinned ACTINV problem stops matching this experiment."""
    if spec.get("spec") != "actinv-spec-1" or spec.get("projectile") != "neutron":
        raise ValueError("expected an ACTINV neutron activation specification")
    if spec.get("library") != {"path": "catalog:tendl-2025-neutron-709g"}:
        raise ValueError("unexpected activation library; preserve the pinned TENDL-2025 709-group input")
    if spec.get("decay") != {"primary": "catalog:endfb-viii-0-decay", "fallback": "catalog:jeff-3-3-decay"}:
        raise ValueError("unexpected decay data references")
    if spec.get("material") != {"mass_g": 1.0, "basis": "wt_percent", "composition": {"FE": 100.0}}:
        raise ValueError("input is not the one-gram elemental iron sample")
    spectrum = spec.get("spectrum")
    if not isinstance(spectrum, dict) or spectrum.get("structure") != "fispact-709" or spectrum.get("descending") is not True:
        raise ValueError("expected the descending FISPACT 709-group spectrum")
    groups = spectrum.get("flux_per_group")
    if not isinstance(groups, list) or len(groups) != 709:
        raise ValueError("expected exactly 709 group fluxes")
    if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0 for v in groups):
        raise ValueError("group fluxes must be finite and non-negative")
    total = spectrum.get("total")
    if not isinstance(total, (int, float)) or not math.isclose(total, TOTAL_FLUX, rel_tol=0.0, abs_tol=0.0):
        raise ValueError("source-deck total flux must be 1.116e10 n/cm2/s")
    if math.fsum(groups) <= 0.0:
        raise ValueError("group flux spectrum has no positive shape to normalize")

    schedule = spec.get("schedule")
    if not isinstance(schedule, list) or len(schedule) != 21:
        raise ValueError("expected 300-second irradiation plus all 20 cooling increments")
    if duration_seconds(schedule[0].get("dt", "")) != IRRADIATION_SECONDS or schedule[0].get("flux") != 1.0:
        raise ValueError("first ACTINV step must be 300 s at full prescribed flux")
    previous = Decimal(0)
    for row, measurement in zip(schedule[1:], measurements, strict=True):
        if row.get("flux") != 0.0:
            raise ValueError("cooling steps must have zero flux")
        endpoint = Decimal(str(measurement["cooling_time_s"]))
        expected_increment = endpoint - previous
        if duration_seconds(row.get("dt", "")) != expected_increment:
            raise ValueError("ACTINV cooling increments do not end at every published measurement time")
        previous = endpoint


def build(upstream: Path) -> dict:
    upstream = upstream.expanduser().resolve()
    verify_pinned_revision(upstream)
    source_bytes = {rel: read_pinned(upstream, rel) for rel in PINNED_FILES}
    problem = json.loads(source_bytes[SPEC_REL])
    measurements = parse_measurements(source_bytes[CSV_REL])
    validate_problem(problem, measurements)
    files = [
        {"role": role, "path_in_source": rel, "sha256": PINNED_FILES[rel][1], "bytes": len(source_bytes[rel])}
        for rel, role in ((SPEC_REL, "ACTINV 1.0.0 problem specification"),
                          (CSV_REL, "processed FNS-96 Fe decay-heat measurement table"),
                          (METADATA_REL, "ACTINV benchmark metadata"))
    ]
    detectors = []
    for index, row in enumerate(measurements, start=1):
        detectors.append({
            "id": f"cooling-{index:02d}",
            "response_class": "decay_heat",
            "controlling_variable": {"name": "cooling time after shutdown", "value": row["cooling_time_s"], "unit": "s"},
            "reference": {"value": row["reported_heat_microW_per_g"], "u": row["reported_error_microW_per_g"]},
            "reported_error_relative": row["reported_error_relative"],
        })
    manifest = {
        "schema": SCHEMA,
        "case_id": CASE_ID,
        "title": "FNS Fe 1996 five-minute irradiation decay heat",
        "evidence_class": "experiment",
        "source": {
            "url": "https://github.com/AvilaLabs/open-benchmarks",
            "commit": SOURCE_COMMIT,
            "retrieved": "2026-11-09",
            "benchmark": "JAEA Fusion Neutronics Source 1996 iron sample, five-minute irradiation, distributed by IAEA CoNDERC",
            "original_archive_url": ARCHIVE_URL,
            "original_archive_sha256": ARCHIVE_SHA256,
            "original_archive_members": ARCHIVE_MEMBERS,
            "processing_note": {"path": SOURCE_NOTE_REL, "sha256": PINNED_COMMIT_ONLY[SOURCE_NOTE_REL][0],
                                "bytes": PINNED_COMMIT_ONLY[SOURCE_NOTE_REL][1]},
            "license_file": {"path": LICENSE_REL, "sha256": PINNED_COMMIT_ONLY[LICENSE_REL][0],
                             "bytes": PINNED_COMMIT_ONLY[LICENSE_REL][1]},
            "benchmark_definition": "https://jade-a-nuclear-data-libraries-vv-tool.readthedocs.io/en/latest/benchmarks/experimental.html#fns-decay-heat-iron-five-minute-irradiation",
            "source_deck": "TENDL-2017_1996exp_5min.i: one gram Fe, 300 s, total flux 1.116e10 n/cm2/s; exact archived member hash listed above",
        },
        "license": {
            "spdx": "CC-BY-4.0",
            "attribution": "JAEA FNS data distributed by IAEA CoNDERC; JADE processed inputs and measurement table from IAEA-NDS/open-benchmarks, repackaged at AvilaLabs/open-benchmarks commit 1914ed418b597398d758e7c6fa11e83158e2ca21. Attribution and redistribution terms follow the source repository's CC BY 4.0 license.",
            "terms": "https://creativecommons.org/licenses/by/4.0/",
        },
        "files": files,
        "files_are_external_not_vendored": True,
        "normalisation": {
            "status": "confirmed",
            "quantity": "decay heat per gram of sample at each cooling time after shutdown",
            "units": "microW/g; ACTINV heat_W_per_g.total is converted to microW/g by multiplying by exactly 1e6",
            "source_normalisation": "one gram elemental Fe; pinned FISPACT-709 group shape scaled by ACTINV to the source-deck total 1.116e10 n/cm2/s; 300 s irradiation",
            "location": "non-spatial homogeneous inventory calculation; no geometry or density scaling",
            "reaction_or_particle": "neutron activation followed by alpha, beta and gamma decay heat; response is their ACTINV total",
            "energy_integration": "energy-integrated total decay heat",
            "basis": "Original CoNDERC source archive/member SHA-256 values are recorded above. The pinned JADE problem reproduces its one-gram Fe, 300-second, 1.116e10 n/cm2/s irradiation and 709-group spectrum. Its raw group vector sum differs slightly from the declared total; ACTINV documentation states the declared total scales the vector while preserving shape, so the adapter retains both source inputs unchanged and relies on that documented behavior. JADE's benchmark documentation defines the processed table's Value as microW/g, time as seconds after shutdown, and Error as reported absolute error divided by measured heat. The input schedule is checked against all 20 endpoints.",
        },
        "uncertainty": {
            "components": [
                {"name": "reported experimental error magnitude", "kind": "reported", "meaning": "CSV Error is relative; manifest u is Value*Error in microW/g. Its confidence level and covariance across points are not stated."},
                {"name": "ACTINV sampling uncertainty", "kind": "none", "meaning": "ACTINV is deterministic for this run and reports no sampling uncertainty; this does not quantify model or nuclear-data uncertainty."},
            ],
            "note": "C/E is descriptive. The reported error magnitude is preserved without assigning a confidence level. Covariance is unknown, so the shared scorer must keep every row INCONCLUSIVE; no experimental PASS/FAIL or total prediction uncertainty is claimed.",
        },
        "compatibility": {
            "declared": "2026-11-09",
            "k": 1.0,
            "k_basis": "authored descriptive scale only; an acceptance interval is not asserted because reported-error confidence and calculation uncertainty are unavailable",
            "covariance": {"kind": "unknown", "basis": "no covariance between deterministic ACTINV prediction and reported experiment is published; model/nuclear-data uncertainty is not quantified"},
        },
        "response_classes": {
            "decay_heat": {"unit": "microW/g", "description": "total decay heat per sample gram at each measured cooling time after shutdown"},
        },
        "detectors": detectors,
        "not_covered": [
            "transport reconstruction of the FNS irradiation field; the benchmark uses its prescribed 709-group spectrum",
            "predictive/model/nuclear-data uncertainty and experimental covariance; only the reported error magnitude is carried",
            "a blind trial: this same FNS-96 Fe case has already been used in ACTINV validation work",
            "any material, spectrum, irradiation duration or cooling time outside this one-gram Fe, 300-second case and its 20 measured endpoints",
            "decay heat per component, volume, dose, whole-device shutdown dose, or plant qualification",
        ],
        "qualified_range": {
            "materials": ["elemental Fe, 100 wt%, sample mass 1.0 g"],
            "geometry_class": "non-spatial inventory benchmark; geometry and density are not modeled",
            "spectrum_class": "prescribed FISPACT-709 group spectrum, normalized to 1.116e10 n/cm2/s",
            "parameters": [{"name": "irradiation time", "min": 300, "max": 300, "unit": "s"},
                          {"name": "cooling time", "min": measurements[0]["cooling_time_s"], "max": measurements[-1]["cooling_time_s"], "unit": "s"}],
            "cooling_time": "20 source measurements, all represented without interpolation",
        },
        "literature_context": [],
    }
    issues = verify_files(manifest, upstream)
    if issues:
        raise ValueError("source files failed manifest verification: " + "; ".join(issues))
    return check_manifest(manifest)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--upstream", type=Path, required=True, help="local open-benchmarks checkout containing the pinned source commit")
    parser.add_argument("--out", type=Path, default=HERE / "manifest.json")
    args = parser.parse_args()
    manifest = build(args.upstream)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
