# R4: Configurability, extensibility, automation, interoperability, workflows, provenance (2026-10-05)

Verification status (updated after coordinator reminder): a live-fetch pass was done on the numeric items listed in the
VERIFIED block below; those are tagged [V]. Everything else numeric or specific is [U] (recalled from prior knowledge, NOT
fetched this session) and must be checked before becoming a hard requirement. Existing "(unverified)" markers mean [U].
Anything labelled (provisional) is a proposed target with stated uncertainty.

VERIFIED [V] (fetched 2026-10-05):
- [V] PEP 387: warning in >= 2 minor versions of the same major; preferred 5 years (e.g. warn 3.10, remove 3.15); minimum >= 2 years. https://peps.python.org/pep-0387/
- [V] Slurm: "The default value of MaxArraySize is 1001." https://slurm.schedmd.com/job_array.html
- [V] SALib Saltelli sampler yields N*(2D+2) samples (N*(D+2) with calc_second_order=False). https://salib.readthedocs.io/en/latest/user_guide/basics.html
  Hence D=10, N=1024: 22*1024 = 22,528 evaluations (arithmetic from the verified formula).
- [V] SALib Morris sampler rows = (G/D+1)*N/T, D columns (N trajectories); so r trajectories cost r(D+1) when G=D. https://salib.readthedocs.io/en/latest/api/SALib.sample.morris.html
- [V] OpenMC restart: `openmc -r statepoint.100.h5` / `openmc.run(restart_file=...)`; statepoint must match model; continues to batch count in settings. https://docs.openmc.org/en/stable/usersguide/settings.html (page does not state fixed-source vs eigenvalue scope or seed/thread reproducibility: that part is [U]).
- [V] RFC 6962: "A log is a single, ever-growing, append-only Merkle Tree". https://www.rfc-editor.org/rfc/rfc6962
- [V] OpenMC RNG page confirms skip-ahead exists for parallel reproducibility but does NOT state thread/MPI independence: that claim is [U]. https://docs.openmc.org/en/stable/methods/random_numbers.html
- [V-negative] PROCESS About page and IMAS-Python index page do NOT state equation/variable counts or DD versions; my earlier "~80 constraints / ~170 iteration variables" and DD dates are [U]. PROCESS page does confirm a VMCON solver and scan/UQ features. https://ukaea.github.io/PROCESS/
- [V-negative] Blender extension permissions URL tried returned 404; permission list claim is [U].
SECOND VERIFICATION PASS (2026-10-05, coordinator follow-up):
- [V] OpenMC parallelization methods page: "to ensure that the results are reproducible, one must guarantee that the process by which fission sites are randomly sampled does not depend on the number of processors"; fission sites are sorted by unique id before sampling. So processor-count independence is a stated design goal for the eigenvalue fission-bank algorithm. https://docs.openmc.org/en/stable/methods/parallelization.html . Per-particle seeding wording and fixed-source scope NOT found: remains [U]. Also bitwise equality across compilers/architectures is not claimed anywhere I fetched: keep determinism classes in PRV-5/6.
- [V] IMAS Data Dictionary GitHub releases page: latest shown 4.1.1 (released 26 Feb; year not shown on fetched page, so presumably 2026 [U]); 4.1.0 (14 Nov) added BALANCE_OF_PLANT and BREEDING_BLANKET IDSs. This corrects my earlier "thin neutronics/blanket coverage" in 1.4: a breeding_blanket IDS now exists (field-level coverage of TBR/tally data NOT inspected, [U]). Licence NOT confirmed (page showed none; LICENSE.txt path 404). https://github.com/iterorganization/IMAS-Data-Dictionary/releases
- [V] VTKHDF: spec version 2.8; types PolyData, UnstructuredGrid, ImageData, RectilinearGrid, StructuredGrid, HyperTreeGrid, Table, plus OverlappingAMR, MultiBlockDataSet, PartitionedDataSetCollection; has its own status page; extension .vtkhdf. Actively maintained, versioned. https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/index.html
- [V] RO-Crate: latest is 1.3 ("current long term release"); 1.2 is released and stable. My earlier "1.1/1.2" should read "1.2 stable, 1.3 current": target RO-Crate 1.3 (or 1.2 if tooling lags). https://www.researchobject.org/ro-crate/specification.html
- [U] FAIR4RS: nature.com/doi.org fetch blocked by redirect; DOI 10.1038/s41597-022-01710-x remains from memory (not opened).
- [U] Blender extension manifest permissions: manual pages fetched did not contain the field (5.2 LTS manual index only). Claim stays [U]; requirement PLG-3 should cite the manifest only after reading it.
All other URLs in section 1 are pointers to primary docs that I did not open in this pass; treat their content claims as [U].

## 1. Best-in-class landscape

### 1.1 Scripting / API
- OpenMC: the model is defined entirely through the Python API (openmc.Model; export_to_xml/ from_xml round-trips), and
  the CLI `openmc` only runs XML. Every input is reachable from Python, so parity is 100% by construction because there
  is no GUI. Releases are numbered 0.x (0.15.x series); no formal semver stability promise, deprecations via
  DeprecationWarning for roughly one release. https://docs.openmc.org/en/stable/usersguide/ ; https://docs.openmc.org/en/stable/releasenotes/index.html
- Blender bpy: Python API is the same layer the UI is built on (operators, properties); essentially all UI operators are
  callable. API stability: breaking changes only on major/minor (x.y) releases, listed in "Python API" release notes,
  deprecation of ~2 releases for ordinary API [U]; stable ABI not promised for C-extension. Startup ~1-3 s [U]. https://docs.blender.org/api/current/info_api_reference.html ; https://developer.blender.org/docs/release_notes/
- ParaView/pvpython: GUI "Python Trace" records GUI actions as a Python script (the key parity mechanism); state files
  (.pvsm) and Catalyst for in situ. https://docs.paraview.org/en/latest/UsersGuide/introduction.html#python-trace ; Python Trace section of the guide.
- COMSOL: Model Java API and LiveLink for MATLAB/Simulink; every model-tree node is scriptable; GUI can "Save as Model File for Java/MATLAB/VBA"
  (code generation from GUI). https://www.comsol.com/livelink-for-matlab
- Ansys PyAnsys: per-product PyMAPDL, PyFluent, PyAEDT etc., semver on PyPI, gRPC backends, typed (py.typed in several),
  Jupyter-friendly; Fluent journal recording of GUI actions. https://docs.pyansys.com/
