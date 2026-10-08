# Avila Core study integration

FARIS generates and compiles a reusable study contract, then can execute four
coarse Rust stages through Core's controlled runner. The native upper-right
**Compile study** button invokes the real compiler. The separate **Run bound
study stages** control prepares a case, executes it, and retains Core's actual
receipts, artifact checks, claims replay and scoped requirement verdicts.

The checked implementation is Avila Core 0.1.0 with
`avila.core/semantic/0.2-draft`; recorded runs pin executable bytes, declarations
and the compiled snapshot. Core is an external tool. The Rust simulation engine
and ordinary transport/history CLI remain independently usable.

## Stage boundary

| Stage | Actual operation | Bound inputs |
| --- | --- | --- |
| transport | Revalidate a previously executed OpenMC record and its normalization inputs | Exact scenario, physics, nuclear-data audit and portable recorded transport |
| normalize | Extract the declared responses and the driving rates from the transport stage's normalized result | Verified transport stage output and scenario |
| history | Run the deterministic fuel/decay/processing/exposure/maintenance ledger | Normalize stage output (driving rates), scenario and explicit operating assumptions |
| energy | Replay that history and check its signed electrical ledger | Recorded history and scenario |

The transport stage verifies prior OpenMC execution; it does not rerun OpenMC
under Core. **Run fresh transport** remains an explicit FARIS operation with
installed solver and nuclear data. Recorded transport includes raw tallies,
spectra, worker source, input/audit bytes and local execution records; nuclear
data and OpenMC statepoints remain external identified artifacts.

Stage outputs (`faris-core-stage-output/v0.2`) carry only what the stage newly
computes. The transport output holds the normalized result as a compact JSON
string (`normalized_json`); the normalize output holds the extracted claims and
the driving rates (`rates_json`); the history output holds the history ledger.
No stage forwards the recorded transport bundle: Core binds every stage input
by SHA-256 to the upstream step's receipt-verified output, so a downstream
stage checks only that the upstream envelope has the expected version and stage
and names the bound scenario. Saved `faris-core-stage-output/v0.1` outputs
(which forward the full bundle) are still read and inspected; new cases are
written as v0.2.

Core's verified execution means the declared workflow completed. A compiled
contract or verified receipt does not qualify reactor physics. The physical
breeding requirement requires scientific qualification and a supported bounded
claim; the current nominal response remains **NOT_EVALUATED**. Monte Carlo
standard errors are retained in Rust artifacts and are not silently converted
into confidence bounds. Fixture PASS/FAIL/INCONCLUSIVE states test software
semantics and are never presented as reactor verdicts.

## CLI use

```bash
cargo build -p faris-cli --locked --jobs 1
target/debug/faris study generate \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --variant reference --analysis breeding,shielding,fuel-history,electricity \
  --output runs/full-study
target/debug/faris evidence prepare --run /path/to/verified/run.json \
  --study runs/full-study/study.json \
  --assumptions scenarios/arc-inspired/demo-operating-assumptions.json \
  --core /path/to/avila-core --output runs/full-case
target/debug/faris evidence run --case runs/full-case \
  --core /path/to/avila-core --workspace runs/full-execution \
  --output runs/full-evidence.json
```

Output directories must be fresh. The builder computes expected outputs through
the shared Rust engine, and Core's bound CLI recomputes them. All stage IDs,
compiled identity, receipts, successful execution, artifact reproduction and
exact claims replay are required for FARIS to report a completed workflow.
Interrupted/rejected work preserves diagnostics. A new CLI build requires a
newly prepared case because its executable digest changes.

## Numeric policy and packaging

Authoritative Rust numerical payloads travel as exact JSON strings, with
round-trip f64 parsing. Extracted Core quantities use shortest round-trip decimal
strings expanded to canonical decimal notation. This preserves numerical
meaning; it does not assert that a calculated physical quantity is exact.
Core treats these nominal quantities as unquantified. Display rounding is not
used in generation, comparisons or criteria.

Raw tally files remain bounded at 16 MiB, recorded transport at 32 MiB of file
content, and history stage envelopes at 64 MiB. Linux jobs enforce inherited
address-space/per-file limits plus monitored aggregate artifact limits, wall
time, captured logs and owned process-group cancellation. These bounds are for
trusted scientific tools and are not a hostile-executable sandbox.

The packaging script generates the same four-stage family for both arrangements
and matched controls. Ordinary use needs no handwritten Core contract or
per-component declaration. The maintained integration is a reusable template,
one adapter builder and exact boundary checks; upgrades must recheck the pinned
Core profile. See [the acceptance record](../../docs/DEMO_ACCEPTANCE.md) for the
current verification and remaining scientific/release evidence.

The small **Powered by Avila Core** attribution accompanies the actual compile
experience, with a version/profile tooltip and a reduced-motion setting. It
attributes the compiler and does not certify the reactor.
