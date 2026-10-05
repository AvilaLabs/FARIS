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

## Fast flux in magnet regions

`flux_above { energy_min_ev }` scores neutron flux integrated above an energy
bound; the magnet screening responses use 1e5 eV (0.1 MeV), the energy the REBCO
fast-fluence value of Sorbom et al. (arXiv:1409.3540) refers to. The score is the
same OpenMC track-length `flux` as the whole-energy response (raw cm per source
neutron, normalized with the same factors), restricted by an energy filter that
runs from the bound to 1 GeV. It is a separate score from `flux`, so the
energy-integrated flux and the per-component spectra are unchanged and a fast
flux is never shown under the energy-integrated name.

The domain `component_region { component_id, region }` selects a region of a
toroidal-shell component by the major radius R0, the cylindrical radius R from
the torus axis and the toroidal angle phi (angle 0 is the +x axis, the centre of
the outboard port prism):

| Region | Meaning |
| --- | --- |
| `inboard_half` | R < R0, all toroidal angles |
| `outboard_half` | R >= R0; with `excluding_sector_half_width_rad: w`, only where the angle from the port centre exceeds w |
| `port_sector { half_width_rad: w }` | R >= R0 and the angle from the port centre is at most w |

The default w is 0.1745 rad (10 degrees), larger than the angle the port itself
subtends at the magnet radius, so the sector is the magnet behind and around the
port, not only the port footprint. A request may carry one sector width per
component and it must lie in (0, pi/2). Regions are defined for `flux_above`
scores only. The inboard half, the outboard half without the sector and the sector
partition the component; the whole-component `flux_above` response on a
`component` domain is kept as the reference they sum to.

`request_for_case` adds, for the `magnets` component, `magnets-fast-flux`,
`magnets-inboard-fast-flux`, `magnets-outboard-fast-flux` and
`magnets-port-sector-fast-flux`, for every arrangement, with or without a port,
so a port arrangement and its control are directly comparable. The width is chosen
with `request_for_case_with_regions`.

Worker realisation. OpenMC's `CylindricalMesh` is always about the z axis and a
mesh filter has a translation but no rotation, while the torus axis is y, so the
planned cylindrical-mesh filter cannot express these regions without rotating
the model. The worker instead partitions the magnet cell with a nested universe:
the parent cell keeps its identity, name and port cut, and is filled with
sub-cells cut by a y-axis cylinder of radius R0 and two planes through the torus
axis at plus and minus the sector half width. Every existing `CellFilter` on the
parent still matches (it matches at any nesting depth). Each region response is
one scalar tally on a cell filter, an energy filter and the neutron filter with
the track-length estimator, so its per-batch values and covariance work exactly
like every other scalar response, and no per-bin summing or wrap-around at angle
0 is needed. The geometry ownership audit identifies a cell by its outermost
cell and its material by its innermost.

Region volumes. For a shell between minor radii a < b the volume element is
dV = R r dr dtheta dphi with R = R0 + r cos(theta); the outboard half annulus
integrates to R0 pi (b^2 - a^2) / 2 + 2 (b^3 - a^3) / 3 and the inboard half to
the same with a minus sign, so the full-torus volume of a region is
Delta-phi times that, with Delta-phi = 2 pi for a half, 2 w for the sector and
2 pi - 2 w for the outboard half without it (the halves sum to
2 pi^2 R0 (b^2 - a^2)). With a penetration, the removed part of each affected
region is the worker's uniform-point sampling of the port box (each removed
point is classified by position into every requested region, so the port is never
assumed to lie wholly in one region) with its binomial standard error, carried
as the region's volume standard error. Rust checks every region volume against
the exact formula minus its own independent midpoint estimate of the removed
part. `controls/check_transport_arithmetic.py` recomputes the region volumes at
50 digits, checks that the three region volumes sum to the component volume and
that the region raw fast-flux track lengths sum to the whole-component fast flux.

## Response covariance

Each new run records the Monte Carlo sampling covariance between its scalar
response means (every non-mesh response), so correlated transport results can be
sampled later instead of assuming independence. The worker asks OpenMC for a
statepoint after every batch, reads each scalar tally's cumulative `sum`, and
takes consecutive differences as per-batch values `x_b`. Before using them it
checks against the final statepoint that `mean(x_b)` equals OpenMC's tally mean
(relative 1e-12) and `sqrt(var(x_b, ddof=1) / n)` equals its standard deviation
(relative 1e-9). This relies on OpenMC 0.15.3 defining `mean = sum / n` and
`std_dev = sqrt((sum_sq / n - mean^2) / (n - 1))` (`openmc/tallies.py`). The
covariance of the batch means is the sample covariance of the per-batch
vectors (ddof = 1) divided by `n`; method id `batch-means-sample-covariance/v1`.
At least 2 batches are required. Only the final statepoint is kept; the
intermediate ones are deleted after reading (about 0.3 MB each for the coarse
mesh, so a 1000-batch run briefly holds a few hundred MB against the 512 MiB
artifact cap).

A reactor run accepts 30 to 1000 batches and at most 50 million histories
(`MAX_RUN_HISTORIES`). Wall time is bounded separately by `--timeout-seconds`
(default 3,600 s, at most 14,400 s, `MAX_RUN_TIMEOUT_SECONDS`); a 30 million
history run takes about 68 minutes at 7 threads on the reference laptop.
Small regions need the upper end without variance reduction: at 10 million
histories the magnet port-sector fast flux of the no-port control has a 36 to
54 % relative error, too large for Gaussian ensemble sampling (more than 1 %
of draws would be negative), and 30 million brings it to about 30 %.

The worker writes `transport-batch-values.json` (response ids, batch count and
per-batch raw values) and adds `response_covariance` to
`transport-artifact.json`: method, batches, response ids, the row-major raw
matrix (raw tally units per source, product units off the diagonal), and the
batch-values file name and sha256. Rust scales entry `(i, j)` by the same
per-response factors as `integrated_mean` (`cov_ij * s_i * s_j`) and records it
as `response_covariance.integrated` in the normalized result. Artifacts without
the field still parse and give `None`; consumers that need correlations must
then fail closed.

It covers Monte Carlo sampling between responses of one run. It does not include
volume-estimate, nuclear-data or model uncertainty, and with fewer batches than
responses the matrix is rank-deficient (at most `batches - 1` independent
directions), so the estimated correlations are noisy and can be exactly +/-1.

Normalization rejects the artifact if the method is unknown, the ids do not match
the scalar results one-to-one, the matrix is not `n * n` or has a non-finite
entry, it is asymmetric beyond 1e-12 relative, a diagonal entry differs from
`integrated_standard_error^2` by more than 1e-6 relative, a zero-variance
response is correlated with another, a correlation is outside `[-1 - 1e-9,
1 + 1e-9]`, or the correlation matrix is not positive semidefinite (diagonally
pivoted Cholesky, tolerance 1e-9 on the unit-diagonal scale). The independent
control `controls/check_response_covariance.py` recomputes the matrix from the
batch values with exact fractions and checks the integrated scaling.

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
