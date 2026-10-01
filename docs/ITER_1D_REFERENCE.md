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
comparison as `NOT_EVALUATED` because no provenance-qualified ITER_1D response
reference is wired into this control.

The local JADE dummy fixture has an ITER_1D metadata label and a statepoint,
but it lacks the run settings and input identities needed to prove that it
corresponds to the current benchmark inputs. Another nearby JADE dummy result
directory is labeled Oktavian. Neither is promoted to a numeric acceptance
target. The fixture demonstrates that response files exist locally; without
their exact source, geometry, materials, tally definitions, and nuclear data
identities, it cannot supply a trustworthy benchmark value.

This control does not set a statistical pass band or turn standard deviations
into rigorous enclosures. Until a traceable reference and predeclared response
criteria are available, execution can be complete while scientific and
code-to-code qualification remain `NOT_EVALUATED`. It also does not validate
TBR, magnet lifetime, experimental accuracy, full tokamak transport, or the
FENDL library's provenance/temperature suitability for reactor claims.

The benchmark files do not include settings. This worker explicitly sets the
OpenMC room-temperature default to 293.6 K, nearest-temperature selection, and
10 K tolerance. The local FENDL cache contains 294 K neutron data for these
materials. This records the executable treatment; it is not a claim that the
data bound a hotter reactor-material temperature.
