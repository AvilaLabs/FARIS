# Independent numerical controls

Status: the analytic control and OpenMC 0.15.3 execution path are implemented.
A real run using this synthetic library completed on this workstation. It is a
mathematical transport control and does not represent a physical material
calculation.

## One-group pure-absorber sphere

[`controls/pure_absorber_sphere.py`](../controls/pure_absorber_sphere.py)
defines a fixed-source problem with a monoenergetic point source at the center
of a homogeneous sphere, isotropic source direction, no scattering, and a
vacuum boundary. The default radius is 10 cm and the synthetic macroscopic
absorption cross section is 0.3 cm⁻¹. These values are deliberately selected
for a mathematical solver and normalization control. They are not evaluated
nuclear data, do not describe a material, and must not be used as a reactor
benchmark.

Every neutron starts at the center and travels a distance `R` to the boundary
if it survives. With exponentially distributed absorption distance and
`Sigma_a > 0`, the exact per-source-particle expectations are:

| Response | Exact value | OpenMC tally meaning / units |
| --- | --- | --- |
| Escape probability | `exp(-Sigma_a R)` | OpenMC global leakage tally: leaked source particles per source particle |
| Absorption probability | `1 - exp(-Sigma_a R)` | `absorption`: reactions per source particle |
| Integrated track length | `(1 - exp(-Sigma_a R))/Sigma_a` | `flux`: particle-cm per source particle, equivalent to cm per source particle |

The exact responses satisfy `Sigma_a * integrated_track_length =
absorption_probability` and `escape + absorption = 1`. For the defaults,
escape is 0.049787068367863944, absorption is 0.950212931632136, and integrated
track length is 3.167376438773787 cm per source particle. The analytic record
is independently available without OpenMC.

The finalized run used seed 123456789, 100 batches, 10,000 particles per batch,
and 8 OpenMP threads (1,000,000 nominal source histories). OpenMC 0.15.3 returned analog
absorption 0.9501120 ± 0.0002236894 reactions/source particle, global leakage
0.0498880 ± 0.0002236894 leaked particles/source particle, and track-length
flux 3.1669682 ± 0.002900160 particle-cm/source particle. Each estimate met
the three-standard-deviation rule. Absorption plus leakage was
0.9999999999999998 per source particle (residual −2.22 × 10⁻¹⁶), consistent
with source-by-source conservation for this no-scatter, nonmultiplying case.
The solver status is `COMPLETED`; control status is `PASS`. The control script
SHA-256 is `3029fc19a5f71c7e9e9efda70a6bc3ac872182119ef45d4b960fd137755eb188`,
combined input SHA-256 is `bbcf42a6a0ec760d39633f071fa58bcff7daccdde01eaaaf2c0127d7f9eb5779`,
and the OpenMC executable SHA-256 is
`d939af8f885800c8727e687e628c2802138674e7f3b108371ef813a606939f51`. The
statepoint SHA-256 is `730aca4b925ffbf96c4e73c126edf23d90135e6b07cf933544415d828dbb9af9`.
The generated JSON, XML inputs, multigroup library, and statepoint are in the
ignored local output directory `runs/openmc-pure-absorber-control-final-20260930f/`;
they are verification artifacts, not bundled source data.

An earlier completed run used control script SHA-256
`564b19f76c0d4388d711018a18605636046b9e1ea806ffc20ceb0b33a280e780`; its
record remains in `runs/openmc-pure-absorber-control-final-20260930e/`. The
later run above records the explicit thread count, caps, and finalized failure
handling in the current script. Both use OpenMC 0.15.3 and the same seed,
batch/history counts, geometry, synthetic macroscopic data, and estimators.

The OpenMC tally is integrated over the sphere. The cell volume is not divided
out: dividing by volume would produce a volume-averaged flux density and would
not match the exact total path length. Source strength is 1.0, while all
reported comparison responses are per source particle. Do not multiply these
per-history estimates by source rate when comparing to the control. A separate
source-rate conversion may multiply a verified per-particle value to obtain a
rate, with units shown explicitly.

