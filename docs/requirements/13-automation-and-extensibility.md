# Automation and extensibility

How FARIS is driven without a window and extended without recompiling: command layer, command line, Python, headless batch, macro recording, plugins and adapters, job queue and scheduler, content-addressed caching, resume, service mode, event hooks and a builder for locked-down apps. The principle is that the desktop is one client of a command layer that every other client also uses, so parity is structural, not a checklist. Timings are on the reference laptop (RL) unless stated; see [the index](README.md#reference-hardware-and-models). Performance numbers for parallel studies live in PERF-034 and are not restated here.

## Command layer and parity

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-001 | FARIS shall route every user action through one command layer that the desktop, CLI, Python and service all call. | 100 % of desktop actions map to a registered command id; CI fails if a UI action has no id. | Enumeration of the UI action table against the command registry. | Blender UI is built on bpy; ParaView Python Trace [U] | F1 | Partial: `faris-model` and `faris-engine` own semantics and the CLI and desktop are clients; no command registry |
| AUTO-002 | The CLI shall cover 100 % of non-view operations in the desktop. | 0 gaps in the parity matrix; the matrix lists each operation with its CLI command and test. Today's known gap, study export (PDF brief, CSV, SVG and PNG charts), closes first. | Registry versus CLI enumeration test; export golden-file test. | FARIS choice; research target of 100 % parity [R4, R6] | F1 | Partial: CLI covers history, evidence, transport, study, study-file, reactor and control; `faris export` writes only a geometry manifest, no study export |
| AUTO-003 | Parity shall be measured on actions and on reads, not on function counts. | Every number displayed in the UI is readable through the API by the same name; a lint fails a displayed value with no API accessor. | UI value table versus API accessor table. | Trap: counting functions is not parity [R4] | F1 | No |
| AUTO-004 | Every CLI command shall give machine-readable output and stable exit codes. | `--json` on 100 % of commands; exit codes documented: 0 ok, one distinct non-zero per failure class (usage, validation, rejected evidence, solver failure, cancelled, resource refused). | Exit-code table test; JSON schema validation of outputs. | OpenMC and Dakota CLI practice [U] | F1 | Partial: JSON output on most commands; exit codes 0, 1 (rejected) and 2 (other) only |
| AUTO-005 | Long CLI operations shall report progress and honour cancellation. | Progress lines on stderr (or JSON events with `--json`); SIGINT stops the job and frees resources in ≤ 2 s, matching PERF-024; 0 orphan processes. | Kill test across commands. | PERF-024 | F1 | Partial: engine job runner cancels process groups; CLI signal handling unmeasured |
| AUTO-006 | Every API error shall have a stable code, a message and a remediation. | 100 % of errors have an id documented in the error reference; 0 panics reachable from the API in a 24-hour fuzz run per entry point. | Error-id docs check; cargo-fuzz. | FARIS choice | F1 | No |
| AUTO-007 | FARIS shall run without a display and without network. | 100 % of non-view operations pass in CI in a container with no display and no network namespace. | Headless CI job. | OpenMC CLI is headless [U]; HPC-1 and HPC-16 in research | F1 | Partial: CLI never opens a window; not tested in a no-display container |
| AUTO-008 | The command layer shall version itself. | A command-layer version string; clients negotiate it; backward compatibility for one major version, tested against the previous release's recorded call set. | Compatibility test with N-1 client. | Jupyter messaging protocol is versioned [U] | F2 | No |

## Python API

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-010 | FARIS shall ship Python bindings over the Rust core. The core stays authoritative; Python never reimplements physics. | One package installable offline from a wheel; 0 calculation code in Python (lint: no numerical solver in the package). | Wheel install test; code scan. | FARIS house rule (Rust first); PyO3 and maturin [U] | F6 | No |
| AUTO-011 | The Python API shall be fully typed. | `mypy --strict` and `pyright` clean on the shipped stubs; `py.typed` present; 100 % of public symbols annotated. | Type-check job on the package and on every docs example. | PEP 561; PyAnsys ships typed packages [U] | F6 | No |
| AUTO-012 | Every public Python item shall be documented with a runnable example. | 100 % documented; ≥ 90 % with an example; doctests pass. | `interrogate` and doctest run in CI. | docs.rs and Sphinx practice [U] | F6 | No |
| AUTO-013 | Results shall display richly in Jupyter. | Every public result type has `_repr_html_` and a plot; all documentation notebooks run headless in CI in ≤ 10 minutes. | nbmake run. | PyAnsys and OpenMC notebooks [U] | F6 | No |
| AUTO-014 | The Python API shall carry units and kind labels on every number. | 100 % of returned quantities are objects with value, unit, kind label and, where applicable, standard error; bare floats are not returned for dimensional results. | Type test over the API surface. | House rule: kind labels | F6 | No |
| AUTO-015 | Python shall cover 100 % of the command layer. | Generated from the command registry; parity test as AUTO-002; additions to the registry appear in Python in the same release. | Registry-to-Python generation diff. | Blender bpy [U] | F6 | No |
| AUTO-016 | Python import and first call shall be quick. | `import faris` ≤ 300 ms and opening a recorded study ≤ 1 s on RL (provisional: not measured; confirm before F6 gate). | Timing harness, 20 runs, P95. | FARIS choice | F6 | No |
| AUTO-017 | The Python package shall work with only declared dependencies and pin none of the user's. | Install into a clean virtual environment with ≤ 3 required runtime dependencies; tested on the 3 newest Python minor versions. | Matrix CI. | FARIS choice | F6 | No |

## Scripting and macro recording

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-020 | FARIS shall record desktop sessions as a script in the CLI or Python form. | 100 % of recordable commands recorded; the recording opens as plain text with comments naming each step. | Record and read-back test on 20 golden sessions. | ParaView Python Trace, COMSOL "Save as Java", Fluent journals [U] | F6 | No |
| AUTO-021 | Replaying a recorded session shall reproduce the same study. | Replay on a clean profile gives the same study-file content hash on 20 of 20 golden sessions (deterministic parts); stochastic parts rerun with their recorded seeds and agree to the determinism class in PRV (see 16-evidence-and-provenance). | Golden-session replay in CI. | ParaView trace replay [U] | F6 | No |
| AUTO-022 | Recording shall not capture secrets or personal paths. | 0 account tokens or home paths in 100 recorded sessions (scanner). | Scanner test. | Security rule; see 19-security | F6 | No |
| AUTO-023 | Scripts shall declare the command-layer version they target and be refused when it is newer. | 100 % of scripts carry the version; a newer-than-supported script is refused with the version named. | Version-gate test. | Fail-closed house rule | F6 | No |
| AUTO-024 | A script shall run in dry-run mode that validates every step without computing. | Dry run of a 100-step script reports all errors in ≤ 5 s on RL and starts 0 jobs. | Dry-run test. | FARIS choice | F6 | Partial: `faris validate` and `study compile` check inputs without running solvers; no script dry run |
| AUTO-025 | Scripts shall be able to be saved as study templates and shared. | A recorded script becomes a template with named parameters in ≤ 5 steps; template runs on a clean install. | Template round-trip test. | FARIS choice; see DSN-090 | F6 | No |

## Content-addressed caching

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-030 | FARIS shall cache every expensive result under a key made from a canonical hash of its inputs. | Cache hit returns in ≤ 1 s with 0 transport runs (PERF-027); result stored immutably under its key. | Re-run test. | Nix and Bazel content addressing [U]; PERF-027 | F1 | Partial: recorded bundles reused by hash in the .faris file; no general cache |
| AUTO-031 | The cache key shall be computed by one function that is the only place a key is built. | 1 key function; lint fails any second key construction. | Static scan. | FARIS choice | F1 | No |
| AUTO-032 | Every cache entry shall record its full key components, readable by a person. | `faris cache explain <key>` lists each component with its value or hash; 100 % of entries. | CLI test. | FARIS choice | F1 | No |
| AUTO-033 | FARIS shall never serve a false cache hit: the key shall cover every input that can change a result. | Key components: FARIS code version, calculation-crate versions, nuclear and material data library hash, canonical inputs, seeds, adapter name and version, external tool version and build, result-changing settings (CFG-005), and platform-relevant facts (CPU architecture, thread and rank count where results depend on them). A mutation test changes each component one at a time and asserts a miss for every one: 0 false hits across ≥ 30 components and ≥ 3 values each. | Mutation test run on every release and on any change to key-building code; component list compared with the registry of result-changing settings. | Nix and Bazel key = hash of all inputs, tool version and environment [U]; Trap: cache hit rate is a vanity metric, false-hit rate is the one to measure [R4] | F1 | Partial: recorded bundles are verified by hash; no general cache, no mutation test |
| AUTO-034 | The key shall be built from canonical forms so that equal inputs hash equally. | Reordering keys, whitespace and unit-equivalent rewrites (m versus cm, with the same stored value) give the same key in 100 % of 200 pairs; 0 key changes from timestamps, paths or host names. | Property test. | Trap: false misses waste hours but are safe; false hits are not | F1 | No |
| AUTO-035 | A cache entry shall be verified on every read. | Entry bytes re-hashed on read; a corrupted entry is refused, reported and rebuilt; 100 % of 200 injected corruptions detected. | Corruption-injection test. | Fail-closed house rule | F1 | No |
| AUTO-036 | The cache shall be bounded and manageable. | `faris cache gc --max-size` evicts least-recently-used entries; pinned results are never evicted; size and hit counts shown in the UI and CLI. | GC test with pinned entries. | Nix gc [U] | F2 | No |
| AUTO-037 | The cache shall be shareable. | Local directory and shared file system at F6; optional S3-compatible remote later; integrity verified by hash on read for all backends. | Two-machine test; corruption injection. | Bazel remote cache [U] | F6 | No |
| AUTO-038 | FARIS shall report cache hit rate and false-hit rate separately. | Release notes give both; false-hit rate is 0 by AUTO-033; a hit whose stored provenance disagrees with the current key is counted as a false hit and blocks release. | Release report check. | Trap: hit rate alone is a vanity metric | F1 | No |
| AUTO-039 | Cached results shall show that they came from the cache, and from when. | 100 % of cache-served results carry the original run date, key and code version; the UI says "reused from <date>". | UI and export snapshot test. | House rule: fail closed, show provenance | F1 | No |

## Jobs, queue and scheduler

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-040 | FARIS shall run every external computation as a bounded, cancellable job. | 100 % of adapter runs go through the job runner with memory, file-size and time limits; cancellation terminates the whole process group. | Kill test across every adapter (see PERF-024). | Job runner exists in `faris-engine` (process group, prlimit) | F1 | Partial: `run_job` in crates/faris-engine/src/jobs.rs owns a process group, applies limits and cancels; adapter coverage beyond OpenMC unmeasured |
| AUTO-041 | FARIS shall queue jobs and admit them by memory and cores. | Default 1 concurrent transport job; a job projected beyond free memory minus margin is refused with an explanation (PERF-041, PERF-043). | Admission test; soak. | Laptop rule [internal] | F1 | Partial: one-at-a-time by convention |
| AUTO-042 | Job state shall be observable through UI, CLI and API. | States queued, running, cancelling, done, failed, cancelled; status change visible within 1 s; machine-readable. | State-transition test. | Slurm squeue, Dask dashboard [U] | F1 | Partial: UI shows run state; no queue |
| AUTO-043 | Jobs shall be prioritised and reorderable. | Interactive preview jobs run before batch jobs; a user can move, pause and cancel queued jobs; order change visible ≤ 100 ms. | Queue test. | FARIS choice | F2 | No |
| AUTO-044 | A failing case shall never abort a study. | 1 injected failure in a 100-case study leaves 99 results; failure listed with cause; retry count configurable, default 1. | Fault-injection test. | Snakemake and Nextflow practice [U] | F6 | No |
| AUTO-045 | FARIS shall submit work to Slurm and PBS through a backend interface. | Tested against a containerised Slurm cluster in CI; array jobs supported within the site array limit (Slurm default MaxArraySize 1001); larger studies are chunked automatically. | Integration test with chunking at 2,500 cases. | Slurm job array docs, default MaxArraySize 1001 [V]: [Slurm job arrays](https://slurm.schedmd.com/job_array.html) | F6 | No |
| AUTO-046 | A cluster run shall produce the same receipt hash as a local run for deterministic work. | 100 % identical receipt hashes on 10 deterministic cases; Monte Carlo cases agree within 3σ and are flagged not bitwise. | Paired local and cluster runs. | R6 sweep row; OpenMC parallel reproducibility is a stated design goal for fission sites [V]: [OpenMC parallelization](https://docs.openmc.org/en/stable/methods/parallelization.html); fixed-source scope [U] | F6 | No |
| AUTO-047 | FARIS shall ship an OCI image and an Apptainer definition. | Image builds twice to the same digest (provisional: needs pinned base image; confirm before F6 gate); runs rootless under Apptainer. | Build-twice test. | Apptainer and Docker for HPC [U] | F6 | No |
| AUTO-048 | Cloud offload shall be possible through OCI plus an object store with no vendor-specific code in the core. | One documented runbook run per release on one provider; 0 vendor SDK imports in core crates. | Runbook run; dependency scan. | FARIS choice (provisional: needs a test account; confirm before F6 gate) | F6 | No |
| AUTO-049 | Every job shall record wall time, CPU time, peak memory and host in its receipt. | 100 % of jobs. | Receipt schema test. | FARIS choice | F1 | Partial: wall time recorded for runs; memory and host not |

## Resume and determinism

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-050 | A killed study or sweep shall resume without redoing finished work. | After SIGKILL at a random point, resume runs only unfinished cases; finished results reused 100 %; lost work ≤ 1 checkpoint interval. | Kill-and-resume test, 50 random kill points. | Dakota restart, Optuna storage [U] | F1 | No |
| AUTO-051 | A resumed deterministic study shall be bit-identical to an uninterrupted one. | Result hashes equal on 20 of 20 kill points. | Paired runs. | FARIS choice; R6 sweep row | F1 | No |
| AUTO-052 | Long Monte Carlo runs shall checkpoint at batch granularity and resume from the checkpoint. | A killed run resumes from the last statepoint; final result agrees with an uninterrupted run within 3σ, and is bit-identical where the adapter promises it. | Kill test on RM-S. | OpenMC restart: `openmc -r statepoint.N.h5`, must match the model [V]: [OpenMC settings](https://docs.openmc.org/en/stable/usersguide/settings.html); seed and thread reproducibility [U] | F2 | No |
| AUTO-053 | Each computation shall declare its determinism class. | 100 % declare bitwise, seed-bitwise or statistical; the class is shown in the result record. | Enumeration test. | OpenMC documents processor-count independence only for fission-bank sampling [V]: [OpenMC parallelization](https://docs.openmc.org/en/stable/methods/parallelization.html); cross-platform bitwise equality not claimed anywhere found | F1 | No |
| AUTO-054 | Calculation crates shall take time and randomness as inputs. | 0 ambient clock or RNG calls in calculation crates (lint). | Static check. | FARIS choice | F1 | No |

## Adapter contract and plugins

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-060 | FARIS shall define a versioned adapter contract. | The contract specifies inputs, outputs, units, receipts, cancellation, failure modes and tool version range; contract version declared by each adapter. | Contract document and schema in the repo. | FUSE actors, bluemira codes interface [U] | F1 | Partial: OpenMC, ACTINV and Core adapters exist; no written common contract |
| AUTO-061 | A conformance suite shall run against every adapter. | ≥ 20 checks (units declared, receipt complete, cancel honoured, timeout honoured, bad input rejected, no write outside run directory) pass for every shipped adapter in CI. | Conformance CI job. | FARIS choice | F2 | No |
| AUTO-062 | Adapters shall refuse tool versions outside their declared range. | Out-of-range tool version blocks the run; an override is allowed only when recorded in the result. | Fake-version test. | VS Code `engines`, Blender minimum version [U] | F2 | No |
| AUTO-063 | Adapters shall run as separate processes with declared capabilities. | Default: no network, write only to the run directory; a violation aborts the job and logs; Linux enforced by namespaces or Landlock where available. | Negative tests (forbidden write, forbidden connection). | Blender extension permissions [U]; WASI [U] | F6 | Partial: separate process, resource limits; no capability enforcement. The job runner is not a sandbox for hostile programs (jobs.rs header) |
| AUTO-064 | An adapter crash or timeout shall not crash FARIS or corrupt a study. | 0 study corruptions in 1,000 injected crashes; configurable timeout with a default per adapter. | Fault-injection test. | VS Code extension host isolation [U] | F1 | No |
| AUTO-065 | Adapter outputs shall carry units, kind label, uncertainty and source or be rejected. | 100 % of adapter numbers have unit, kind label and source; a number without them is refused at the boundary. | Schema validation test. | House rule; AGENTS: record units and normalisation at adapter boundaries | F2 | Partial: transport bundles record units and normalisation; no boundary schema enforcement |
| AUTO-066 | Adapters shall be listed with version, hash and health. | `faris adapters list --json` and a UI panel; health check ≤ 5 s per adapter without running physics. | CLI test. | `code --list-extensions` style [U] | F1 | Partial: `faris doctor` reports PATH presence and `NOT_EVALUATED` status; no version or hash |
| AUTO-067 | A minimal adapter shall be writable from a template. | Template in ≤ 200 lines builds and passes the conformance suite in CI. | Template CI. | FARIS choice | F6 | No |
| AUTO-068 | Third parties shall load plugins without recompiling the app. | Out-of-process or WebAssembly component plugins; cold load ≤ 500 ms (provisional: not measured; confirm before F6 gate). | Plugin load benchmark. | VS Code extension host; wasmtime component model [U] | F6 | No |
| AUTO-069 | A plugin shall never be able to alter a number without it appearing in the record. | Plugin outputs pass the same boundary schema as adapters (AUTO-065); results from a plugin are labelled with plugin name, version and hash; 0 unlabelled plugin numbers. | Negative test with a plugin that rewrites a built-in value. | Trap: a plugin changing numbers is a provenance hole [R4] | F6 | No |

## Plugin API stability

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-070 | The public API, CLI, plugin API and file formats shall carry separate semantic versions. | 4 version strings published; a change to one does not force the others. | Release checklist test. | Trap: file-format and API versions drift separately [R4] | F1 | No |
| AUTO-071 | Breaking changes in the public Rust and Python API shall be detected automatically. | 0 undeclared breaking changes per release after 1.0; before 1.0, breaks only on a minor release with a changelog entry. | cargo-semver-checks and griffe in release CI. | cargo-semver-checks [U]; Trap: semver on 0.x means nothing [R4] | F6 | No |
| AUTO-072 | Deprecation shall precede removal. | At least 2 minor releases and 6 months; the warning names the replacement; 0 removals without an earlier warning. | Deprecation registry test. | PEP 387: warning for ≥ 2 minor versions, minimum ≥ 2 years, preferred 5 years [V]: [PEP 387](https://peps.python.org/pep-0387/). FARIS shortens the window and records that as a choice | F6 | No |
| AUTO-073 | The plugin API shall stay stable across all minor releases of one major version. | A reference plugin built against 1.0 passes the conformance suite on every later 1.x release (matrix N, N-1, N-2). | Compatibility matrix. | VS Code API versions [U] | F6 | No |
| AUTO-074 | Plugins shall be signed, and unsigned plugins shall need explicit opt-in. | Signature and hash shown before install; unsigned plugin refused until the user opts in per plugin; site policy can forbid opt-in (CFG-061). | Signed and unsigned install tests. | Sigstore, Blender extensions [U] | F6 | No |

## Service mode, hooks and events

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-080 | FARIS shall run as a local service with a REST interface described by OpenAPI. | `faris serve` binds to loopback by default; OpenAPI 3.1 document published per release; 100 % of command-layer operations reachable. | Contract test from the OpenAPI document. | SysML v2 API and Services v1.0 is REST with OpenAPI [U]; R6 interop row | F6 | No |
| AUTO-081 | The service shall be safe by default. | Loopback only unless configured; token required for every call; 0 unauthenticated state-changing endpoints; see 19-security. | Port scan and negative tests. | Security rule | F6 | No |
| AUTO-082 | The service shall report progress as a stream. | Server-sent events or WebSocket events for job state and progress within 1 s of change. | Stream test. | PERF-022 | F6 | No |
| AUTO-083 | The service shall keep results and the OpenAPI contract compatible across one major version. | Client from release N-1 passes the contract suite against release N. | Contract test. | Jupyter messaging protocol [U] | F6 | No |
| AUTO-084 | FARIS shall offer event hooks. | Hooks on study opened, job started, job finished, verdict changed, export written; a hook is a user-configured command with a timeout (default 10 s) and 0 ability to alter results. | Hook test with failing and slow hooks. | FARIS choice | F6 | No |
| AUTO-085 | A failing hook shall never block or change a run. | A hook that crashes or hangs leaves the run unchanged and logs the failure in 100 % of 100 injected cases. | Fault-injection test. | Fail-safe design | F6 | No |
| AUTO-086 | Every hook call shall be logged. | Hook name, command hash, exit code and time in the activity log. | Log test. | Audit rule | F6 | No |

## Applications for non-experts

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| AUTO-090 | An expert shall be able to publish a locked-down app from a study. | The builder selects sliders, results and explanatory text; a trained user builds an app from a finished study in ≤ 10 minutes (provisional: small sample; confirm before F6 gate). | Timed usability study with 5 trained users. | COMSOL Application Builder [U]: Introduction to Application Builder (cdn.comsol.com) | F6 | No |
| AUTO-091 | An app shall not be able to alter hidden inputs. | 0 changes to unexposed inputs in a 200-case tamper test over UI, API and file edits; app file carries a hash of the locked inputs. | Tamper test. | Fail-closed house rule | F6 | No |
| AUTO-092 | An app shall keep the full evidence trail. | Every app result names the study hash, exposed inputs and kind labels; exports from an app are stamped with the app hash. | Export snapshot test. | House rules | F6 | No |
| AUTO-093 | An app shall run offline from one file. | App file opens on a clean machine with FARIS installed and declared adapters; missing adapters reported precisely. | Clean-machine test. | FARIS choice | F6 | No |
| AUTO-094 | An app shall refuse to show a verdict outside the validated range of its exposed inputs. | Slider ranges equal the validated range; out-of-range requests blocked. | Range test. | Fail-closed house rule | F6 | No |

## Traps

- "100 % API coverage" counted in functions is not parity. Parity is actions and displayed values (AUTO-001, AUTO-003).
- Cache hit rate looks good and hides stale results. Measure false hits, which must be 0 (AUTO-033, AUTO-038).
- A key that omits the data library, the adapter version, the thread or rank layout, or one environment variable fails silently. The mutation test is the only proof.
- Semantic versioning before 1.0 promises nothing, and API, CLI, plugin and file versions drift. Four separate versions with automated break detection is the honest alternative (AUTO-070, AUTO-071).
- A plugin ecosystem is a security surface and a provenance hole. Unsigned, unlabelled or unsandboxed plugin numbers must not reach a verdict.
- "Bit-identical" without a determinism class is misleading for Monte Carlo. OpenMC documents processor-count independence for fission-bank sampling only; do not promise more without testing.
- Cluster and cloud claims without a tested backend rot. Containerised Slurm in CI is the minimum credible bar.
- A locked-down app is only locked if hidden inputs are tamper-checked, not just hidden in the interface.
