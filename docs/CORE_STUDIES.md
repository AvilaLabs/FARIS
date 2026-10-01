# Generated Avila Core studies

FARIS generates a contract and registry from the selected analyses and calls an
external Avila Core executable. The native **Compile study** button runs this
operation on a cancellable worker. The **Powered by Avila Core** attribution
accompanies an actual worker attempt, rather than a simulated progress indicator.

The compiler currently uses `avila.core/semantic/0.2-draft`. The locally checked
Core executable identifies itself as version 0.1.0; FARIS records the exact
executable, contract, and registry SHA-256 identities for each compilation.
No Core code is bundled in FARIS. Select it with `--core`, the desktop compiler
settings, or `FARIS_CORE_EXECUTABLE` for the desktop.

```bash
cargo run -p faris-app -- --core /path/to/avila-core

cargo run -- study generate --scenario scenarios/arc-inspired/scenario.json \
  --variant reference --analysis breeding,shielding --output runs/study-001
cargo run -- study compile --scenario scenarios/arc-inspired/scenario.json \
  --study runs/study-001/study.json --core /path/to/avila-core \
  --output runs/compiled-001
```

Both commands refuse an existing output directory. Compilation regenerates the
study from the exact scenario and selections and rejects modified study JSON.
The output preserves contract, registry, study, raw compiler logs, process status,
and parsed compiler report. Rejected declarations remain reviewable records.
The compile command exits 0 for compiled, 1 for a compiler rejection, and 2 for
execution, input, or report errors.

Breeding and shielding require transport and normalization. Fuel history adds
the operating-history stage; electricity adds history and net-energy accounting.
The planner declares those dependencies even while their adapters are absent.
Compilation does not assert that those stages are executable. The scientific
qualification policy is required, so a declared metric cannot acquire a
qualified scientific verdict just because its contract compiles.

The breeding criterion is a user-authored research threshold. Its bounded basis
requests 95% coverage; it does not construct such a bound from a Monte Carlo
standard error. Missing qualification and missing uncertainty support must be
resolved separately before any requirement assessment.

Verification used the actual compiler: the default breeding/shielding study
compiled, and a deliberately malformed exact-number limit was rejected with
`CORE-S1102` at `/requirements/0/limit/value`. These checks establish compiler
integration only. No reactor prediction, stage execution receipt, or scientific
qualification is supplied by compilation. Evidence execution and qualification
binding remain later demo work.

Shielding declares a magnet-region mean-neutron-flux output with units
`neutrons/m²/s`. It has no invented acceptance threshold. The current Core
profile requires at least one requirement: a shielding-only study is therefore
rejected with `CORE-S1102` at `/requirements`. FARIS preserves that diagnostic.
An analysis may still run independently through the transport workflow.

Compiler hashes are checked before and after execution. A changed executable
invalidates the report; unavailable execution retains an error receipt when
the output directory can be written. These are local consistency checks, not
signed attestation or a guarantee against adversarial file replacement.
