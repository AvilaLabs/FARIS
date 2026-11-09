<!-- kind: reference -->
# ITER_1D OpenMC reference execution

This control prepares and runs the local IAEA-NDS ITER_1D OpenMC model with
OpenMC 0.15.3. It is a bounded **code-to-code test and adapter exercise**. It is
not an experimental comparison, a validated reactor model, or the analytic
pure-absorber control in `NUMERICAL_CONTROLS.md`. The registered input/source
identity and current comparison scope are in
[`references/iter-1d-reference.json`](../references/iter-1d-reference.json).

The local benchmark bundle identifies its OpenMC input as version 1.1 and
contains `geometry.xml`, `materials.xml`, `iter_1d_source.cpp`, a CMake file,
and a prebuilt `libsource.so`. It does not include OpenMC settings or tally
definitions. The [IAEA-NDS repository license](https://github.com/IAEA-NDS/open-benchmarks/blob/main/LICENSE)
is CC BY 4.0. This repository does not copy the benchmark files or its nuclear
data; the worker copies selected inputs into a new local run directory and
records hashes.

The source-law translation was checked directly against the identified C++
file. Accepted source sites have unit weight. Candidate positions are uniform in
the Cartesian box `[-1115.2,1115.2] cm` in x and y and `[-1000.2,1000.2] cm` in
z, and are retained only in void cell 51. Directions are isotropic, and energy
is sampled from an untruncated normal distribution with mean
`14.055843863040961 MeV` and standard deviation `0.23863574091114037 MeV`.
The C++ source gives up after 100000 rejected box samples and emits a zero
weight site in that exceptional case. The translated OpenMC `IndependentSource`
uses its documented `domains` rejection constraint with resampling, so it has
the same conditional source law and unit accepted-site weight but not the same
random stream or that finite-cap failure branch. The geometry-derived box
acceptance probability is about 0.623; the probability of 100000 consecutive
rejections is about 10^-42356.3. A source-law check in each run receipt verifies
the C++ constants, source cell, geometry, and finite-cap tail before execution.

The bundled binary is never loaded. A one-time local smoke test rebuilt the
source using cached headers and system GCC 15.2 against the cached OpenMC 0.15.3
build made with GCC 14.3. The build succeeded, but OpenMC aborted on the first
history with stack smashing in `CompiledSource::sample`. The cause is
undiagnosed, so the custom binary is not used. The checked-in C++ source is
instead mirrored through the documented
[`openmc.IndependentSource`](https://docs.openmc.org/en/v0.15.0/pythonapi/generated/openmc.IndependentSource.html)
and [`openmc.stats.Box`](https://docs.openmc.org/en/v0.15.0/pythonapi/generated/openmc.stats.Box.html)
interfaces. This validates the declared source law and tested OpenMC API path;
it does not validate the C++ plug-in ABI or imply stream-level equivalence.

The worker adds two material-cell tallies to the benchmark geometry and
materials:

| Response | Estimator | Raw units per source neutron | Volume conversion |
| --- | --- | --- | --- |
| Neutron flux | tracklength | particle-cm/source | Divide cell-integrated track length by exact cell volume in cm³ for volume-average particle/cm²/source |
| Absorption | analog | reactions/source | Divide reactions/source by exact cell volume in cm³ for reactions/cm³/source |

The distinct estimators make absorption and flux a meaningful implementation
cross-check rather than deriving one response algebraically from the other.
Their uncertainties remain correlated because both use the same histories.
No summed component standard error is inferred. OpenMC documents flux as
particle-cm per source particle and reaction scores as reactions per source
particle in the pinned release's
[`tally API`](https://docs.openmc.org/en/v0.15.3/pythonapi/generated/openmc.Tally.html).
The tally-bin cell IDs must match the geometry-derived material-cell IDs.

Cell volumes are derived analytically from the benchmark's simple CSG: all
cells are concentric z-cylinder annuli bounded by the shared reflective planes
at ±1000 cm. Each volume is `π (r_outer² - r_inner²) (z_max - z_min)` cm³,
with inner/outer radii read from the referenced cylindrical surfaces. The
worker rejects other surface/region forms; it checks contiguous, nonoverlapping
annuli from the axis to the outer vacuum cylinder and checks that their sum is
the enclosing cylinder volume. These are geometry-derived exact volumes for
this specific input, not OpenMC stochastic volume estimates.

## Run

Use an existing OpenMC 0.15.3 Python environment and executable, an explicit
cross-section XML, and a fresh output directory. The worker does not install
software or data. It refuses to overwrite an existing directory, limits the
run to 10 million histories and 600 seconds, uses 100 batches, and stores the
raw stdout/stderr, input XML, a statepoint, and `reference-result.json`.
Requested histories must divide evenly across the 100 batches.

```sh
/path/to/openmc-python integrations/openmc/iter_1d_reference.py run \
  --benchmark-dir /path/to/ITER_1D \
  --cross-sections /path/to/cross_sections.xml \
  --openmc-executable /path/to/openmc \
  --output-dir /path/to/new-run-directory \
  --threads 1 --histories 1000000 --seed 1 --time-limit-sec 600
```

Exit status `0` means OpenMC completed and the statepoint identities and tally
shapes were checked. Exit status `2` means preparation, validation, execution,
or result verification failed; the output record is retained when the worker
owns that output directory. A completed run still records the numerical
comparison as `NOT_EVALUATED` because no compatible reference response set is
wired into this control.

The local JADE dummy fixture has an ITER_1D metadata label and a statepoint,
but it lacks the run settings and input identities needed to prove that it
corresponds to the current benchmark inputs. Another nearby JADE dummy result
directory is labelled Oktavian. Neither is promoted to a numeric acceptance
target. The fixture demonstrates that response files exist locally; without
their exact source, geometry, materials, tally definitions, and nuclear data
identities, it cannot supply a trustworthy benchmark value.

This control does not set a statistical pass band or turn standard deviations
into rigorous enclosures. Until the published reference is mapped to the
current tally definitions and normalisation and predeclared response criteria
are available, execution can be complete while scientific and code-to-code
qualification remain `NOT_EVALUATED`. It also does not validate
TBR, magnet lifetime, experimental accuracy, full tokamak transport, or the
FENDL library's provenance/temperature suitability for reactor claims.

## Published JADE MCNP total-flux data

JADE also publishes an ITER_1D total-neutron-flux CSV for MCNP 6.2 with
FENDL 3.2b. Its [raw result table](https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/dfc38c467c1a6d9c0f735081ada4fd644097d69e/ROOT/_mcnp_-_FENDL%203.2b_/ITER_1D/ITER_1D%20Total%20neutron%20flux.csv)
and [metadata](https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/dfc38c467c1a6d9c0f735081ada4fd644097d69e/ROOT/_mcnp_-_FENDL%203.2b_/ITER_1D/metadata.json)
are pinned to commit `dfc38c467c1a6d9c0f735081ada4fd644097d69e`. The CSV SHA-256
is `b231fd670e067e549fd1fd542e8faa7dacedc0bf0e058544369123d54a1675d3` and
the metadata SHA-256 is `6fa3d6d7a257ff010649880b18621c9ff25b1ced3196c1288f4eb1e18ac36f8b`.
The result table and metadata are covered by the repository's
[EUPL-1.2 license](https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/dfc38c467c1a6d9c0f735081ada4fd644097d69e/LICENSE).
The intake helper verifies the pinned CSV, metadata, JADE cell-coordinate map,
and MCNP tally deck by SHA-256, and keeps every downloaded source file outside
Git. It reads the published relative standard error, converts the tally value
and its uncertainty to per-source-neutron units using the deck's explicit
`FM204 1.222e21` factor, and does not derive uncertainty from any other source.
The MCNP manual describes tally relative error as an estimate of relative
standard deviation (1 sigma); see the [LANL MCNP manual](https://mcnp.lanl.gov/pdf_files/TechReport_2024_LANL_LA-UR-24-24602Rev.1_KuleszaAdamsEtAl.pdf).

The JADE post-processing config and input deck are separately pinned to JADE
commits `735331b3f9f277acc728a0783498742124d68c97` and
`10deb931cbc8f7c22185e4a00e98d260d0a7698d`. The helper requires their reviewed
hashes, maps all 97 CSV coordinates to unique MCNP cell IDs, and verifies exact
coverage of the 97 cells on tally F204. It checks those cell IDs and outer
cylindrical radii against the [pinned IAEA-NDS OpenMC geometry](https://github.com/IAEA-NDS/iaea-open-benchmarks/blob/1d9855b00e05c5d5e25a11026ac56940dc75b516/jade_open_benchmarks/inputs/ITER_1D/ITER_1D/openmc/geometry.xml)
(SHA-256 `80bef907daab030cf2175e54383c20a0cbf92b5b1a8aa26e9bc88b744fbb6228`,
CC-BY-4.0). The pinned OpenMC source implementation is also hash-recorded in
the external manifest. The JADE code repository is GPL-3.0-only; the manifest records
each source file's license separately, since the EUPL-1.2 statement above
applies to the published result files.
`Cells` denotes the outer radial
boundary of each tallied cell; F4 is a whole-cell volume-averaged response,
not a point value at that boundary. Dividing by the explicit FM204 multiplier
produces `particle/cm2 per source neutron`; the same factor is applied to the
published statistical standard uncertainty. Geometry and normalisation
mapping are therefore explicit, while the score remains blocked per detector.

The source spectrum remains a qualification gap. The pinned MCNP deck uses
`SP1 -6`, the MCNP Muir velocity-Gaussian spectrum. The [MCNP 6.2 manual](https://mcnp.lanl.gov/pdf_files/TechReport_2024_LANL_LA-UR-24-24602Rev.1_KuleszaAdamsEtAl.pdf)
defines its energy density in terms of the square root of energy and documents
the default `a = -0.01 MeV`, `b = -1` for DT fusion at 10 keV. The benchmark's
[pinned OpenMC source](https://github.com/IAEA-NDS/iaea-open-benchmarks/blob/1d9855b00e05c5d5e25a11026ac56940dc75b516/jade_open_benchmarks/inputs/ITER_1D/ITER_1D/openmc/iter_1d_source.cpp)
samples a normal energy variate with its own mean and standard deviation.
OpenMC's [`stats.muir`](https://docs.openmc.org/en/stable/_modules/openmc/stats/univariate.html)
also defines its Muir helper as a normal distribution; the benchmark's source
parameters are not shown equivalent to MCNP's `-6` defaults. This is an
approximation and parameterisation question, not evidence of an automatically
identical spectrum. It requires source sensitivity qualification or
documented parameter equivalence before scoring. JADE's
result metadata also does not bind output to hashes of the executable,
processed nuclear data, or run settings. Cross-code covariance remains
unknown. This is a code-to-code reference candidate, not an acceptance result
or physical validation. The existing `iter-1d` case and its records remain
unchanged; this source contains no neutron-absorption response.

JADE describes tally 204 as total neutron flux at the same locations as its
energy-binned flux and identifies the 97-cell radial extent in its
[computational-benchmark documentation](https://jade-a-nuclear-data-libraries-vv-tool.readthedocs.io/en/stable/benchmarks/computational.html).
Its raw-results repository documents the metadata fields and that CSVs are
post-processed outputs in the [repository README](https://github.com/JADE-V-V/JADE-RAW-RESULTS).

Fetch and create an external manifest after confirming network access; these
commands do not run a solver. Set the date to the actual retrieval date.

```sh
mkdir -p /tmp/faris-iter-1d-jade
python3 validation/cases/iter-1d-flux-jade-v1/build_manifest.py \
  --fetch \
  --csv /tmp/faris-iter-1d-jade/total-neutron-flux.csv \
  --out /tmp/faris-iter-1d-jade/manifest.json \
  --retrieved 2026-11-09
```

The command also writes `metadata.json`, `cell-radius-map.yaml`, `ITER_1D.i`,
`openmc-geometry.xml`, and `iter_1d_source.cpp` beside the CSV, verifying each pinned hash. The
manifest is not
placed in the repository because it contains third-party response values.
Every reference detector carries a blocker for unresolved source-spectrum and
producer provenance gaps, so scoring remains `NOT_EVALUATED`; do not remove
those blockers until these issues are reviewed. Unknown cross-code covariance
will remain `INCONCLUSIVE` even after those blockers are resolved.

The benchmark files do not include settings. This worker explicitly sets the
OpenMC room-temperature default to 293.6 K, nearest-temperature selection, and
10 K tolerance. The local FENDL cache contains 294 K neutron data for these
materials. This records the executable treatment; it is not a claim that the
data bound a hotter reactor-material temperature.
