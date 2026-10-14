# Releasing

This page describes the checks before a release. `scripts/make_release.py` builds the downloads from a verified
package (see its header); the release gate below decides whether a release may go ahead at all.

## The release gate (QA-070)

`scripts/release_gate.py` prints one line per condition, PASS or FAIL with the reason, and exits 1 unless every
condition passes. It uses only the Python standard library.

| Condition | Where it comes from |
| --- | --- |
| Every ID in the "Release gate" list of `docs/NEXT_RELEASE_PLAN.md` is Met | read from the requirement tables; the list is parsed from the plan |
| The requirement-to-test trace is complete for those IDs, and no marker names an unknown requirement (QA-002) | `Verifies:` markers in the source tree |
| The V&V matrix has no blank cell, and a quantity with no benchmark is labelled "not validated" with a reason (VAL-051) | `validation/matrix.json` |
| The V&V report exists and its hash matches the release record (VAL-091) | `--vv-report`, `--release-record` |
| Wording lint is clean | `scripts/wording_lint.py` |
| All tests pass; performance gates pass (PERF-051); fuzz clean (QA-033); documentation tests pass (QA-016) | evidence files from the jobs that ran them |
| Reproducible build (QA-063) | evidence file with two builds' hashes per artifact |
| 0 open S1 defects and 0 open S2 defects without a waiver (QA-083) | the defect log |

A condition the script cannot decide from the repository takes a file as input and FAILS when the file is absent,
empty, not valid JSON or for another commit. Nothing passes by default. The formats are in the script's header.

At release time:

```sh
python3 -m validation report --out vv --release-record release-record.json
python3 scripts/release_gate.py --vv-report vv --release-record release-record.json \
    --tests tests.json --perf perf.json --defects defects.json --fuzz fuzz.json \
    --repro repro.json --doc-tests doc-tests.json --archive gate-output.txt
```

Archive `gate-output.txt`, `release-record.json` and the `vv` directory with the release record. The gate is not a
CI step on ordinary commits: it fails until the gate requirements are Met, which is the intended result. It is
wired in as a step of the release job, after the test, performance, fuzz and build jobs have written their
evidence files.

An open S1 defect is never waived (QA-083). An open S2 defect needs a waiver with owner, reason and an expiry
date within 90 days (QA-072).

## The V&V matrix and report

The matrix has one row per calculated quantity and benchmark. A quantity with no benchmark has one row with
benchmark "none", status "not validated", and the reason and next step. It is generated, not edited:

```sh
python3 -m validation matrix            # rewrites validation/matrix.json
```

`validation/quantities.json` lists the calculated quantities and which registered benchmark responses cover each.
Add a benchmark there only when a case in `validation/cases` really measures the quantity. A test fails when the
committed `matrix.json` differs from what the registry and manifests generate.

`python3 -m validation report --out DIR --release-record FILE` writes `report.json`, `report.md` and `report.sha256`,
and records the report's SHA-256 in the release record. The report holds the methods, the matrix, the C/E
distributions per case and response class, the qualified ranges, the known issues and the date of the last run of
each benchmark (`run_date` in the run record). List every row labelled "not validated" in the release notes
(VAL-051).
