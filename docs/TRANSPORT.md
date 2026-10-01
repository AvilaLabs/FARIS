# Fixed-source transport boundary

Status: strict input/artifact types and checked Rust normalization are implemented.
There is no ARC-inspired reactor solver adapter yet. Material assignments,
source prescription, penetration, spectra/mesh mappings and physical validation
remain prerequisites in [SCIENTIFIC_BASELINE.md](SCIENTIFIC_BASELINE.md).

## Identity and scope

`faris-model::transport::TransportRequest` defines exact scenario bytes by
SHA-256, an existing arrangement/component identity, nominal fusion power,
the authored D-T source and response definitions. Unknown JSON fields and
unsupported schema versions reject. A source definition records the total
reaction energy in eV, emitted neutron energy, spatial/angular/energy
distribution identity, and exactly one source neutron per D-T reaction.
Positive finite values and energy accounting are necessary checks; they do
not validate the plasma/source prescription.

`faris-engine::transport::TransportArtifact` echoes the complete request and
provides solver/data identities, nominal histories, explicit positive domain
volumes, score-specific per-source raw values and one Monte Carlo standard
error. Every requested response needs exactly one tally and its domain volume.
Missing/duplicate responses, unused volumes, invalid SHA-256 identity syntax,
negative/nonfinite means or standard errors, and inconsistent units reject.
Reaction events and produced particles are separate score types. Tritium
production must use `particle_production` with `particle: "tritium"` and
`score: "H3-production"`; it cannot masquerade as a reaction-event score.

Imported identities and raw values are supplied by an adapter. Checking a hash
string's shape is not authenticating its claimed contents. The import records
hashes of the exact request and artifact bytes; it does not execute, reproduce
or authenticate the asserted solver run. Scientific qualification remains
`NOT_EVALUATED`. Only a subsequent owned adapter and independently reviewed
evidence can bind an actual solver result to these records.

## Dimensional normalization

With fusion power `P_f` in MW, declared reaction energy `Q` in eV, and the exact
SI elementary charge `e = 1.602176634 × 10⁻¹⁹ J/eV`, Rust calculates
`R_f = P_f × 10⁶ / (Q × e)` reactions/s and `S_n = R_f` neutrons/s.
Fusion power is total reaction power; it is not neutron power. For example,
14.1 MeV neutrons and a 17.6 MeV reaction convention allocate only `14.1/17.6`
of the fusion power to primary neutrons. Those rounded energies must be
declared approximations in a production study.

OpenMC's raw fixed-source scores are per simulated source particle. The initial
boundary requires unit source strength and one primary neutron per history;
an adapter must not multiply by source strength and then label the result
per-source. Importance weighting, source biasing and particle filters need
explicitly verified adapter treatment before production use.

For raw mean `x`, standard error `u`, and domain volume `V` in m³:

| Score | Raw unit | Integrated conversion | Volume average |
| --- | --- | --- | --- |
| Neutron flux | cm/source neutron | `x × 0.01 × S_n`, neutron-m/s | Divide by `V`, neutrons/(m² s) |
| Reaction rate | reactions/source neutron | `x × S_n`, reactions/s | Divide by `V`, reactions/(m³ s) |
| Particle production | particles/source neutron | `x × S_n`, particles/s | Divide by `V`, particles/(m³ s) |
| Heating or heating-local | eV/source neutron | `x × e × S_n`, W | Divide by `V`, W/m³ |

The same positive conversion factor applies to `u`. Volumes may arrive in
cm³ and are converted by `10⁻⁶`. Integrated and averaged means/errors remain
available with explicit units, domain, source and estimator. Nonfinite or
vanishing source normalization rejects. The output does not convert standard
error into a confidence interval or include material/data/source uncertainty.
Correlated sums and differences require covariance; summing standard errors
in quadrature without defensible independence is unsupported.

Heating conventions have different physical assumptions: `heating-local`
assumes secondary-photon energy is locally deposited, while coupled neutron/
photon `heating` needs the appropriate photon/data treatment. Recording a
score name alone does not establish that its nuclear data or transport mode
were configured correctly. Heating is an energy-deposition rate, not a
temperature, and neutron heating alone is not whole-plant recoverable heat.

These dimensions follow the [OpenMC 0.15.3 tally specification](https://github.com/openmc-dev/openmc/blob/v0.15.3/docs/source/usersguide/tallies.rst).

## Execution and evidence

`faris-engine::jobs` runs explicit executables with selected environment
variables, null stdin, a wall-time deadline, bounded stdout/stderr and a
cancellation token. Unix jobs own a process group; timeout/cancellation stops
ordinary solver descendants and closes their output pipes. Solver memory,
disk, escaped process groups and hostile executables are outside this runner's
containment. Call it on a worker when integrating the desktop.

The CLI's `control absorber` is the first owned execution client. Its reviewed
Python worker is embedded in the Rust binary and copied into each new run,
so execution preserves that build's exact control source. It caps work
at ten million histories, uses one thread, reserves a new evidence directory,
fingerprints the script/interpreter/executable, writes `request.json` before
launch, checks the completed result's reported bindings, and writes `execution.json`
alongside raw OpenMC inputs/statepoint and a scoped control result. Ctrl-C
sets the cancellation token. Execution status and control comparison status
remain distinct; a solver failure cannot become a scientific PASS.

Imports and execution receipts use a synced temporary sibling and no-clobber
publication. Existing evidence files/directories are refused. This protects
against ordinary partial writes and replacement, not tampering or storage
failure. Generated runs and nuclear data remain out of Git. The control's
synthetic cross sections must never supply reactor results.