- FreeCAD: Python console echoes GUI commands (macro recording), full Python API for Part/Sketcher; documented API is not
  frozen (0.x/1.0 breaks). https://wiki.freecad.org/Python_scripting_tutorial
- Jupyter: widgets (ipywidgets/anywidget), nbformat is versioned (v4.5), kernels via ZMQ protocol with versioned message spec. https://jupyter-client.readthedocs.io/en/stable/messaging.html
- Semver spec: https://semver.org/ ; PEP 387 (CPython backward-compat policy: deprecation >= 2 releases) https://peps.python.org/pep-0387/ ;
  Rust: cargo-semver-checks automates breaking-change detection https://github.com/obi1kenobi/cargo-semver-checks ; PyO3/maturin for typed Python bindings, .pyi stubs via pyo3-stub-gen; PEP 561 py.typed.
- Reference value for "GUI parity": no tool publishes a measured percentage; Blender/ParaView achieve it structurally (UI built on API / trace).
  This is the model FARIS should copy: UI calls the same command layer that the API exposes.

### 1.2 Plugins / adapters / fusion integration
- VS Code: extensions run in a separate Extension Host process, contributions declared in package.json manifest, `engines.vscode`
  version-range compat, Marketplace discovery. https://code.visualstudio.com/api/get-started/extension-anatomy
  Not sandboxed against file/network access (extensions have user privilege) - a known gap. https://code.visualstudio.com/docs/editor/extension-runtime-security
- Blender add-ons: bl_info + (4.2+) extension manifest (blender_manifest.toml) with `blender_version_min`, permissions declaration (files/network/clipboard/camera/microphone), online extensions platform. https://docs.blender.org/manual/en/latest/advanced/extensions/index.html
- ParaView plugins (XML server-manager + Qt/Python). QGIS plugin repository with metadata.txt `qgisMinimumVersion`. https://docs.qgis.org/latest/en/docs/pyqgis_developer_cookbook/plugins/
- WebAssembly component model / WASI as the credible sandbox for Rust hosts (wasmtime): capability-based, deterministic, fuel-metered. https://component-model.bytecodealliance.org/ ; https://wasmtime.dev/
- Fusion frameworks: IMAS actors/workflows run on IDS-in/IDS-out contracts (Kepler, then Python), https://imas.iter.org ; https://imas-python.readthedocs.io (IMAS-Python, formerly imaspy);
  FUSE.jl (GA) uses "actors" with a shared `dd` (IMAS data dictionary-structured) and `ini`/`act` parameter structures, each actor has a typed ParametersActor and a documented IMAS in/out. https://fuse.help/ ; https://github.com/ProjectTorreyPines/FUSE.jl ;
  bluemira (UKAEA/Fusion Reactor Design) has `codes` interface (PROCESS, PLASMOD, etc. wrapped as solver classes with a `Run/Read/Write` mode pattern and a ParameterFrame with units & source tracking). https://bluemira.readthedocs.io/ ; https://github.com/Fusion-Power-Plant-Framework/bluemira
  OMAS (GA): ODS (OMAS data structure) over IMAS-like schema with unit-aware conversions, many formats. https://gafusion.github.io/omas/
  PROCESS: Python-driven input (IN.DAT, possibly moving to newer formats [U]), also an IMAS interface. https://github.com/ukaea/PROCESS

