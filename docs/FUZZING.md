# Fuzzing the parsers

Requirement QA-033: every parser that reads bytes or text from outside FARIS has a fuzz target, each
target runs for at least one CPU-hour per release, and a release candidate has no open crash, hang or
out-of-memory finding. This page says what is covered, how to run it and what the release gate reads.

## What is covered

Every target is a function of bytes in `fuzz/entries/src/lib.rs`. The libFuzzer targets in
`fuzz/fuzz_targets/` each call one of them, so the two cannot drift apart; a test checks that every
entry has a target file.

| Target | Reads | Entry points |
| --- | --- | --- |
| `scenario` | scenario files | `faris_model::LoadedScenario::from_bytes`, `validate_identity` |
| `design_file` | design files (`*.faris-design.json`) and the checks run on what they parse to | `design::parse_design`, `check_2_static`, `check_4`, `check_5`, `void_solids`, `check_waivers`, `waiver_references` |
| `history_assumptions` | operating histories, driving rates, maintenance assumptions, designs files | `design::scenario_facts`, `OperatingHistoryAssumptions` and `MaintenanceAssumptions` (parse and `validate`), `TransportDrivingRates`, `maintenance::files::parse_designs` |
| `facets` | the facets binary and its manifest (geometry import) | `design::parse_facets`, `design::load_facets` |
| `step_inspection` | the STEP helper's inspection report (STEP header facts) | `design::StepInspection::from_bytes` |
| `transport_artifact` | raw transport artifacts, transport requests | `transport::TransportArtifact::from_bytes`, `TransportRequest` parse and `validate_against` |
| `json_records` | recorded results read back: history results and ensembles, maintenance results, reactor runs, physics cases, normalized and raw transport results, recorded bundles, Core evidence runs, job results, import and material-audit reports, CAD worker responses | `serde_json` readers of those types |
| `helper_outputs` | what the design import reads back from its helpers and cache (worker result, window and piece records, cache key, progress lines) | `design::fuzz_helper_outputs` (hidden, for this target) |
| `nuclear_data_audit` | the nuclear-data library audit and the cross-section file it names | `reactor::bind_audited_library` |
| `actinv_result` | ACTINV result streams | `maintenance::actinv::scan_result` |
| `file_readers` | the readers that take a path: ACTINV points, worker point files, physics cases, reactor runs, recorded bundles, study descriptors | `actinv::read_points`, `protocol::read_points`, `reactor::load_physics_case`, `load_reactor_run`, `core_evidence::load_recorded_bundle`, `scenario_from_bundle_file`, `faris_study::descriptor_evidence`, `evidence_from_descriptor` |
| `study_file` | `.faris` study files | `StudyReader::open`, `preview_status`, `preview`, `blob_infos`, `ensembles`, `verify`, `materialize` |
| `tar_gz` | evidence archives and recorded-tree manifests | `faris_study::extract_tar_gz`, `extract_recorded_tree` |
| `evidence_store` | the evidence store index (`store.json`) | `EvidenceStore::open`, `verify`, `read_file`, `verify_store` |
| `saved_case` | a saved Core case: its files and execution report | `case_archive::inspect_saved_case` |
| `png_header` | the preview picture's header | `faris_study::png_dimensions` |
| `proc_and_config_text` | `/proc/meminfo`, `/proc/<pid>/stat` and `status`, the CAD-reader setting, the mesh-preset name, ids | `memory::parse_meminfo_available`, `memory::sampler::parse_stat_group`, `parse_status_kib`, `CadReader::from_label`, `MeshPreset::from_cli`, `design::valid_id`, `is_reserved`, `suggest_id` |

Not covered by a cargo-fuzz target, and why:

- Python readers: the STEP reader and tessellation helper (`integrations/cad/`), the OpenMC statepoint,
  HDF5 and `.npz` readers and library audit (`integrations/openmc/`), the validation harness
  (`validation/`) and the release scripts (`scripts/`). cargo-fuzz drives Rust code only. Rust sees
  OpenMC's results only as the JSON the Python side writes (`json_records`, `helper_outputs`).
- There is no CSV reader in the Rust crates; CSV is written (exports), never read.
- The command-line binary's own argument parsing (`clap`; `faris-cli::design::parse_unit` is a match on a
  fixed list) and its use of the readers above: the readers are the code that sees file bytes.
- `faris-app` and `faris-occt` are not built here. The native CAD reader parses STEP inside OpenCASCADE;
  its checks are in `crates/faris-occt/`.
- `faris-occt-client::client` reads the last lines of the worker's stderr into an error message (bounded
  to 1 MiB, no parsing).

## Running

The fuzz crate is outside the workspace (`fuzz/`), so normal builds never see it. It needs the nightly
toolchain and `cargo install cargo-fuzz`.

```bash
# every target for 30 seconds each (about 9 minutes), evidence in fuzz/evidence.json
scripts/fuzz_all.sh 30

# the release run: one CPU-hour per target
scripts/fuzz_all.sh 3600 fuzz/evidence.json

# one target by hand
RUSTUP_TOOLCHAIN=nightly cargo fuzz run facets --fuzz-dir fuzz -- -rss_limit_mb=2048 -timeout=10
```

`scripts/fuzz_all.sh` makes the seed corpus first (`scripts/fuzz_seed.py`: the repository's own design,
scenario, history, physics-case, facets, ACTINV and PNG files plus a real study file and a few small
archives; it is generated into `fuzz/corpus/`, not committed), then runs each target under libFuzzer with
`-rss_limit_mb=2048 -timeout=10 -jobs=1` inside a 3 GB memory cgroup on two CPUs, and writes the
evidence file. A run over an hour per target is 16 hours or more; ask first.

A finding is written to `fuzz/artifacts/<target>/`. Minimise it (`cargo fuzz tmin`), add the input as an
ordinary `#[test]` in the crate that owns the parser, and fix the parser so it returns an error. The
regression test then runs in the stable CI job without the fuzzer.

### Where the fuzzer cannot be built

libFuzzer needs the `libfuzzer-sys` crate. On a machine without it (an offline checkout),
`DRIVER=smoke scripts/fuzz_all.sh 60` runs `fuzz/entries/src/bin/smoke.rs` instead: a blind mutation
driver (no coverage feedback) that watches for panics, hangs over 10 s and heap growth over 2 GiB. It
is a smoke check only; its evidence says `"driver": "smoke"` and the release gate refuses it.

## Stable CI

The push checks run `cargo test --manifest-path fuzz/entries/Cargo.toml` on the seed corpus and a set of
plain inputs, so a parser change that makes an entry panic fails there. The libFuzzer build (sanitizer
build of the engine, then 30 seconds per target) is `.github/workflows/fuzz.yml`: weekly, on demand, and
on changes under `fuzz/`. It is not a push check because it takes more than the five minutes a push check
may use.

## Release evidence

`scripts/release_gate.py --fuzz FILE` reads the file `scripts/fuzz_all.sh` writes:

```json
{
  "schema_version": "faris-fuzz-evidence/v0.1",
  "commit": "<sha>", "dirty": false, "driver": "libfuzzer", "toolchain": "rustc 1.99.0-nightly ...",
  "seconds_per_target_requested": 3600, "finished": "2026-10-20T04:00:00Z",
  "checks": [
    {"name": "facets", "verdict": "PASS", "seconds": 3600, "executions": 41000000, "crashes": 0, "exit_status": 0}
  ]
}
```

The gate passes only when the commit is the one being released, the tree was clean, the driver is
`libfuzzer`, and every file in `fuzz/fuzz_targets` has a check that is PASS with `crashes` 0 and
`seconds` of at least 3600.