The Python API constructs one multigroup energy group spanning 0–20 MeV with
macroscopic total and absorption cross sections equal to `Sigma_a` and zero
scattering. A 14 MeV fixed source lies in that group. This intentionally
synthetic group library is produced locally by OpenMC's Python API; it is not
an external data library. OpenMC requires a multigroup HDF5 library in
multigroup mode, and its API supports `XSdata` inside `MGXSLibrary` for that
purpose. Absorption uses OpenMC's analog estimator, integrated flux uses the
track-length estimator, and escape uses OpenMC's global leakage tally. The two
user tally estimators differ, and all responses use shared histories and may be
correlated. See OpenMC 0.15.3's [multigroup data
configuration](https://docs.openmc.org/en/v0.15.3/usersguide/data.html), [MGXS
library API](https://docs.openmc.org/en/v0.15.3/pythonapi/generated/openmc.XSdata.html),
and [multigroup library format](https://docs.openmc.org/en/v0.15.3/io_formats/mgxs_library.html).

## Run records and acceptance

The control emits an analytic JSON record with `status: NOT_EVALUATED` and
`solver_status: NOT_RUN`. It never presents the analytic answer as an OpenMC
result. `--run` requires the OpenMC Python package and executable; the executable
can be fixed explicitly with `--openmc-executable`, while the script is invoked
by the Python interpreter in the selected OpenMC environment. The code refuses
to use an existing output directory. The JSON records seed, batches, particles
per batch, nominal histories, solver version and path, input hash, raw means,
reported standard deviations, exact references, and solver status. A missing
package/executable produces `NOT_EVALUATED` with a `NOT_AVAILABLE` solver
status. An execution exception produces `NOT_EVALUATED` with a `FAILED` solver
status; a completed run whose statepoint or tally checks fail records solver
status `COMPLETED` and leaves the control `NOT_EVALUATED`.

For each of the three sampled responses, the control applies
`abs(estimate - reference) <= 3 * reported standard_deviation + 1e-12`, where
the small absolute floor is in the response's native units. This is a
predeclared, approximate three-standard-deviation consistency screen for a
single Monte Carlo run with at least 30 batches under the assumptions that the
reported sampling error is valid and the estimate is sufficiently close to
normally distributed. It is not a rigorous enclosure, proof of correctness, validation of the reported
uncertainty, or a physical acceptance criterion. Passing only supports
consistency for this sphere and these three responses at the sampled precision.
Use independent seeds or higher histories to investigate marginal outcomes;
do not tune histories, seed, cross section, or acceptance width after seeing a
result.

OpenMC reports each response's standard deviation, but this control does not
export covariance between the two user tallies and the global leakage tally.
It compares all three responses separately and makes no combined uncertainty
statement. Separate analog absorption and track-length flux estimators avoid a
tautological same-estimator check; shared histories can still correlate their
errors.

Examples (choose a new output path for each invocation):

```sh
python3 controls/pure_absorber_sphere.py --out runs/analytic-control-001
/path/to/openmc-python controls/pure_absorber_sphere.py --run \
  --openmc-executable /path/to/openmc --seed 123456789 \
  --batches 100 --particles 10000 --out runs/openmc-control-001
```

Invalid, non-finite, or non-positive radius/cross section values, non-positive
seed/particle/thread counts, fewer than 30 batches, more than 32 threads, and
history counts above 10,000,000 are rejected. Existing output
paths are rejected to preserve prior run evidence. For a reproducible rerun, choose a new path and
record the changed seed/history inputs.

The control's lightweight analytic and negative-input checks run with
`python3 -m unittest discover -s controls -p 'test_*.py'`.

## Scope and limitations

### FARIS integration verification

The Rust CLI also completed the same one-million-history control with one
thread. Its bundled worker, preflight request, execution receipt, XML/library
inputs and statepoint are preserved locally in `runs/faris-absorber-003/`.
Execution was `SUCCEEDED`, control status `PASS`, and the completed result's
script/executable/history/seed/thread bindings matched the Rust request.
Physical qualification remained `NOT_EVALUATED`.

This execution used worker SHA-256
`3029fc19a5f71c7e9e9efda70a6bc3ac872182119ef45d4b960fd137755eb188`,
input SHA-256 `d9e91a92ff45c62db17181249769597eb3eb9f8495da20a0e30eef64cc4cf753`,
and statepoint SHA-256
`d2465abe4d1bc37f21a215af630c5aba6cced301632ea62ecf3bcd0f9be18c56`.
Elapsed worker wall time was about 6.66 seconds on this workstation; this is
an observed control runtime, not a reactor runtime budget.

A separate ten-million-history CLI attempt was deliberately interrupted after
input generation. It returned `CANCELLED`, wrote an execution receipt, and
retained physical qualification `NOT_EVALUATED` in
`runs/faris-absorber-cancel-001/`. Unit checks cover owned descendants holding
output pipes, deadlines, output limits and pre-cancelled launch. A Decimal
arithmetic check independently verified the CLI's source-rate/volume/unit
conversions using explicit test-only raw tallies; those fixtures are never
reactor results. Existing output and stale scenario identities were rejected.

### Scientific boundary

This control tests a narrow chain: centered isotropic source setup, one-group
fixed-source attenuation, integrated absorption and track-length tally units,
and per-source normalization. It does not validate continuous-energy nuclear
data, a FARIS reactor material, fusion source modeling, arbitrary geometry,
spatial mapping, shielding response, breeding, energy deposition, source-rate
normalization, geometry volumes, time evolution, or Rust adapter correctness.
It does not create any physical result for FARIS. A future FARIS comparison must
retain the raw OpenMC run and input identities and independently check its own
normalization, volume, units, and region mapping.

Relevant OpenMC 0.15.3 documentation: [independent source](https://docs.openmc.org/en/v0.15.3/pythonapi/generated/openmc.IndependentSource.html),
[fixed-source settings](https://docs.openmc.org/en/v0.15.3/usersguide/settings.html),
[tally scores and units](https://docs.openmc.org/en/v0.15.3/usersguide/tallies.html),
and [execution API](https://docs.openmc.org/en/v0.15.3/pythonapi/generated/openmc.run.html).
