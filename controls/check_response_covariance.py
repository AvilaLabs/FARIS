#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Independent exact-arithmetic control for the transport response covariance.

Uses only Python's standard library (fractions), without importing FARIS or
OpenMC. Recomputes the covariance of the batch means from
transport-batch-values.json, compares it with the matrix recorded in
transport-artifact.json, and, when run.json holds a normalized record, checks
the integrated matrix against the same per-response factors used for
integrated means. A PASS applies to arithmetic on supplied values: it does not
show that the batch values came from the solver, and it covers Monte Carlo
sampling between responses only, never volume, nuclear-data or model error.
"""
import argparse
from fractions import Fraction
import hashlib
import json
from pathlib import Path

METHOD = "batch-means-sample-covariance/v1"
TOLERANCE = Fraction(1, 10**9)
ELEMENTARY_CHARGE = Fraction("1.602176634e-19")
UNIT_FACTORS = {"cm_per_source": Fraction(1, 100), "particles_per_source": Fraction(1),
                "events_per_source": Fraction(1), "ev_per_source": ELEMENTARY_CHARGE}


def load(path):
    with Path(path).open() as stream:
        return json.load(stream, parse_float=Fraction)


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            value.update(block)
    return value.hexdigest()


def batch_mean_covariance(columns):
    """Exact covariance of batch means: sample covariance (ddof=1) / n."""
    n = len(columns[0])
    if n < 2 or any(len(c) != n for c in columns):
        raise ValueError("covariance needs equal columns of at least 2 batches")
    means = [sum(c, Fraction(0)) / n for c in columns]
    size = len(columns)
    return [[sum((columns[i][b] - means[i]) * (columns[j][b] - means[j]) for b in range(n))
             / (n - 1) / n for j in range(size)] for i in range(size)]


def max_scaled_difference(actual, expected, size):
    """Largest |actual - expected| relative to sqrt(expected_ii * expected_jj).

    Compared squared, so no square root is needed; a zero-variance pair must
    match exactly. Raises ValueError above TOLERANCE.
    """
    worst = Fraction(0)
    for i in range(size):
        for j in range(size):
            diff = Fraction(actual[i * size + j]) - expected[i][j]
            scale_sq = expected[i][i] * expected[j][j]
            if diff == 0:
                continue
            if scale_sq == 0 or diff * diff > TOLERANCE * TOLERANCE * scale_sq:
                raise ValueError(f"covariance entry ({i},{j}) differs: {float(diff)} against scale "
                                 f"{float(scale_sq) ** 0.5}")
            worst = max(worst, diff * diff / scale_sq)
    return worst


def check_raw(artifact, batch_values, batch_file_sha256):
    """Recompute the raw matrix from the batch values and compare."""
    record = artifact.get("response_covariance")
    if record is None:
        raise ValueError("artifact has no response_covariance")
    if record["method"] != METHOD or batch_values["method"] != METHOD:
        raise ValueError("unsupported covariance method")
    if record["batch_values_sha256"] != batch_file_sha256:
        raise ValueError("batch-values file differs from its recorded sha256")
    ids = record["response_ids"]
    if ids != batch_values["response_ids"]:
        raise ValueError("response ids differ between artifact and batch values")
    n_batches = record["batches"]
    if n_batches != batch_values["n_batches"]:
        raise ValueError("batch count differs between artifact and batch values")
    columns = [batch_values["values"][i] for i in ids]
    if any(len(c) != n_batches for c in columns):
        raise ValueError("a response does not have one value per batch")
    expected = batch_mean_covariance(columns)
    size = len(ids)
    if len(record["raw_per_source"]) != size * size:
        raise ValueError("raw matrix is not n * n")
    worst = max_scaled_difference(record["raw_per_source"], expected, size)
    # The solver's own mean and standard error must be what the batches imply.
    tallies = {t["response_id"]: t for t in artifact["tallies"]}
    for index, response_id in enumerate(ids):
        tally = tallies[response_id]
        mean = sum(columns[index], Fraction(0)) / n_batches
        reported = Fraction(tally["mean"])
        if abs(mean - reported) > TOLERANCE * (abs(reported) if reported else max(map(abs, columns[index]))):
            raise ValueError(f"{response_id}: batch mean differs from the tally mean")
        reported_se = Fraction(tally["standard_error"])
        if reported != 0 and abs(expected[index][index] - reported_se ** 2) > TOLERANCE * reported_se ** 2:
            raise ValueError(f"{response_id}: batch variance differs from the tally standard error")
    return ids, expected, float(worst) ** 0.5


def source_neutron_rate(request):
    return (Fraction(request["fusion_power_mw"]) * 1000000
            / (Fraction(request["source"]["energy_per_reaction_ev"]) * ELEMENTARY_CHARGE)
            * Fraction(request["source"]["neutrons_per_reaction"]))


def check_integrated(artifact, request, normalized, ids, raw_expected):
    """Check the normalized integrated matrix against raw * s_i * s_j."""
    record = normalized.get("response_covariance")
    if record is None:
        raise ValueError("normalized record has no response_covariance")
    if record["response_ids"] != ids:
        raise ValueError("normalized response ids differ from the artifact")
    rate = source_neutron_rate(request)
    tallies = {t["response_id"]: t for t in artifact["tallies"]}
    scales = [rate * UNIT_FACTORS[tallies[i]["unit"]] for i in ids]
    size = len(ids)
    expected = [[raw_expected[i][j] * scales[i] * scales[j] for j in range(size)] for i in range(size)]
    worst = max_scaled_difference(record["integrated"], expected, size)
    results = {r["response_id"]: r for r in normalized["results"]}
    for index, response_id in enumerate(ids):
        want = Fraction(results[response_id]["integrated_standard_error"]) ** 2
        if abs(expected[index][index] - want) > Fraction(1, 10**6) * want:
            raise ValueError(f"{response_id}: integrated variance differs from integrated standard error")
    return float(worst) ** 0.5


def correlation(expected):
    size = len(expected)
    out = []
    for i in range(size):
        row = []
        for j in range(size):
            scale = (float(expected[i][i]) * float(expected[j][j])) ** 0.5
            row.append(float(expected[i][j]) / scale if scale else None)
        out.append(row)
    return out


def check(directory):
    directory = Path(directory)
    solver = directory / "solver"
    artifact = load(solver / "transport-artifact.json")
    record = artifact["response_covariance"]
    batch_path = solver / record["batch_values_file"]
    batch_values = load(batch_path)
    ids, expected, raw_difference = check_raw(artifact, batch_values, digest(batch_path))
    report = {
        "run_directory": str(directory), "batches": record["batches"], "response_ids": ids,
        "max_scaled_raw_difference": raw_difference,
        "correlation": correlation(expected),
        "integrated_checked": False,
    }
    run_path = directory / "run.json"
    if run_path.exists():
        normalized = load(run_path).get("normalized")
        if normalized is not None:
            request = load(directory / "input.json")["request"]
            report["max_scaled_integrated_difference"] = check_integrated(
                artifact, request, normalized, ids, expected)
            report["integrated_checked"] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, action="append", required=True,
                        help="Run directory with solver/, input.json and optional run.json; repeat")
    parser.add_argument("--output", type=Path, help="New output file; otherwise stdout")
    args = parser.parse_args()
    report = {
        "schema_version": "faris-independent-response-covariance/v0.1",
        "covariance_control": "PASS", "scientific_qualification": "NOT_EVALUATED",
        "scope": "Exact Fraction recomputation of the batch-means covariance of scalar responses and its "
                 "unit scaling; relative tolerance 1e-9 against sqrt(C_ii C_jj). Monte Carlo sampling "
                 "between responses of one run only; excludes volume, nuclear-data and model uncertainty.",
        "runs": [check(directory) for directory in args.run],
    }
    encoded = json.dumps(report, indent=2) + "\n"
    if args.output:
        with args.output.open("x") as stream:
            stream.write(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
