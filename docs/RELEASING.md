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
| All tests pass; performance gates pass (PERF-051); fuzz clean (QA-033); documentation tests pass (QA-016) | evidence files from the jobs that ran them (fuzz: `scripts/fuzz_all.sh 3600`, see `docs/FUZZING.md`) |
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

## Documentation samples (QA-016)

`scripts/doc_samples.py` runs the handbook's code samples and writes the `--doc-tests` evidence file. CI runs it in
the `check` job after the debug `faris` binary is built (about 1.5 minutes):

```sh
cargo build -p faris-cli
python3 scripts/doc_samples.py --faris target/debug/faris --cargo cargo --evidence doc-tests.json
```

It reads the fenced blocks of `README.md`, `docs/*.md` and `docs/guide/*.md`, the `faris ...` commands written in
backticks in the text, and the rustdoc comments of the crates. Each gets one disposition, and a block with none
fails, so a new sample cannot be added unchecked:

- **Executed.** A `faris` command is run with the debug binary on fixtures in a temporary folder and must exit 0.
  The fixtures are synthetic (transport bundles with placeholder members, and the recorded transport rates of the
  engine's determinism test), so a command runs but says nothing about physics. The commands of one block share a
  folder, in order. The debug build takes half a minute for one history, so `--samples` of an ensemble is reduced
  to 4 when run, and identical ensemble commands are run once.
- **Checked, not run.** A command that needs OpenMC, ACTINV, Avila Core, a CAD Python, a transport run or a path you
  fill in. Its subcommand path and every `--flag` are looked up in the binary's own help; a Python script it names
  must exist and show each flag in its `--help` (or in its source, when it needs the reader's OpenMC Python); a
  repository path must exist. The reasons are in `NEEDS` in the script.
- **JSON examples** of file formats are parsed by the real parsers in `crates/faris-engine/tests/doc_json_samples.rs`
  (the design-file waivers, the maintenance designs and assumptions). Python snippets are parsed with `ast`.
  Diagrams, formulas and outline fragments are checked for their field names where they name fields.
- **Skipped**, with a reason in `SKIP_PROGRAMS`: the desktop app, `tar` and `sha256sum` on a downloaded release,
  `curl`, `cargo` build and test commands (CI runs them as earlier steps), and setting the path of your OpenMC.

An expected failure needs a written reason in `EXPECTED_FAIL`; there are none. The handbook has no step-by-step
tutorial that runs without a display or OpenMC: the tour is in the desktop app, and the demo walkthrough
(`docs/DEMO_WALKTHROUGH.md`) needs OpenMC, so its commands are checked, not run. The library crates have no rustdoc
examples; `cargo test --doc` runs for them and the script records how many passed.

## Reproducible build (QA-063)

The Linux `faris` command-line binary is reproducible: two builds of one commit, from checkouts at different paths
and into different target directories, hash identically. The recipe, applied by `scripts/repro_check.sh`:

```sh
eval "$(scripts/repro_check.sh --env --target "$CARGO_TARGET_DIR")"   # RUSTFLAGS, SOURCE_DATE_EPOCH, TZ, LC_ALL
cargo build --release --locked -p faris-cli
```

`RUSTFLAGS` remaps the checkout, the cargo home, the rustup home and the target directory to fixed names in every
path rustc embeds. `SOURCE_DATE_EPOCH` is the commit time. The release profile (`codegen-units = 1`, thin LTO)
needs no change. To check a commit:

```sh
scripts/repro_check.sh --work /scratch/repro --evidence repro.json
```

This builds from the current checkout and from a fresh `git worktree` of the same commit, into two target
directories, and writes the `--repro` evidence of the release gate. It needs a clean tree and takes two
release builds (3.7 minutes each on a laptop with three jobs; expect 10 to 15 minutes on a hosted runner). The `Reproducible build` workflow runs it
on version tags and by hand, and uploads `repro.json`. If the hashes differ, compare the two binaries with
`diffoscope`, or with `cmp -l` and `strings`, and look for a path, a timestamp or a hash of a path.

Not covered: `faris-app` (graphics libraries and system packages that differ between builders), the macOS and
Windows builds, and signed archives, which embed signing times. They are built on a best-effort basis and are not
claimed to be reproducible.

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
