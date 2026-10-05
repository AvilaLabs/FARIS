# Interoperability

What FARIS reads and writes, and how well: its own open study format, geometry and transport-model formats, field and scene formats, tables, fusion data standards, systems-engineering exchange, outputs from other design codes, and nuclear data. Every format claim is backed by a corpus test and a published matrix, because a format that opens "most of the time" is not interoperability. Units and provenance travel with every export. Timings are on the reference laptop (RL); see [the index](README.md#reference-hardware-and-models) for the reference models. Settings that choose export defaults are in 12-configurability; scripting and service access to the same operations is in 13-automation-and-extensibility.

## The .faris file and its openness

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-001 | The .faris format shall be openly specified, complete enough to write a reader without FARIS source. | The specification covers container, manifest, blob table, encodings, roles, layers, versioning and failure rules; an independent reader written from the spec alone passes the full fixture set. | Clean-room reader (Python, ≤ 300 lines) tested against ≥ 20 fixtures. | docs/STUDY_FILE.md exists; FARIS choice | F1 | Partial: docs/STUDY_FILE.md specifies container, manifest and blobs; no independent reader and no formal schema |
| INT-002 | A third party shall be able to read a .faris file with standard tools. | `unzip` plus `zstd` plus any JSON reader recover the manifest and every blob; `mimetype` is the first entry, stored uncompressed. | Shell-only extraction script in CI on 5 fixtures. | Zip container with content-addressed blobs (STUDY_FILE.md) | F1 | Partial: the container is a zip with zstd blobs as designed; no shell-only test |
| INT-003 | The manifest shall have a published JSON Schema. | Schema in the release; every fixture and every file written by FARIS validates; a diff fails CI. | Schema validation in CI. | JSON Schema 2020-12 [U] | F1 | No |
| INT-004 | Readers shall fail closed on bad files. | 100 % of truncated, hash-mismatched, wrong-type and newer-major files refused with the cause named; 0 partial loads, over a ≥ 100-case corrupt corpus. | Corrupt-corpus test. | Fail-closed house rule | F1 | Partial: reader is fail-closed with named errors; no large corrupt corpus |
| INT-005 | Save, load, save shall give a byte-identical file for the same content. | 100 % on the fixture corpus, ignoring only the writer timestamp, which is stored outside the hashed content. | Round-trip test. | Reproducibility rule | F1 | Unmeasured: blobs are content-addressed; whole-file determinism not tested |
| INT-006 | Large arrays shall be stored as typed binary arrays, with the evidence chain kept. | `array-f64-le` and `array-f32-le` implemented; every derived array names the hash of its source and the FARIS version; a text manifest stays under 1 MB for any study. | Size test; chain-verification test. | STUDY_FILE.md reserves both encodings | F2 | No: v1 writes only `verbatim`; encodings reserved |
| INT-007 | A study shall be shareable with licensed or proprietary data stripped. | Export flag removes named blobs and leaves hash references; the stripped file opens, states what is missing and reports receipts as not included. | Strip-and-open test. | STUDY_FILE.md: referenced evidence layer; R4 COL-11 | F2 | Partial: evidence layer can be referenced by hash only; no named-blob strip |
| INT-008 | The study file shall be exportable as an RO-Crate with a workflow-run profile. | RO-Crate 1.3 (1.2 if tooling lags) validates with the reference validator; replay from the crate reproduces the result hash on 5 golden studies. | Validator in CI; replay test. | RO-Crate 1.2 stable and 1.3 current [V]: [RO-Crate specification](https://www.researchobject.org/ro-crate/specification.html) | F6 | No |
| INT-009 | The study export folder shall be self-describing. | `export-manifest.json` lists every file with SHA-256 and size and the study stamp; 100 % of files in the folder listed. | Manifest completeness test. | docs/STUDY_EXPORT.md | F1 | Met: `export-manifest.json` with hashes, byte counts, version, UTC time and study stamp (docs/STUDY_EXPORT.md, crates/faris-report tests assert schema `faris-export/1`) |

## Geometry and CAD

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-010 | FARIS shall import STEP geometry (AP203, AP214, AP242). | ≥ 95 % of a ≥ 50-file public corpus imports with volume relative error ≤ 1e-6; failures are listed with a cause. | Corpus test (NIST CAD test models). | OpenCASCADE, FreeCAD [U]; NIST MBE PMI models [U] | F7 | No: demo geometry is built in code from scenario JSON |
| INT-011 | FARIS shall report whether imported geometry is ready for transport. | Checks for watertightness, overlaps, gaps and tolerance run on 100 % of imports; the report names each failing body with location; a failing model blocks a transport run. | Corpus with 20 seeded defects; detection ≥ 95 %. | Trap: STEP import does not mean transport-ready [R4] | F7 | No |
| INT-012 | FARIS shall export and import DAGMC `.h5m`. | Exported file passes the DAGMC watertightness check; volumes agree with CAD to ≤ 1e-4 relative; OpenMC runs the file. | Test with DAGMC and OpenMC tools. | DAGMC, cad_to_dagmc [U] | F7 | No |
| INT-013 | FARIS shall export the geometry as a surface or mesh file for downstream tools. | STL, OBJ and PLY export with units stated in an accompanying record; reimport within 1e-6 relative volume. | Round-trip test. | Mesh formats standard [U] | F7 | Partial: `faris export` writes a geometry metadata manifest; no mesh formats |
| INT-014 | FARIS shall keep component identities across every geometry exchange. | Component ids and names survive export and reimport for 100 % of components on 3 fixtures. | Id round-trip test. | AGENTS: one scenario definition, displayed identities | F7 | Partial: identities shared between scene and solver inside FARIS; no exchange test |
| INT-015 | Display-only edits (cutaways) shall never be exported as solver geometry. | 0 solver-geometry changes caused by a cutaway in a 20-case test. | Geometry hash before and after cutaway. | AGENTS: display cutaway cannot change solver geometry | F2 | Partial: rule held in design; no test over exports |

## Transport model and nuclear data formats

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-020 | FARIS shall export an OpenMC model (geometry, materials, settings, tallies) that runs unmodified in stock OpenMC. | Stock OpenMC reproduces the FARIS TBR within 3σ at equal seed and histories on RM-S and RM-M; 100 % of supported geometry features covered. | Cross-run test. | OpenMC XML and `model.xml` [U] | F2 | Partial: FARIS writes OpenMC inputs for its own runs; no stand-alone model export |
| INT-021 | FARIS shall import an OpenMC model and its statepoint results. | `model.xml` and `statepoint.h5` read into a study with units and nuclear-data identity; unsupported features listed, never dropped silently. | Corpus of ≥ 10 OpenMC models. | OpenMC statepoint format [U] | F6 | Partial: recorded OpenMC outputs are normalised into bundles |
| INT-022 | FARIS shall export an MCNP-style input as a text conversion. | Parser round trip exact; runs in MCNP or the openmc_mcnp_adapter path agree within 3σ on 3 benchmarks (provisional: MCNP licence limits external runs; confirm before F6 gate). | Parser test; external run when available. | openmc_mcnp_adapter [U] | F6 | No |
| INT-023 | FARIS shall read OpenMC HDF5 nuclear data libraries and report identity. | Library name, version and SHA-256 recorded on every result (CFG-055); a library not matching the expected hash is refused. | Library-identity test. | OpenMC library layout [U] | F2 | Partial: library hash recorded per run |
| INT-024 | FARIS shall read ENDF-6, ACE and GNDS metadata well enough to identify and cite the source. | Evaluation name, version and date extracted from 100 % of ≥ 20 sample files; no cross-section content is altered. | Metadata extraction test. | ENDF-6 manual, GNDS (OECD/NEA) [U] | F6 | No |
| INT-025 | FARIS shall record the FENDL and photon data versions and their licence terms in every export. | 100 % of exports name library, version, hash and licence; redistribution restrictions stated. | Export content test. | Licence clarity is table stakes [R6] | F2 | Partial: library hash recorded; licence terms not in exports |

## Fields, scene and views

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-030 | FARIS shall export mesh and field results to VTU and VTKHDF. | 100 % of mesh tallies load without warnings in ParaView LTS and PyVista; values bit-identical to internal; units stored as field attributes. | Load tests. | VTKHDF spec 2.8, types PolyData, UnstructuredGrid, ImageData, RectilinearGrid and others [V]: [VTKHDF format](https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/index.html) | F2 | No |
| INT-031 | FARIS shall export the 3D scene as glTF 2.0. | 0 errors from the Khronos glTF validator on every export; component names and material labels preserved. | Validator in CI. | glTF 2.0 (ISO/IEC 12113:2022) [U]; glTF-Validator [U] | F2 | No |
| INT-032 | FARIS shall import Exodus II and CGNS meshes for field overlay. | ≥ 1 reference mesh per format with node and element counts exact. | Corpus test. | SEACAS, CGNS [U] | F7 | No |
| INT-033 | FARIS shall export figures in SVG and PNG with data attached. | Each chart export includes its data series and units as metadata or a sidecar CSV; opens in Inkscape and a browser without warnings. | Open test; sidecar check. | Existing exports [docs/STUDY_EXPORT.md] | F1 | Partial: SVG and PNG chart exports and CSV exist; data not embedded in the figure |
| INT-034 | A figure export shall reproduce the on-screen figure. | Pixel difference of PNG export versus the on-screen render ≤ 1 % on 10 chart types. | Image comparison test. | FARIS choice | F2 | Unmeasured |

## Tables and arrays

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-040 | FARIS shall export results as CSV with a self-describing header. | 100 % of CSV files carry units in column names, a kind label per column, the study hash and FARIS version; decimal point always `.`, UTF-8, LF. | Header lint on all exports; locale test (CFG-037). | Existing CSV export | F1 | Partial: CSV export exists and is stamped with the study hash; units and kind labels not audited per column |
| INT-041 | FARIS shall export JSON with a published schema. | 100 % of JSON exports validate against a released schema; keys stable per CFG-076. | Schema validation. | JSON Schema [U] | F1 | Partial: JSON exports exist with version strings; no published schemas |
| INT-042 | FARIS shall export Parquet and HDF5 with metadata in-file. | Units, kind labels, provenance and licence stored in file metadata (Parquet key-value, HDF5 attributes); readable by pandas and h5py without FARIS. | Read test with both libraries. | Parquet, HDF5 [U] | F2 | No |
| INT-043 | FARIS shall import time series and tables from CSV, Parquet and HDF5 with mapping. | Column mapping recorded; units required or declared per column; dimension mismatch is an error (CFG-041). | Import tests with mis-labelled columns. | FARIS choice | F4 | No |
| INT-044 | Every export shall carry units, kind labels and provenance. | 100 % of export formats embed units; a lint over the export registry fails a format that cannot carry them and requires a sidecar. | Export registry lint. | House rule; Mars Climate Orbiter lesson [U] | F2 | Partial: units appear in exports in most places; no lint |

## Fusion data standards

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-050 | FARIS shall read and write IMAS IDS for the subsets it uses, naming the Data Dictionary version. | Round trip lossless for supported fields of equilibrium, core_profiles, wall and neutron source; DD version declared in every file and shown in the matrix; units match the DD. | IDS round-trip test; DD validation tool run in CI. | IMAS DD latest 4.1.1 shown on releases page; year not shown [V]; 4.1.0 added BALANCE_OF_PLANT and BREEDING_BLANKET: [IMAS DD releases](https://github.com/iterorganization/IMAS-Data-Dictionary/releases) | F5 | No |
| INT-051 | FARIS shall validate every IDS it reads or writes against the Data Dictionary. | 100 % of IDS operations validated; invalid files refused with the path named. | Corrupt-IDS corpus. | Fail-closed house rule | F5 | No |
| INT-052 | FARIS shall check which breeding_blanket and balance_of_plant fields it can fill before claiming support. | A field-by-field mapping table exists and every claimed mapping has a round-trip test; unmapped TBR or tally quantities stated as not covered. | Mapping table plus tests. | DD 4.1.0 added these IDSs; field coverage not inspected [V for existence, U for coverage] | F5 | No |
| INT-053 | FARIS shall accept a plasma neutron source from an IDS or equivalent source file. | Source profile read with units; integrated rate checked against the file within 1e-6; flagged as imported, authored or calculated. | Fixture test; see SRC in 02-radiation-transport. | IMAS neutron source [U] | F5 | No |
| INT-054 | FARIS shall support DD version upgrades without silent change. | When DD advances, a conversion report lists each moved or renamed field; files written under an older version still read. | Version-matrix test (3 DD versions). | IMAS DD versioning (3.x to 4.x) [U] | F5 | No |
| INT-055 | FARIS shall publish an IMAS support statement that names the DD version and IDS subset. | Statement in the interoperability matrix (INT-090); the phrase "IMAS compatible" does not appear without both. | Docs lint. | Trap: "IMAS compatible" without a DD version is empty [R4] | F5 | No |

## Systems engineering and PLM exchange

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-060 | FARIS shall read requirements from SysML v2 textual notation. | Requirement definitions, constraints and units parsed from ≥ 10 sample models; unsupported constructs listed, never dropped. | Parser tests on public SysML v2 samples. | SysML v2, KerML, Systems Modeling API and Services v1.0 adopted by OMG July 2025 [U: press release only] | F6 | No |
| INT-061 | FARIS shall read models and push results through the Systems Modeling API. | Read-only API client passes a contract suite against a reference server at F6; write of verdicts and margins follows (provisional: full API conformance deferred until demand; confirm before F6 gate). | Contract tests. | SysML v2 API and Services v1.0 [U] | F6 | No |
| INT-062 | FARIS shall import and export requirements as ReqIF. | Round trip of ≥ 10 files keeps id, text, attributes and links; unit and tolerance fields preserved in attributes. | ReqIF round-trip test. | ReqIF (OMG) [U] | F6 | No |
| INT-063 | FARIS shall export a bill of materials and a mass and volume summary for PLM. | CSV and JSON with component id, material id, mass, volume, units and source; totals within 1e-9 relative of the model. | Totals test. | Reactor-scale CAD practice [U] | F7 | No |
| INT-064 | FARIS shall write requirement verdicts back into an exchange format. | Verdict, margin, uncertainty and checker version exported per requirement for SysML v2 and ReqIF; verdicts are derived, never editable on import. | Export and reimport test. | Verdicts come from checkers (house rule); see DSN-070 | F6 | No |

## Other design codes, plant data and nuclear tools

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-070 | FARIS shall import PROCESS outputs through a documented mapping table. | Every mapped quantity lists PROCESS name, FARIS name, unit conversion and the mapping source; ≥ 1 worked example runs in CI; unmapped variables listed. | Example in CI. | PROCESS: Python-driven, VMCON solver, scan and UQ features [V]: [PROCESS](https://ukaea.github.io/PROCESS/); variable counts not verified [U] | F5 | No |
| INT-071 | FARIS shall import bluemira and FUSE outputs through documented mappings. | Mapping table plus one tested example each (provisional: output formats of both codes need reading before commitment; confirm before F5 gate). | Examples in CI. | bluemira ParameterFrame (value, unit, source); FUSE `dd`/`ini`/`act` structures [U] | F5 | No |
| INT-072 | Imported systems-code values shall carry their origin and kind label. | 100 % labelled "imported, calculated by <code> <version>", with the run identity; they never appear as FARIS calculated results. | Label test. | House rule | F5 | No |
| INT-073 | FARIS shall exchange activation data with ACTINV. | Round trip of ≥ 5 cases: inventory, decay heat and dose-rate tables with nuclide ids, units and library identity; ACTINV version recorded. | Round-trip test with the released ACTINV. | ACTINV adapter exists; format documented in the ACTINV handbook [internal] | F2 | Partial: ACTINV path detection in `faris doctor`; no data exchange test |
| INT-074 | FARIS shall read FISPACT-II style inventory outputs for comparison. | Nuclide inventory, dose and heat tables parsed from ≥ 3 sample outputs with units; read-only; licence of the source tool respected. | Parser test. | FISPACT-II output formats [U]; licence-limited tool | F4 | No |
| INT-075 | FARIS shall import measured plant data from CSV and IDS to compare with model predictions. | Imported series labelled "measured"; synthetic recovery test recovers an injected parameter within 5 %. FARIS remains a design model, not a digital twin, until a live path exists. | Synthetic recovery test. | R6 lifecycle row [U]; house rule on digital twins | F4 | No |
| INT-076 | FARIS shall import cost, price and availability data from CSV with source and date. | Each row carries unit, source and as-of date; stale rows flagged by age rule (ECO). | Import test. | See 04-plant-systems ECO | F4 | No |

## Loss, round trip and versioning

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-080 | A lossy conversion shall produce a loss list. | 100 % of lossy import and export paths write a loss report; strict mode fails on any loss. | Report test on all paths. | FARIS choice | F2 | No |
| INT-081 | Each format shall have a round-trip test on a corpus. | Loss measured per format: geometry volume ≤ 1e-6 relative, numbers ≤ 1 ulp unless the format is lossy (stated), names and ids exact; corpus of ≥ 10 files per format. | Corpus round-trip suite. | FARIS choice | F2 | No |
| INT-082 | Unknown optional fields shall be preserved on round trip. | 100 % of injected unknown fields kept byte for byte in own formats. | Round-trip test. | Protobuf forward-compatibility rule [U] | F1 | No |
| INT-083 | Every format shall carry a schema version and refuse a newer major. | As CFG-070 and CFG-074 for every export and import format. | Version-gate tests. | Fail-closed house rule | F1 | Partial: study, export, history carry versions; others unchecked |
| INT-084 | Imports from untrusted files shall be bounded. | Size, depth and entry-count limits enforced; ≥ 10 hostile files (zip bomb, deep nesting, huge counts) refused in ≤ 5 s without exceeding 1 GB of memory. | Hostile-corpus test. | Security rule; see 19-security | F2 | Unmeasured |
| INT-085 | Import and export shall run off the UI thread and be cancellable. | No frame > 50 ms during import of 1 GB; cancel stops in ≤ 2 s. | Frame log during import (PERF-014). | PERF-014, PERF-024 | F2 | Partial: study I/O runs behind a modal; frame log not taken |

## Support matrix

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| INT-090 | FARIS shall publish an interoperability matrix of every format with status. | Each cell: tested in CI, untested or unsupported; version of the other tool or standard; direction; known losses. A format never appears as supported without a CI test. | Docs check against the test list. | FARIS choice | F1 | No |
| INT-091 | The matrix shall be generated from the tests. | Matrix and tests cannot disagree: CI fails when a "tested" cell has no test id. | Generation diff. | QA-002 | F1 | No |
| INT-092 | FARIS shall pin and display the versions of external tools and standards it was tested with. | 100 % of cells name versions (OpenMC, ParaView, DD, DAGMC, SysML v2 API); a version drift check runs monthly. | Monthly CI job. | Trap: format drift [R4] | F2 | No |
| INT-093 | A user shall be able to see what each export leaves out. | Each export dialog and CLI help lists omitted data classes (evidence, receipts, licensed data) before writing. | UI and CLI text test. | FARIS choice | F1 | Partial: export notes name omitted items in the manifest; not in the dialog |

## Traps

- "Supports STEP" means nothing without a corpus pass rate and a transport-readiness check. A model that imports but leaks, overlaps or has gaps gives wrong neutronics.
- "IMAS compatible" without the Data Dictionary version and the list of IDS fields is empty, and coverage of blanket and tally quantities in recent dictionaries has not been inspected.
- A round trip that ignores units, names and ids passes while losing the meaning. Test losses per field, not file validity.
- Export formats that cannot carry units become the next unit mix-up. Sidecars are the fallback, never silence.
- A matrix with untested cells is marketing. Cells without a CI test are labelled so.
- Provenance export (RO-Crate) that nobody can replay is compliance theatre. The metric is replay success.
- Imported numbers from another design code are not FARIS results; labelling them as such matters more than conversion accuracy.
- Open specification is real only if a clean-room reader passes the fixtures (INT-001).
