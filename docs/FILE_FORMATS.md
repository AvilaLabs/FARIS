<!-- kind: reference -->
# File formats

Every file FARIS reads or writes names its format in a `schema` or `schema_version` field, such as
`faris-scenario/v0.1`. The text before the slash is the format, the text after it is the version. An unknown major
version is refused; unknown fields in a known version are ignored. This page lists the formats the code knows by
that name, where each is described, and which are not described yet. It is checked against the source by hand at
each release; there is no JSON Schema file for any format yet, so the Rust types that read the file are the schema.

Last checked: 2026-10-14.

## Described formats

| Format | What it is | Read and written by | Described in | Example |
| --- | --- | --- | --- | --- |
| `.faris` container (`faris-saved-study-store/v0.1`) | A saved study: manifest and content-addressed blobs | `faris-study`, `faris-cli study-file` | [Study file](STUDY_FILE.md), [handbook](guide/study-files.md) | `faris study-file verify` on any saved study |
| `faris-scenario/v0.1` | The plant: geometry, materials, solver inputs | `faris-model` | [Architecture](ARCHITECTURE.md), [geometry notes](notes/GEOMETRY_0.2_DESIGN.md) | `scenarios/arc-inspired/` |
| `faris-design/v0.1` | A design file (bring your own CAD) | `faris-model` | [Design file](guide/design-file.md), [format notes](notes/DESIGN_FILE_FORMAT.md) | In the handbook page; parsed by `crates/faris-engine/tests/doc_json_samples.rs` |
| `faris-operating-history/v0.1` | Operating schedule and assumptions | `faris-model` | [Operating history](OPERATING_HISTORY.md), [design file](guide/design-file.md) | `scenarios/arc-inspired/` |
| `faris-history-result/v0.1`, `faris-history-ensemble/v1` | A calculated history and an uncertainty ensemble | `faris-engine` | [Operating history](OPERATING_HISTORY.md), [Study file](STUDY_FILE.md) | Recomputed from a recorded study |
| `faris-transport-request/v0.1`, `v0.2`; `faris-transport-artifact/v0.1`, `v0.2` | A transport request and the normalised result | `faris-model` | [Architecture](ARCHITECTURE.md), [Cold reference](COLD_REFERENCE.md), [Transport](TRANSPORT.md) | The recorded bundles in a release package |
| `faris-openmc-input/v0.1` | The OpenMC input FARIS writes | `faris-engine` | [Cold reference](COLD_REFERENCE.md) | In the recorded package |
| `faris-maintenance-designs/v0.1`, `faris-maintenance-assumptions/v0.1` | Inputs of the computed maintenance durations | `faris-app`, `faris-cli` | [Maintenance](guide/maintenance.md), [design notes](notes/MAINTENANCE_0.2_DESIGN.md) | `maintenance/` in a release package |
| `faris-recorded-demo-package/v0.7` (and `v0.5`) | The package index of a release | `faris-app`, `scripts/verify_recorded_demo.py` | [Package notes](notes/PACKAGE_0.1.1_DESIGN.md), [evidence store notes](notes/EVIDENCE_STORE_DESIGN.md) | `package-index.json` in a release |
| `faris-recorded-tree-archive/v0.1` | A tar archive of one saved Core case | `faris-study` | [Package notes](notes/PACKAGE_0.1.1_DESIGN.md) | `port/archives` in an older package |
| `faris-facets/v1`, `faris-step-inspect/v1` and `v2` | CAD facets and STEP inspection output | `faris-occt-client`, `integrations/cad` | [CAD integrity checks](notes/CAD_INTEGRITY_CHECKS.md), [OCCT layer](notes/OCCT_LAYER.md) | `integrations/cad/fixtures` |
| CSV, SVG, PNG and PDF exports | Tables, charts and the brief | `faris-report` | [Study export](STUDY_EXPORT.md), [handbook](guide/export.md) | An exported brief |

## Not yet described

These formats have a schema name in the code and no description in `docs/`. Each is a gap against the requirement
that every format has a prose description, a schema and a versioned example (DOC-007).

- Core evidence: `faris-core-compilation/v0.1`, `faris-core-compilation-receipt/v0.1`, `faris-core-evidence-run/v0.1`,
  `faris-core-stage-output/v0.1` to `v0.3`, `faris-normalized-transport/v0.1`, `faris-generated-study/v0.1`.
- Saved cases: `faris-saved-case-inspection/v0.2`, `faris-saved-study-archive/v0.1`,
  `faris-recorded-transport-bundle/v0.1`, `faris-recorded-materialization/v0.1`.
- Control and comparison output: `faris-control-request/v0.1`, `faris-control-execution/v0.1`,
  `faris-history-comparison/v0.1`, `faris-history-comparison-provenance/v0.1`, `faris-history-sensitivity/v0.1`.
- Transport and design reports: `faris-design-transport/v3`, `faris-design-transport-request/v3`,
  `faris-design-transport-report/v0.4`, `faris-design-import-report/v0.2`, `faris-transport-import/v0.1`,
  `faris-transport-spectra/v0.1`, `faris-reactor-run/v0.1`, `faris-exploratory-precision-goals/v0.1`,
  `faris-physics-case/v0.1`, `faris-openmc-geometry-ownership-audit/v0.1`, `faris-source-sites/v1`,
  `faris-material-audit/v1`, `faris-conversion/v1`.
- Native app and browser: `faris-package-check/v0.1`, `faris-interface-check-plan/v0.1`,
  `faris-interface-check-report/v0.1`, `faris-native-frame-benchmark/v0.1`, `faris-recorded-frames/v0.1`,
  `faris-web-demo/v1`, `faris-web-ensembles/v1`, `faris-tessellation/v1`.
- Worker protocol: `faris-occt-request/v1`, `faris-occt-response/v1`.

There is no plugin manifest format yet because FARIS has no plugin system, and no receipt format of its own: receipts
are Avila Core's, and the handbook's [Evidence page](guide/evidence.md) describes what FARIS shows of them.