### 1.3 Configuration / scenario formats
- JSON Schema 2020-12 (https://json-schema.org/specification) and Rust `schemars`; TOML 1.0 (https://toml.io/en/v1.0.0); YAML 1.2 (footguns: Norway problem).
- Units: pint (Python; https://pint.readthedocs.io), UDUNITS-2 (https://www.unidata.ucar.edu/software/udunits/), Rust `uom` (compile-time) and `dimensioned`; QUDT ontology (https://qudt.org); Mars Climate Orbiter loss (1999) is the canonical failure (NASA MCO mishap report, https://llis.nasa.gov/lesson/641).
- Defaults transparency/provenance: bluemira ParameterFrame stores value+unit+source per parameter (see above); PROCESS writes full output with input-vs-default flags (unverified detail).
- Large data: HDF5 (https://www.hdfgroup.org/solutions/hdf5/), Zarr v3 (https://zarr-specs.readthedocs.io), VTKHDF [V] spec v2.8 (https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/index.html), OpenMC statepoint.h5/summary.h5.
- Migration: Cargo/Django-style versioned migrations; Kubernetes API conversion; Flyway. Best-in-class = every on-disk schema carries a version, N-to-N+1 migrations are tested with golden files.

### 1.4 Fusion/neutronics data standards and import/export
- IMAS: Data Dictionary releases are versioned (3.x -> 4.x; DD 4.0 released 2024; unverified exact date) with IDS per physics domain (equilibrium, core_profiles, wall, neutron_source? check; `neutron_diagnostic`, `wall`, `pf_active`, `tf`, `blanket`/`neutronics` IDSs exist in recent DD). Backends: HDF5, MDSplus, UDA, and NetCDF in newer AL5. https://imas-python.readthedocs.io ; https://imas.iter.org/ ; Data dictionary: https://github.com/iterorganization/IMAS-Data-Dictionary (verify org path).
  [V] DD 4.1.0 added BALANCE_OF_PLANT and BREEDING_BLANKET IDSs (see verification block); field coverage for TBR/mesh tallies not inspected [U], so INT-7 should be re-scoped after reading those IDSs; FARIS could plausibly export blanket/plant-balance quantities.
- DAGMC: dagmc.h5m on MOAB; built from CAD via CAD-to-DAGMC (STEP -> Cubit/Gmsh/OCC -> h5m); watertightness (<1e-? gap) is the key criterion. https://svalinn.github.io/DAGMC/ ; https://github.com/fusion-energy/cad_to_dagmc
- OpenMC: XML (geometry/materials/settings/tallies), HDF5 outputs, `openmc.Model` single-file model.xml (since 0.13). https://docs.openmc.org/en/stable/io_formats/index.html
- MCNP: text input (cells/surfaces/data cards); `openmc.model.from_mcnp`? not official; conversion tools: mcnp2openmc / openmc_mcnp_adapter, https://github.com/openmc-dev/openmc_mcnp_adapter ; Serpent, FLUKA, Geant4 GDML, Attila. Unfolding: UKAEA-style `Unified Neutronics`? skip.
- CAD: STEP AP203/214/242 (ISO 10303), IGES (legacy), BREP; OpenCASCADE (https://dev.opencascade.org) as the open kernel; mesh: STL, OBJ, PLY, 3MF; glTF 2.0 (ISO/IEC 12113:2022, https://www.khronos.org/gltf/) for 3D viewing exchange; USD (https://openusd.org); IFC for plant/BIM (ISO 16739).
- Field/mesh data: VTK legacy/VTU/VTKHDF; XDMF; Exodus II (Sandia, https://sandialabs.github.io/seacas-docs/); CGNS (https://cgns.github.io); MED (Salome); UGRID; OpenMC mesh tallies map to Cartesian/cylindrical/spherical/unstructured (libMesh/MOAB). 
- Tools like Salome, ParaView, Gmsh and meshio (https://github.com/nschloe/meshio; reads/writes ~30+ mesh formats; count unverified) set the import/export-breadth reference. FreeCAD/Onshape/SolidWorks: STEP/IGES/STL import-export all standard; COMSOL imports ~20+ CAD/mesh formats (unverified count).
- Nuclear data: ENDF-6 (https://www.nndc.bnl.gov/csewg/docs/endf-manual.pdf), GNDS (OECD/NEA WPEC SG38, https://www.oecd-nea.org/jcms/pl_39689), ACE, HDF5 OpenMC library, FENDL-3.2 (IAEA, https://www-nds.iaea.org/fendl/), EXFOR.

### 1.5 Headless / batch / HPC / cloud
- Slurm (https://slurm.schedmd.com/documentation.html), PBS; MPI; OpenMC supports MPI+OpenMP and its CLI is headless; Apptainer (https://apptainer.org) for HPC containers, Docker/OCI; OpenMC publishes official Docker images.
- Caching: Nix (content-addressed store; https://nixos.org/manual/nix/stable/), Bazel remote cache/RBE (https://bazel.build/remote/caching), DVC (https://dvc.org), Snakemake, Cargo/sccache. Key property: key = hash(all inputs + tool version + env), result immutable.
- Throughput reference: no authoritative published "cases/hour" for neutronics; throughput is bounded by transport cost. FARIS's own observed anchor: 1M-history OpenMC run (time per run per FARIS notes; exact seconds not in this brief) and 1 s history recalculation. DOE of N cases: reuse is the lever (cache hits skip transport).
- Resumable jobs: OpenMC supports restart from statepoint (`openmc -r statepoint.N.h5`) for criticality and via `max_batches` extension for fixed source (verify); checkpoint = batch-level.
- Job queues / orchestration: Dask/Ray, Parsl, FireWorks, AiiDA daemon, Slurm arrays ([V] default MaxArraySize 1001; sites differ; https://slurm.schedmd.com/job_array.html).

### 1.6 Workflow and provenance standards
- W3C PROV-O/PROV-DM (https://www.w3.org/TR/prov-overview/); RO-Crate 1.2 stable / 1.3 current [V] (https://www.researchobject.org/ro-crate/) ; Workflow Run RO-Crate profiles (https://www.researchobject.org/workflow-run-crate/);
  FAIR principles (Wilkinson et al. 2016, https://doi.org/10.1038/sdata.2016.18, 15 sub-principles); FAIR4RS (Barker et al. 2022, https://doi.org/10.1038/s41597-022-01710-x, 4 principles F/A/I/R); CITATION.cff (https://citation-file-format.github.io); CodeMeta (https://codemeta.github.io); SPDX SBOM (ISO/IEC 5962:2021); CycloneDX (ECMA-424).
- AiiDA: every calculation stored as node in a DAG (data, calculation, workflow nodes), automatic provenance, queryable by SQL/QueryBuilder; "never lose provenance" design; (Huber et al. 2020, https://doi.org/10.1038/s41597-020-00638-4).
- Snakemake (https://snakemake.readthedocs.io), Nextflow (https://www.nextflow.io), CWL v1.2 (https://www.commonwl.org/v1.2/), all give re-runs only for changed inputs; CWL has formal spec, conformance tests.
- Reproducibility: reproducible-builds.org practices, `SOURCE_DATE_EPOCH`, Nix/Guix bit-for-bit builds; for MC transport, bitwise reproducibility needs fixed seed + fixed thread/MPI decomposition + same FP/compile (OpenMC uses per-particle seeding via skip-ahead, which gives results independent of thread count for fixed-source - verify with OpenMC docs https://docs.openmc.org/en/stable/methods/random_numbers.html).
- Data citation: DataCite DOIs (https://datacite.org), Zenodo versioned DOIs (concept + version DOI), Joint Declaration of Data Citation Principles (https://force11.org/info/joint-declaration-of-data-citation-principles-final/).
- Software supply chain: SLSA (https://slsa.dev), Sigstore/cosign signing, in-toto attestations.

### 1.7 Design workflows
- DOE/sampling: Latin hypercube, Sobol sequences (scipy.stats.qmc), Halton; Sobol/Saltelli sensitivity indices and Morris screening in SALib (https://salib.readthedocs.io). Cost: [V] Saltelli needs N(2D+2) evals (N typically 2^10..2^12 -> for D=10: 22*1024 ~ 22,528 evals); Morris needs r(D+1) [V] with r=10..50 [U]. (Saltelli et al. 2010, https://doi.org/10.1016/j.cpc.2009.09.018)
- Frameworks: Dakota (https://dakota.sandia.gov; UQ, DOE, optimization, surrogate, parallel evaluation concurrency), OpenMDAO (https://openmdao.org; analytic derivatives, MDO), pymoo (https://pymoo.org; NSGA-II/III), BoTorch/Ax (https://botorch.org), Optuna (https://optuna.org; multi-objective, pruning, storage backends incl. RDB), SMT (surrogates, https://smt.readthedocs.io), UQpy, OpenTURNS (https://openturns.github.io).
- Surrogates with error control: Gaussian process predictive variance, leave-one-out/k-fold CV (report RMSE, Q2 >= 0.9 typical rule of thumb), polynomial chaos with Sobol from coefficients; adaptive refinement until held-out error < tolerance. Q2 >= 0.9 threshold is a heuristic [U].
- Systems codes: PROCESS has a VMCON constrained optimiser, figures of merit (e.g. capital cost, COE), iteration variables and constraint equations (~80+ constraint equations and ~170 iteration variables per docs; verify), a "scan" mode and an optional UQ tool; FUSE has a Workflow/Optimization (including multi-objective via "optimization" with Metaheuristics.jl; verify) and a Monte-Carlo "uncertainty" mode. https://ukaea.github.io/PROCESS/ ; https://fuse.help/
- Multiobjective: Pareto front with hypervolume indicator, non-dominated sort; constraint handling via feasibility-first (Deb rules) or penalty. 

### 1.8 UI configurability
- VS Code: settings.json (JSON with schema, per-user/workspace/folder scope), keybindings.json, themes, Settings Sync. https://code.visualstudio.com/docs/getstarted/settings
- Blender: Preferences, keymap editor, workspaces, themes as XML/py, factory reset; app-templates.
- Units preference: Blender Scene Units (metric/imperial, length/mass/time/temperature scale), FreeCAD Preferences > Units (schemas); Onshape per-document units; Paraview no unit system.
- XDG Base Directory spec for Linux config paths (https://specifications.freedesktop.org/basedir-spec/latest/); `directories` crate in Rust.

### 1.9 Versioning / collaboration of studies
- Git-friendly text: TOML/JSON with sorted keys, one param per line, stable float formatting; Jupytext for notebooks; nbdime for notebook diff/merge (https://nbdime.readthedocs.io). Onshape: branching/merging of CAD documents with version graph and full history, no file locking (https://cad.onshape.com/help/Content/branch.htm). Git LFS, DVC for big data. Audit trails: append-only, hash-chained logs (Merkle chain, e.g., Certificate Transparency RFC 6962 https://www.rfc-editor.org/rfc/rfc6962), 21 CFR Part 11 audit-trail expectations as a model for tamper-evident records (https://www.ecfr.gov/current/title-21/chapter-I/subchapter-A/part-11).
- Real-time co-editing (CRDT: Automerge/Yjs, https://automerge.org) relevant only if FARIS goes multi-user; offline-first single-user is the realistic baseline.

## 2. Candidate requirements

Column order: Area | Requirement | Metric | Best-in-class reference | Proposed FARIS target | How to verify

### 2.1 Scripting / API
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| API-1 | FARIS shall expose every GUI action through a single command layer that the GUI itself calls | GUI-action parity = scriptable actions / total GUI actions | Blender (UI built on bpy), ParaView Python Trace | 100%; CI fails if a UI command lacks a registered API id | Automated enumeration of UI command registry vs API registry; test fails on diff |
| API-2 | FARIS shall record GUI sessions as a replayable script | Replay of recorded session reproduces the same project hash | ParaView Python Trace; COMSOL "Save as Java/MATLAB" | 100% of recordable commands; replay hash-identical on 20 golden sessions | Golden-session replay test in CI |
| API-3 | FARIS shall ship a Python package (typed) wrapping the Rust core | Type-check pass; stubs present | PyAnsys (py.typed), PyO3 stubs | `mypy --strict` and `pyright` clean on the shipped stubs; py.typed present; 100% public symbols annotated | CI type-check job over public API + docs examples |
| API-4 | FARIS shall declare a semantic-versioned public API and CLI/file-format contract | Breaking change without major bump | semver.org; cargo-semver-checks | 0 undeclared breaking changes per release (post-1.0); pre-1.0: break only on minor with changelog entry | cargo-semver-checks + Python API diff tool (griffe) in release CI |
| API-5 | FARIS shall deprecate before removal | Releases between DeprecationWarning and removal | [V] PEP 387 (>=2 minor releases, >=2 years, preferred 5 years); Blender ~2 [U] | >= 2 minor releases AND >= 6 months; warning names the replacement | Deprecation registry test: each removed symbol had a prior warning |
| API-6 | FARIS shall work in Jupyter with rich display | Notebook example executes; results render | PyAnsys, OpenMC notebooks | Every public result type has `_repr_html_`/plot; all docs notebooks run in CI headless in < 10 min | nbmake/pytest over docs notebooks |
| API-7 | FARIS shall provide a stable machine-readable command/RPC interface (JSON-RPC or gRPC) | Version negotiation; schema published | Jupyter messaging protocol (versioned), LSP | Interface version handshake; schema file in repo; backward compat for 1 major | Contract tests against previous release client |
| API-8 | FARIS shall document every public API item with a runnable example | Doc coverage | docs.rs/Sphinx | 100% public items documented; >= 90% with example; doctests pass | `#![deny(missing_docs)]`, doctests, interrogate |
| API-9 | The CLI shall have parity with the API for all non-interactive operations | CLI coverage | OpenMC CLI, Dakota | 100% of non-view commands; consistent exit codes (0 ok, distinct non-zero per failure class); `--json` output on all | CLI/API enumeration test; exit code table test |
| API-10 | API errors shall be typed and machine-readable | Error with stable code + remediation | Rust thiserror; Blender operator reports | Every error has stable id (E-xxxx) documented; 0 panics reachable from API (fuzz) | cargo-fuzz on API entry points; error-id docs check |

### 2.2 Plugins / adapters
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| PLG-1 | FARIS shall define a versioned adapter contract for external codes (inputs, outputs, units, failure modes) | Contract version + conformance suite | FUSE actors (IMAS in/out), bluemira codes interface | Each adapter declares contract version; conformance test suite of >= 20 checks runs against every adapter (OpenMC, ACTINV first) | CI conformance job |
| PLG-2 | An adapter shall declare supported tool versions and refuse unknown ones unless overridden | Version-range check | VS Code `engines`, Blender `blender_version_min` | Fail-closed on out-of-range; override recorded in provenance | Test with fake versions |
| PLG-3 | Adapters shall run as separate processes (or wasm) with declared capabilities (fs paths, network, exec) | Capabilities enforced | Blender 4.2 extension permissions; WASI | Default: no network, write only to run dir; violation = abort + log; Linux: seccomp/landlock or cgroup | Negative tests (adapter attempts forbidden op) |
| PLG-4 | FARIS shall support third-party plugins without recompiling the app | Plugin loads w/o rebuild | VS Code, QGIS | WASM component or out-of-process plugin load; cold load < 500 ms (provisional: VS Code host start order of 100s ms) | Plugin load benchmark |
| PLG-5 | Plugins shall be signed and listed with provenance | Signature verification | Sigstore, Blender extensions platform | Unsigned plugin requires explicit user opt-in per plugin; signature + hash shown in UI | Signed/unsigned plugin tests |
| PLG-6 | Plugin API shall have its own semver, independent of app | API version string | VS Code API versions | Plugin API stable across all minor releases of one major | Plugin compat test matrix (N, N-1) |
| PLG-7 | Plugin-supplied numbers shall carry the same status label (calculated/authored/...) and units as built-in ones | Label required | FARIS model | 100% of plugin outputs have status+unit+source or load is rejected | Schema validation |
| PLG-8 | FARIS shall list installed adapters, versions, hashes and health in UI and CLI | Discoverability | `code --list-extensions`, `pip list` | `faris adapters list --json` and UI panel; health check < 5 s per adapter | CLI test |
| PLG-9 | Adapter crash/timeout shall not crash FARIS or corrupt project | Isolation | VS Code extension host | 0 project corruption in 1000 injected-crash runs; timeout configurable, default set per adapter | Fault-injection test |
| PLG-10 | Adapter examples/templates shall exist (a minimal adapter in <= 200 lines) | Template exists, tested | | Template repo builds and passes conformance in CI | CI |

### 2.3 Configuration / scenario formats
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| CFG-1 | Every input file format shall have a published JSON Schema (2020-12) | Schema coverage | JSON Schema; schemars | 100% of user-authored file types; schema published per release | Schema generated from Rust types in CI; diff = fail |
| CFG-2 | Inputs shall be validated with errors giving file, line, path, expected, got, and a suggestion | Error quality | rustc diagnostics; Avila Core diagnostics | 100% validation errors have location + fix hint; unknown keys rejected (deny_unknown_fields) | Negative-corpus test of >= 100 malformed inputs |
| CFG-3 | Every dimensional input shall carry a unit and be dimension-checked | Unit errors caught | MCO (1999) failure; pint | 0 unitless dimensional values accepted; dimension mismatch = hard error; fuzz-injected wrong-dimension inputs (>= 500) all rejected | Property tests; unit-fuzz |
| CFG-4 | Unit conversion shall be exact to documented tolerance and round-trip | Round-trip error | UDUNITS; pint | Relative error <= 1e-12 for f64 conversions on all supported units | Property test |
| CFG-5 | Scenario files shall be diff-friendly text (TOML/JSON canonical form) | Canonical form | git diff | Canonical serialisation: sorted keys, shortest round-trip floats, LF, trailing newline; parse->write is byte-identical on corpus | Round-trip test on all fixtures |
| CFG-6 | Every file format shall be versioned and auto-migrated from all prior released versions | Migration coverage | Django/Flyway; Kubernetes conversion | Migration from every released schema version N to current; golden files per version; migrated result passes validation; original preserved | Migration test across all archived fixtures |
| CFG-7 | Migrations shall be reported, not silent | Migration log | | A migration writes a record (from->to, changes) into provenance | Test |
| CFG-8 | Newer-than-supported files shall be refused with a clear message, not partially loaded | Forward-compat behaviour | | 100% fail-closed with required version stated | Test |
| CFG-9 | Defaults shall be transparent: every effective parameter shows whether it is user-set, default, derived or literature, with source | Parameter origin coverage | bluemira ParameterFrame (value+unit+source) | 100% of effective parameters carry origin tag; `faris params explain <id>` outputs origin chain | Enumeration test |
| CFG-10 | A fully-resolved ("effective") configuration shall be exportable | Effective-config dump | Nextflow `-resume` config dump, Hydra | `faris config resolve` yields complete file; re-running it yields identical result hash | Test |
| CFG-11 | Templates/presets for common studies with documented provenance | Template count + validity | | >= 5 templates at 1.0; all validate and run in CI | CI |
| CFG-12 | Large arrays shall be stored in chunked, compressed binary (HDF5/Zarr/Arrow) referenced by hash from text config | Format | HDF5, Zarr v3 | Text config < 1 MB for any study; arrays externalised with content hash | Size test |
| CFG-13 | Config layering (defaults < project < study < CLI override) shall be defined and shown | Precedence documented + test | Hydra, VS Code settings scopes | Precedence order tested; `--show-origin` | Test |
| CFG-14 | Environment/path configuration shall follow XDG on Linux, native conventions elsewhere | Path compliance | XDG spec | 100% user config under XDG dirs; none in home root | File-system audit test |
| CFG-15 | Numeric inputs shall have declared valid ranges with physical justification and warn-vs-error levels | Range coverage | | 100% of parameters have range; out-of-range = error or labelled override | Enumeration test |

### 2.4 Interoperability / data standards
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| INT-1 | FARIS shall import STEP (AP203/214/242) geometry | Import success rate on corpus | OpenCASCADE; FreeCAD | >= 95% of a >= 50-file public STEP corpus (NIST CAD test models, https://www.nist.gov/ctl/smart-connected-systems-division/smart-connected-manufacturing-systems-group/mbe-pmi-0 ) imports with correct volume (rel. error <= 1e-6) | Corpus test |
| INT-2 | FARIS shall export OpenMC model (XML + materials + tallies) that runs unmodified in OpenMC | Round-trip equivalence | OpenMC `Model` | Export->run in stock OpenMC reproduces FARIS TBR within 3 sigma at equal seed/histories; 100% of supported geometry features | Cross-run test |
| INT-3 | FARIS shall export/import DAGMC h5m | Watertight | DAGMC / cad_to_dagmc | Exported h5m passes DAGMC watertightness check; volume rel. error <= 1e-4 vs CAD | Test with DAGMC tools |
| INT-4 | FARIS shall export MCNP input | Cross-code agreement | openmc_mcnp_adapter | Export runs in MCNP/OpenMC; TBR agreement within 3 sigma on 3 benchmark models (provisional: MCNP availability/licence limits verification; may be text-diff verified only) | Parser round-trip + external run when available |
| INT-5 | FARIS shall export results to VTK (VTU and VTKHDF), opening in ParaView without warnings | Validity | VTK docs | 100% mesh/tally outputs loadable in ParaView LTS and pyvista; values bit-identical to internal | Load tests |
| INT-6 | FARIS shall export 3D scene to glTF 2.0 and STL/OBJ | Validator pass | Khronos glTF-Validator | 0 errors from glTF-Validator (https://github.com/KhronosGroup/glTF-Validator) | Validator in CI |
| INT-7 | FARIS shall read/write IMAS IDS for the subsets it uses (equilibrium, wall, core_profiles, pf_active, and breeding_blanket / balance_of_plant if fields fit) | IDS round-trip | IMAS-Python | Round-trip lossless for supported fields at current DD LTS; declare DD version; units in IDS match DD | Round-trip test; DD version pin |
| INT-8 | FARIS shall import/export CSV/Parquet/HDF5/NetCDF for tabular and array results with a data dictionary | Self-describing | Parquet, NetCDF-CF | Every export has units and provenance metadata in-file (HDF5 attrs/Parquet metadata) | Metadata check |
| INT-9 | FARIS shall import nuclear data libraries in OpenMC HDF5 and report library identity/hash | Library hash recorded | OpenMC | Library id+hash in every result's provenance | Test |
| INT-10 | FARIS shall import Exodus II / CGNS meshes (read) | Read success | SEACAS, CGNS | Read >= 1 reference mesh per format with node/element count exact | Corpus test |
| INT-11 | Import/export coverage shall be published as a matrix with tested/untested status | Matrix | | Every cell: tested-in-CI / untested; no format claimed that is not tested | Docs check |
| INT-12 | Lossy conversions shall be reported with a loss list | Loss report | | 100% of lossy export paths produce report; strict mode fails on loss | Test |
| INT-13 | Round-trip fidelity on own formats: save->load->save is byte-identical | Byte identity | | 100% on corpus | Test |
| INT-14 | FARIS shall interoperate with units/metadata of PROCESS/FUSE/bluemira via documented mapping tables | Mapping coverage | bluemira, FUSE | Mapping table for >= 1 systems code (provisional: PROCESS first) with tested example | Example in CI |
| INT-15 | Network/file protocol versions shall be negotiated, and unknown optional fields ignored but preserved | Forward compat | Protobuf rules | Unknown fields round-trip | Test |

### 2.5 Headless / batch / HPC / cloud / caching
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| HPC-1 | FARIS shall run all computation without a display | Headless success | OpenMC CLI | 100% of non-view operations pass in CI with no DISPLAY | CI under xvfb-less container |
| HPC-2 | FARIS shall provide Slurm and PBS submission templates and a generic job backend trait | Backend tested | Slurm docs, Parsl | Slurm tested on real or containerised slurm (docker slurm cluster) in CI; arrays supported | Integration test |
| HPC-3 | FARIS shall ship OCI image and Apptainer definition, reproducible | Image digest stable; size | Apptainer; OpenMC images | Image build deterministic (same digest twice, provisional: depends on base image pinning); runs under Apptainer rootless | Build twice, compare |
| HPC-4 | FARIS shall run MPI-distributed transport via the adapter and report scaling | Parallel efficiency | OpenMC | Report; >= 80% strong-scaling efficiency to 32 ranks on reference problem (provisional: OpenMC-dependent, FARIS only orchestrates) | Benchmark |
| HPC-5 | Long jobs shall be resumable at batch granularity | Lost work on kill | OpenMC statepoint | On SIGKILL, <= 1 checkpoint interval lost; resume yields identical final result for deterministic runs | Kill-and-resume test |
| HPC-6 | FARIS shall cache expensive results content-addressed by (inputs, tool versions, data library hashes, seed, settings) | Hit correctness | Nix, Bazel | 0 false hits (stale result served) across mutation test of every input class; hit rate reported | Mutation test: perturb each key component, assert miss |
| HPC-7 | Cache shall be shareable (local dir, shared FS, S3-compatible remote) | Backends | Bazel remote cache | Local+shared-FS at 1.0; remote optional; integrity verified by hash on read | Corruption-injection test |
| HPC-8 | Cache shall have size limit and eviction policy, visible in UI/CLI | GC | Nix gc | `faris cache gc --max-size`; pinned results never evicted | Test |
| HPC-9 | FARIS shall run parameter studies of >= 10,000 cases without UI or memory growth | Throughput + memory | Dakota, Optuna | 10k cached-model cases (1 s each class) complete with RSS growth < 5% after warm-up; scheduler overhead < 50 ms/case (provisional) | Soak test with RSS sampling |
| HPC-10 | Study execution shall be fault-tolerant per case (failure isolation, retry policy) | Failure isolation | Snakemake, Nextflow | One failing case never aborts study; failures listed with cause; retry N configurable | Fault injection |
| HPC-11 | Job state shall be observable (queued/running/done/failed, ETA) via CLI/API/UI | Status API | Slurm squeue, Dask dashboard | Status within 1 s of change; machine-readable | Test |
| HPC-12 | Cloud bursting shall be possible via a documented backend using only OCI + object store | Documented, tested on 1 provider (provisional) | | Example runbook tested once per release; no vendor-specific code in core | Manual+scripted run |
| HPC-13 | Memory use shall be capped and reported per job (cgroup-aware) | RSS cap | | Job exceeding cap killed with clear error; core never OOMs the host | cgroup test |
| HPC-14 | Concurrency control: configurable max concurrent heavy jobs, default conservative | Limit enforced | | Default 1 transport job concurrent; never exceeds configured | Test |
| HPC-15 | Wall-time/CPU-time of every run recorded in provenance | Recorded | | 100% runs | Test |
| HPC-16 | Network-less (air-gapped) operation, with offline data bundles | No network calls | | 0 outbound connections by default (verified by netns test) | Network-namespace test |

### 2.6 Workflow, provenance, reproducibility
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| PRV-1 | Every result shall carry a provenance record: inputs hash, code version (git hash), tool versions, nuclear data hash, seed, host/arch, timestamps | Field completeness | AiiDA; PROV | 100% fields present on every stored result; absent = result marked not-evaluated | Schema + test |
| PRV-2 | Provenance shall be exportable as RO-Crate 1.2+ (target 1.3) (with Workflow Run profile) and PROV-JSON | Validity | RO-Crate 1.1 | Export validates with ro-crate-py validator; PROV-JSON loads in `prov` library | Validator in CI |
| PRV-3 | Provenance shall be a queryable graph (results <- runs <- inputs <- upstream results) | Query | AiiDA QueryBuilder | `faris why <result>` lists complete ancestry in < 1 s for 10k-node graph (provisional) | Benchmark |
| PRV-4 | The log shall be append-only and hash-chained; tampering detectable | Detection | CT logs RFC 6962 | 100% of injected modifications detected by `faris verify` | Tamper test |
| PRV-5 | Re-running a study from its provenance shall reproduce the result | Reproduction | | Deterministic parts: bit-identical results; MC parts: bit-identical when same seed+decomposition on same arch; across arch/thread count: statistically consistent within 3 sigma and flagged "not bitwise" | Reproduction test on golden studies on >= 2 machines |
| PRV-6 | Bitwise reproducibility conditions shall be declared per computation | Declared | | Each calc has "determinism class" (bitwise / seed-bitwise / statistical) shown in provenance | Enumeration test |
| PRV-7 | Builds shall be reproducible and releases signed with SBOM | Reproducible build | reproducible-builds.org; SLSA | Two independent builds same hash; SBOM (CycloneDX/SPDX) published; SLSA L2+ (provisional L3) | Release CI |
| PRV-8 | Studies shall be citable: CITATION.cff, versioned DOI via Zenodo/DataCite for releases and exported datasets | Metadata | DataCite, Zenodo | CITATION.cff valid (cffconvert); each release has DOI; `faris export --crate` includes DOI-ready metadata | Validators |
| PRV-9 | Software shall satisfy FAIR4RS | Checklist | FAIR4RS | Documented self-assessment against all 4 principles (F,A,I,R) with each sub-item pass/fail; >= 90% pass at 1.0 (provisional) | Checklist review; howfairis-style scan |
| PRV-10 | Data outputs shall be FAIR: persistent id, rich metadata, standard formats, licence | Checklist | FAIR 15 sub-principles | All 15 assessed; exports carry licence field | Checklist |
| PRV-11 | FARIS shall interoperate with workflow engines (Snakemake/Nextflow/CWL) via CLI contract | Example | CWL 1.2 | One tested example per engine (provisional: Snakemake + CWL) | CI example |
| PRV-12 | Provenance shall record human-in-the-loop decisions (overrides, accepted warnings) with timestamp and identity | Recorded | 21 CFR 11 audit trail | 100% overrides logged; cannot be removed without detection | Test |
| PRV-13 | Receipts from Avila Core shall be linked to FARIS results by hash and verified on load | Verification | Core | Load re-verifies hash; mismatch -> result displayed as unverified | Test |
| PRV-14 | Time and randomness shall be injected, not ambient (clock, RNG seeds explicit) | Determinism | | 0 uses of ambient RNG/time in calculation crates (lint) | Static check |
| PRV-15 | Floating-point policy shall be declared (no fast-math; deterministic reductions or ordered summation) | Policy | | Same input -> same bits on same arch across 100 runs and across thread counts for non-MC calcs | Test |

### 2.7 Design workflows
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| DSN-1 | FARIS shall support DOE: full-factorial, LHS, Sobol/Halton QMC, user-supplied | Methods | Dakota, scipy.qmc | >= 4 methods; discrepancy (scipy) reported | Unit tests vs scipy reference values |
| DSN-2 | FARIS shall compute Sobol first/total-order indices with bootstrap CIs, and Morris screening | Accuracy | SALib; Saltelli 2010 | On Ishigami function (analytic S1~0.3139, ST~0.5576 for a=7,b=0.1 [U: recalled]) error < 0.02 at N=2^14 (provisional) | Analytic benchmark test |
| DSN-3 | Sensitivity results shall report CI and warn when CI width exceeds threshold | CI | | 95% bootstrap CI always shown; warning when CI > 0.1 | Test |
| DSN-4 | FARIS shall propagate input uncertainty (MC/QMC/PCE) to outputs with quantiles | UQ | Dakota, OpenTURNS | Matches analytic test cases to within sampling error; reports 5/50/95% | Test |
| DSN-5 | FARIS shall include single- and multi-objective optimisers with constraints | Optimisers | pymoo, Optuna, PROCESS VMCON | >= 1 gradient-free single-objective, 1 multi-objective (NSGA-II type), 1 constrained local; converge on standard suites (e.g., ZDT1 hypervolume within 1% of reference at 10k evals - provisional) | Benchmark tests |
| DSN-6 | Optimisers shall handle noisy objectives (MC TBR with standard error) | Noise-aware | BoTorch noisy GP | Uses reported sigma; refuses to claim improvement < 2 sigma | Test with injected noise |
| DSN-7 | Pareto fronts shall be visualised and exportable, with dominated/feasible status and hypervolume | Output | pymoo | Front export CSV + plot; hypervolume with reference point recorded | Test |
| DSN-8 | Surrogates shall report held-out error and refuse use outside validated range | Error control | SMT; GP variance | Report CV RMSE and Q2; extrapolation flagged; surrogate-based optimum must be re-verified by true model before being labelled calculated | Test |
| DSN-9 | Optimisation results shall be tagged with which evaluations used surrogate vs full model | Tagging | | 100% | Test |
| DSN-10 | Constraints (e.g., TBR >= 1.05, stress limit, fluence limit) shall be first-class with margin display | Constraint handling | PROCESS constraint equations | Constraint violation shown with margin and uncertainty; feasibility uses lower bound (mean - 2 sigma) option | Test |
| DSN-11 | Studies shall be pausable/resumable and extendable (add points) without redoing cached ones | Reuse | Optuna storage; Dakota restart | 100% prior points reused; adding N points runs exactly N | Test |
| DSN-12 | FARIS shall expose objective/constraint evaluation to external optimisers (Dakota/pymoo/Optuna/OpenMDAO) through API | Integration | | Tested example for >= 2 external frameworks (provisional: pymoo, Optuna) | CI examples |
| DSN-13 | Sweeps shall display convergence (running mean +/- SE) to show whether N is enough | Convergence | | Shown for all MC outputs | UI test |
| DSN-14 | Multi-fidelity: cheap model screening then high-fidelity confirm | Workflow | | Documented workflow with promoted candidates verified at full histories | Example test |
| DSN-15 | Comparison of alternatives shall use paired/common seeds optionally and flag differences < 2 sigma | Paired stats | | Existing 2-sigma flag extended to sweeps; sweep-wide multiple-comparison note | Test |

### 2.8 UI configurability
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| UIC-1 | Settings shall be a schema-validated text file with per-user and per-project scope | Schema | VS Code settings.json | 100% settings have schema, default, description; unknown keys warned | Test |
| UIC-2 | Units preferences (SI/practical: MW, GWd, keV, cm vs m, per-quantity override) | Coverage | Blender Scene Units; FreeCAD unit schemas | 100% displayed quantities honour unit prefs; internal storage SI unchanged; round-trip test | UI snapshot tests across unit sets |
| UIC-3 | Keybindings shall be user-editable, conflict-detected, exportable | Rebindable | VS Code keybindings.json; Blender keymap editor | 100% commands rebindable; conflicts flagged; presets (default + >= 1 alt) | Test |
| UIC-4 | Layout/workspace save, restore, reset | Persistence | Blender workspaces | Restore exact layout across restarts; factory reset works; layout shareable as file | Test |
| UIC-5 | Themes: light/dark/high-contrast, user themes as files | Themes | VS Code | >= 3 built-in; contrast checks WCAG 2.2 AA for text (4.5:1) in all (see accessibility report) | Automated contrast check |
| UIC-6 | Preferences shall be portable (export/import single file; sync optional) | Portability | VS Code Settings Sync | Export/import round-trip lossless; no secrets in export | Test |
| UIC-7 | Font/UI scale user-configurable 50-300% (provisional: egui scale ability) | Scale | | Layout does not clip at 200% on 1366x768 | UI snapshot |
| UIC-8 | Locale: decimal separator/number formats configurable; internal files always locale-independent | Locale | | Files parse identically under any locale (test with de_DE, ja_JP) | Test |
| UIC-9 | Command palette with fuzzy search for all commands | Coverage | VS Code | 100% commands reachable; search latency < 50 ms | Test |
| UIC-10 | CLI/env/config precedence documented and tested (same as CFG-13) | | | | |
| UIC-11 | Configurable defaults for run budget (histories, memory cap, concurrency) with safe built-ins | | | Defaults conservative; changes logged | Test |

### 2.9 Versioning / collaboration
| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| COL-1 | Studies shall diff semantically (parameter-level, with units) via CLI and UI | Diff | nbdime; Onshape compare | `faris diff a b` lists every changed parameter with old/new/unit; 0 false "changed" on re-save | Test |
| COL-2 | Studies shall three-way merge for non-conflicting parameter changes | Merge | git merge; nbdime | Auto-merge independent edits; conflicts reported with both values; registered git merge driver | Merge test corpus >= 30 cases |
| COL-3 | Project container shall be git-friendly: text manifests + content-addressed blobs, stable ordering | Git friendliness | | Single-parameter edit changes <= 1 text line and <= 1 blob; no timestamps in canonical text | Test |
| COL-4 | Branching of scenarios (named variants, base reference) with cheap copy | Branching | Onshape branches | Branch creation O(1) in data size (shares blobs); branch from any saved version | Test |
| COL-5 | Full version history with named milestones and restore | History | Onshape | Restore any version in < 2 s for 1 GB project (provisional) | Benchmark |
| COL-6 | Audit trail: who/what/when for every change, tamper-evident | Audit | 21 CFR 11; PRV-4 | 100% mutations logged; chain verifies | Test |
| COL-7 | Share a study as a single self-contained file that reproduces on another machine | Portability | RO-Crate | Opens on a clean machine with only FARIS + declared adapters; missing adapter reported precisely | Clean-VM test |
| COL-8 | Comments/annotations on parameters and results stored in project | Annotation | | Persist, diffable | Test |
| COL-9 | Concurrent edit safety: lock file or merge, never silent overwrite | Safety | | 0 lost updates under 2-process write race (test) | Race test |
| COL-10 | Project size/compression: report and cap | | Existing zstd | Ratio reported; open time for 1 GB project < 3 s (provisional) | Benchmark |
| COL-11 | Redaction/export for sharing without proprietary data (e.g., licensed libraries) | Redaction | | Export flag strips named blobs, leaves hash references | Test |

## 3. Traps
1. "100% API coverage" claims: counting functions is not parity. Parity must be measured on actions/state transitions (UI command registry vs API), and also on read access to every displayed number.
2. Semver on 0.x: OpenMC, FreeCAD, and many science tools are 0.x and break freely; "we follow semver" means nothing pre-1.0. Also, file-format versions and API versions drift separately; need separate contracts.
3. Plugin systems with no sandbox (VS Code) are a supply-chain risk; a plugin ecosystem is a security surface, and for a tool whose outputs support design decisions, a plugin altering numbers is a provenance hole.
4. Units: "unit support" via display-only labels is not unit checking. Temperature offsets (degC vs K), logarithmic units, and per-quantity (barn, dpa, MWd/kg) custom units are classic failures. Also integer-valued "percent" vs fraction.
5. YAML implicit typing, float formatting churn, locale decimals and map-ordering cause false diffs and silent changes; canonical form must be enforced, not hoped for.
6. Auto-migration that silently changes physics defaults (a new default for a parameter) must change the result hash and be reported; otherwise reproducibility is fake.
7. Caching traps: key omits nuclear data library, tool version, thread/MPI layout, environment variable, or seed -> stale or non-reproducible hits. Cache hit rate is a vanity metric; false-hit rate is the one to measure.
8. Bitwise reproducibility of Monte Carlo is conditional (OpenMC documents processor-count independence of fission-site sampling [V], but not cross-compiler/arch bitwise equality) (decomposition, compiler, FMA, library versions). Claiming "reproducible" without a determinism class is misleading. OpenMC's seed handling should be verified before FARIS promises it.
9. Provenance as decoration: exporting RO-Crate that nobody can replay is compliance theatre. Metric = replay success, not file presence. Likewise "FAIR" self-scores are subjective; use the sub-principle checklist with evidence.
10. IMAS: the data dictionary changes between major versions and neutronics coverage is thin; claiming "IMAS compatible" without naming DD version and IDS subset is empty. Importing STEP does not mean geometry is neutronics-ready (watertight, overlapping volumes, tolerance).
11. Throughput numbers: cases/hour depend on cache hits and cost per transport run; benchmark with cold cache and report both. Scheduler overhead matters only for cheap cases.
12. Surrogate "accuracy" measured on training points (or on points from same LHS) is meaningless; need held-out error and extrapolation flags; optimisers exploit surrogate errors, so verify optima on the true model.
13. Optimising noisy MC outputs without using standard errors yields "improvements" that are noise (winner's curse). Hypervolume comparisons need a fixed reference point.
14. Sobol indices with correlated inputs, or small N, are not valid/stable; report CIs and refuse interpretation when CI width is large.
15. Configurability creep: every option multiplies the test matrix; scenarios that cannot be validated (illegal combinations) should be rejected by schema rather than documented as caveats.
16. Settings sync/portable preferences can leak paths or secrets; exclude them.
17. Cloud/HPC support claims without a tested backend rot; "tested in CI at least on containerised Slurm" is the minimum credible bar.
18. Number sourcing in this document: I did not re-fetch pages in this pass; verify the (unverified)-marked items (DD version dates, PROCESS equation counts, OpenMC restart semantics, Slurm MaxArraySize defaults) before turning any into a hard requirement.
