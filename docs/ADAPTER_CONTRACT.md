<!-- kind: reference -->
# Adapter contract

Version `faris-adapter-contract/v1` (requirement AUTO-060). An adapter is the code that lets FARIS drive one external
tool: OpenMC, ACTINV or Avila Core. This page says what every adapter must specify and what FARIS requires of it. The
machine-readable form is [`schemas/faris-adapter-1.schema.json`](schemas/faris-adapter-1.schema.json), and each adapter
declares itself in `integrations/adapters/<name>.json`. `scripts/test_adapter_contract.py` validates every declaration
against the schema and against this page, in CI.

The contract describes how the three adapters work today. FARIS has no plugin system; a new adapter is a change to
FARIS, not an installed extension.

## What a declaration contains

| Field | Meaning |
| --- | --- |
| `contract` | `faris-adapter-contract/v1`. A declaration for another contract version is refused until the adapter is moved to it. |
| `name` | The tool's name as `faris adapters list` shows it: `openmc`, `actinv` or `avila-core`. |
| `tool_versions` | `tested` lists the versions the adapter was run against. `accepted` is the version range the adapter is meant for (for example `>=0.15.3,<0.16`). `check` says what FARIS does with a tool outside that range: `refuses` (the run does not start, and the error names the version) or `records_only` (the run goes ahead and the receipt keeps the version, so the reader can see it). |
| `inputs` | Each input the adapter hands the tool: a name, the file or argument that carries it, its format, and its unit or `none`. |
| `outputs` | Each output the adapter reads back: name, format, unit or `none`, and the label it carries in FARIS (`calculated`, `checked`, and so on, as in [Reading the numbers](guide/results.md)). |
| `units` | The unit system at the boundary, and where conversions happen. A quantity enters FARIS only in the unit and normalisation this field states. |
| `receipt` | What is recorded about a run so that it can be checked later (see below). |
| `cancellation` | How a running tool is stopped (see below). |
| `failures` | The failure modes, each with the status FARIS reports and what the reader does next. |
| `entry_points` | The Rust modules and scripts that implement the adapter, so a reader can find the code. Each path must exist. |

## What FARIS requires

**Inputs.** Every input is written by FARIS from the scenario and its assumptions. The adapter does not read the user's
environment to decide a result. Inputs that name a file carry the file's SHA-256 in the receipt.

**Outputs and units.** Output values cross the boundary once, in the unit in `units`, and keep their normalisation (per
source particle, per cm³, per second; the field says which). A value the tool did not produce is not filled in; the
result is `NOT_EVALUATED` with the reason.

**Receipts.** A run is described by: the tool's path and SHA-256 and the version it reports; the SHA-256 of every input
file; the arguments and the working limits; the exit status and execution status; the elapsed time; and the SHA-256 of
every output file. Input hashes establish identity, not scientific correctness. The receipt is stored with the result and
is part of the cache key for the result.

**Bounded execution.** Every run goes through the job runner (`faris-engine::jobs`). The program is an absolute path. The
runner enforces a wall-time limit, bounded output capture, an address-space limit, a single-file limit and a limit on
the files and bytes the job adds, and it records the peak memory.

**Cancellation.** The caller can cancel at any moment. The job owns a process group (a job object on Windows), and
cancellation ends the whole group, solver children included. The execution status is then `Cancelled`, and a cancelled
run is never stored as a result.

**Failure modes.** The job runner reports an execution status: the tool succeeded, the tool exited with an error, the job was
cancelled, or it passed the wall-time, output, artifact, file-size or memory limit. Success of the process is not success of the science: an adapter that
sees a clean exit still checks the output it reads (its schema, its tally identities, its normalisation) and reports a
failure of that check as a failure of the adapter, with the file and field named.

**Versions.** An adapter whose `check` is `refuses` refuses a tool version outside `accepted`. One whose `check` is
`records_only` does not, and its declaration says so; moving it to `refuses` is open work. When the version cannot be
read, the health shown by `faris adapters list` is never `OK`: it shows the probe's error or that the probe was not run.

## Changing the contract

A change to a field's meaning or a new required field is a new contract version (`faris-adapter-contract/v2`), with a
migration note in the changelog. A new optional field keeps the version. Each declaration names the version it follows,
so adapters can move one at a time.
