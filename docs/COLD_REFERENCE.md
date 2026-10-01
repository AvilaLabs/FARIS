# Cold-data ARC-inspired transport reference

**Scope:** executable OpenMC fixed-source numerical reference for checking FARIS
geometry export, explicit material construction, source sampling, raw response
mapping, and Rust normalization. The `arc-cold-reference-001` scenario keeps the
existing two radial allocations but replaces the unassigned material labels
with declared cold-data recipes. It is **not** the ARC reactor, an operating
liquid blanket model, an experimental benchmark reproduction, or a qualified
design prediction. Keep scientific qualification `NOT_EVALUATED`.

## Scenario and physical inputs

The full circular-torus model uses major radius 3.3 m, plasma minor radius
1.0 m, 0.08 m plasma-to-first-wall gap, and 1.20 m radial build. The reference
allocation has 0.45 m each for blanket and shield; breeder-emphasis has 0.55 m
blanket and 0.35 m shield. Shared shell thicknesses and identities are fixed.
There are no ports, divertor, supports, flow channels, coil windings, or resolved
penetrations. The viewport cutaway is display-only.

The authored recipes in the two `cold-reference.*.physics.json` files are:

- Natural-isotope W, 19,300 kg/m³, as the first-wall dense-element proxy.
- Li2BeF4 formula stoichiometry with 90 atom% Li-6 within lithium, Be-9 and F-19,
  and density 2,137.175 kg/m³. The density is a fixed-crystal-volume isotope-mass
  reweighting from Gardner et al.'s Li-7-rich solid-salt X-ray fit. It is an
  authored assumption, not a measured enriched-salt density or blanket bulk
  density. This is solid Li2BeF4 at the declared cold numerical state, not liquid
  FLiBe.
- Natural-Ti, natural-H ideal TiH2 at 3,750 kg/m³, representing theoretical
  crystalline density. It is not the packed bulk density of ARC's TiH2 powder
  concept; no porosity, canister, or coolant is modeled.
- Natural-isotope pure Fe, 7,874 kg/m³, as a structural surrogate, not Inconel
  718.
- A geometric void and natural Cu at 8,960 kg/m³ as a magnet-response surrogate.
  The Cu result is not a superconducting-coil failure or lifetime prediction.

All `material_temperature_k` values are null: no physical material temperature
is specified. Every nuclide uses a nuclear-data target of 293.6 K, which matches
the locally audited numeric HDF5 temperature 293.59430848016336 K within 0.1 K.
The files label that table `294K`, and OpenMC 0.15.3 selects by this rounded
label. The adapter separately selects at 294 K with a 1 K nearest-label window;
it does not substitute that rounded label for the exact-kT audit or widen the
PhysicsCase 0.1 K data compatibility check. There is no interpolation or hot-data
extrapolation. The selected local FENDL
files have not had their upstream provenance, checksum against the publisher,
or redistribution terms authenticated. The candidate XML also contains no
thermal-scattering law. Bound molecular scattering for crystalline Li2BeF4 and
TiH2 is therefore not modeled; do not treat low-energy spectra or breeding as
qualified until that sensitivity is assessed.

## Source, transport and returned records

Use the same uniform-in-volume source over the full plasma torus for each
variant. A Cartesian uniform box is rejection-sampled into the plasma cell;
particle direction is isotropic and energy is monoenergetic at 14.1 MeV. The
request's 17.6 MeV per D-T reaction and one neutron per reaction define later
Rust normalization at 525 MW. The adapter reports only per-source-neutron raw
scores and their OpenMC standard errors; Python does not multiply by absolute
source strength.

Run from Rust-owned bounded job directories:

```text
python reactor_transport.py --input input.json --output-dir solver
```

The directory must be fresh. Input schema is `faris-openmc-input/v0.1` and
contains the exact exported manifest, physics case, transport request, sampling
plan, Cartesian mesh, absolute OpenMC/data paths, and bound nuclear-data digest.
The worker verifies selected data-file sizes and hashes, XML-to-file mapping,
nuclide coverage, and numeric temperature metadata before export. OpenMC 0.15.3
builds a plasma source cell, concentric `YTorus` layers in centimetres, an outer
vacuum boundary, component tallies, and the requested regular-mesh flux tally.
For the separate port manifest, it removes the axis-aligned rectangular prism
from exactly the listed component cells and fills its in-envelope portion with
the declared material. The original unported scenario remains the analytic
torus-volume control.
The 12x8x12 mesh bin numbering is `i + nx*(j + ny*k)`; mesh limits are supplied
by the Rust engine. Analytic torus shell and mesh-bin volumes accompany raw
scores for the Rust normalizer.

The adapter writes `solver/transport-artifact.json` in
`faris-transport-artifact/v0.2`, echoing the complete request and one raw tally
per response. Neutron and photon spectra are recorded in
`solver/transport-spectra.json` as an identity-bound sidecar with 0 to 1 GeV
group edges, per-component raw group means and standard errors, and the
sampled volume and its standard error. They are flux spectra, not deposited-
energy spectra. Rust validates the sidecar against the exact request, input,
solver, data and history count, stores normalized group flux, and checks the
neutron group sum against the matching integrated component flux to
floating-point summation tolerance. Per-bin standard errors describe transport
sampling; volume uncertainty is kept separately and is not silently folded
into them.
Solver stdout and stderr are streamed byte-for-byte to the parent Rust worker
while separate local logs retain at most 4 MiB per stream. A stream above that
retention limit fails the worker and terminates the OpenMC child; Rust also
applies its independent 4 MiB-per-stream cap and process-group timeout/cancel
handling. No unbounded Python output capture is used. The worker records each
log's retained byte count, observed byte count, truncation state and hash.

The final statepoint is required to report OpenMC version `(0, 15, 3)`, fixed
source mode, the requested seed, batch count, completed current batch,
realization count and particles per batch. The worker records the statepoint's
filename, size and SHA-256, as well as SHA-256 for every exported OpenMC XML
input and verifies those XML files did not change during execution. Mesh bin
ordering is checked through OpenMC's own `RegularMesh.indices` iterator before
tally export: zero-based bin 0 maps to `(1,1,1)`, x increments fastest, then y,
then z. A nonzero exit, incomplete/mismatched statepoint, non-finite score, log
limit exceedance, or logged lost particle fails execution. The worker output
records execution status separately from scientific qualification.

The requested H3-production score is the modeled gross tritium birth rate per
source neutron. Its whole-model tally is a reconciliation diagnostic and can
include production outside the salt, including hydrogen-bearing regions such
as TiH2; do not label it a breeder-only TBR. Component H3 tallies remain
neutron-filtered. No extraction, retention, decay, recovery, or startup fuel
model is included.
Flux is the OpenMC volume-integrated cell or mesh-bin response before the Rust
normalizer applies absolute source rate and domain volume. Damage-energy and
heating uses coupled neutron-photon transport. For each material component and
the whole torus, the request records five `heating` tallies: directly scored
all-particle deposition (`heating-total-*`) and neutron, photon, electron, and
positron-scoped deposition. The particle-specific scores expose charged
secondary deposition that an n/γ-only split omits under local electron
treatment. They are diagnostics, not operands used to reconstruct total heat;
the all-particle tally is scored directly and retains its own standard error.
No uncertainty for a linear combination is inferred from separate tally SEs.
Neutron flux and H3 production, including the regular mesh, are
explicitly neutron-filtered. Heating uses OpenMC's collision estimator, while
flux, H3, and flux spectra use tracklength; the adapter checks these exact
estimator names in the final statepoint.

Before coupled transport, the worker verifies MT=301 for every used nuclide and
every required photon atomic-data file against the read-only audit, XML path,
byte size, and SHA-256. It also checks the content of the atomic-relaxation
tables; an `AtomicRelaxation` Python object alone does not prove that those
tables contain data. FENDL 3.2 photon files fail this check: OpenMC can parse
them, but their atomic-relaxation shell maps are empty, and OpenMC 0.15.3
segfaulted during a photoelectric collision when cascades were enabled. The
current combined local overlay uses FENDL neutron data with ENDF/B-VII.1
photoatomic plus atomic-relaxation files converted through OpenMC 0.15.3; the
audited shell maps cover every required photoelectric shell. H, Li, and Be have
no tabulated relaxation transitions in these files, while their shell
binding-energy and electron-count maps are present. The overlay and its
acquisition/conversion provenance remain local ignored inputs, not repository
data. The local candidate library lacks MT=901; `heating-local` is rejected.
With the combined data, OpenMC's neutron
heating score excludes transported secondary-photon energy and photon
interactions score deposition separately. OpenMC's `electron_treatment="led"`
option deposits secondary charged-particle energy locally rather than
generating secondary bremsstrahlung photons. These solver-defined responses
remain separate from alpha-particle transport, blanket thermal-fluid behavior,
structural temperatures, thermal losses, coolant enthalpy, or power conversion.

Rust scales per-source scores using the fusion reaction rate
`525 MW / (17.6 MeV per D-T reaction)`. This is the external D-T neutron-source
rate convention: one source neutron per reaction. It differs from normalizing
a fission system to an observed whole-system heating tally. The power
denominator uses the full 17.6 MeV D-T energy, while the transported particle
carries 14.1 MeV and the 3.5 MeV alpha is not transported. Therefore the
all-particle deposition need not equal 14.1 MeV per source neutron: nuclear
reaction Q-values can add or remove energy and neutrons or photons may escape.
The whole-model heating response is an energy-accounting diagnostic, not an
assumed conservation identity. Do not report thermal efficiency or electricity
from this transport case.

## Verification before relying on numerical values

### Run and inspect through the shared Rust engine

Use an existing OpenMC 0.15.3 environment and the XML it was audited against:

```bash
cargo run -- reactor run \
  --scenario scenarios/arc-inspired/cold-reference.scenario.json \
  --physics scenarios/arc-inspired/cold-reference.reference.physics.json \
  --audit references/openmc-library-audit.json \
  --cross-sections /path/to/cross_sections.xml \
  --python /path/to/openmc-env/bin/python --openmc /path/to/openmc-env/bin/openmc \
  --particles 10000 --batches 100 --seed 123456789 --threads 1 \
  --output runs/cold-reference-001

cargo run -- reactor inspect \
  --scenario scenarios/arc-inspired/cold-reference.scenario.json \
  --run runs/cold-reference-001/run.json
```

The supplied audit records this workstation's local cache. Another installation
must run `audit_library.py` and supply its own audit; library content differences
cannot be silently accepted. `reactor run` refuses an existing output directory.
Ctrl-C cancels the owned process group. Neither process completion nor accepted
normalization sets a scientific PASS. Default resource limits are one million
histories, one thread, 600 seconds, and bounded captured logs; memory and total
disk usage are not sandboxed.

Native replay uses the same case definitions and rechecks its recorded inputs,
raw artifact, sampling, audit, adapter identity, volumes, and normalization:

```bash
cargo run -p faris-app -- \
  --scenario scenarios/arc-inspired/cold-reference.scenario.json \
  --physics scenarios/arc-inspired/cold-reference.reference.physics.json \
  --physics scenarios/arc-inspired/cold-reference.breeder-emphasis.physics.json \
  --run runs/cold-reference-001/run.json --field-view flux-slice
```

Repeat `--run` for the other arrangement. Add `--python`, `--openmc`, `--audit`
and `--cross-sections` to enable the native **Run transport** worker. Closing the
native application cancels and joins owned workers; camera interaction remains
available while a job runs.

The display uses a fixed flux scale across arrangements. The spatial view shows
a horizontal layer of the 12×8×12 Cartesian mesh: each value averages its full
bin volume, including void. Gray bins have zero samples; desaturated bins have
more than 30% relative standard error. Neither treatment supplies a zero-flux
bound or total uncertainty. Component coloring represents region averages,
not a resolved within-component field. Spectra remain supplementary raw files.

### Current numerical evidence and precision limit

Both allocations completed one million histories on this workstation in about
257 and 264 seconds respectively, with one thread and separate seeds. Replay
revalidated each run's 1,164 responses. The saved
[verification summary](../references/cold-reference-verification.json) identifies
inputs, raw records and statepoints. An independent 50-digit Decimal check of
all 4,656 normalized values per run and torus/mesh volumes agreed within the
declared relative arithmetic tolerance of `1e-12`. Reproduce that arithmetic
control with Python's standard library:

```bash
python3 controls/check_transport_arithmetic.py \
  --run runs/cold-reference-1m-001 --run runs/cold-breeder-1m-001
```

This arithmetic control does not validate the transport values. The earlier
neutron-only million-history magnet flux has about 52% and 53% relative Monte
Carlo standard error for the two allocations; outer vessel/clearance regions
and many mesh bins are also poorly sampled. Those figures are historical
neutron-only pilots and are not estimates from the new coupled run. They cannot
establish a shielding preference or service-life response.

For the functional demo, predeclare exploratory precision goals separately
from scientific acceptance: at most 5% relative standard error for whole-model
H3 production and integrated component heating, and at most 10% for
component-local heating and magnet/mesh flux. These are FARIS numerical review
goals, not code standards, experimental uncertainty tolerances, or reactor
qualification limits. They do not make an under-sampled quantity acceptable;
every response must report its actual RSE and the presentation must expose
which goals were missed. At the historical 52% magnet-flux RSE, simple
independent-history scaling estimates about 27 million histories to reach 10%
RSE, well above the current 10-million-history cap. Use tested variance
reduction with an independent unbiasedness control or present that local
response as unresolved. Do not extrapolate the 1/sqrt(N) estimate into an
achieved result, or interpret any RSE target as engineering qualification.

The ported input is intentionally a separate branch, not a changed baseline:
`cold-reference-port.scenario.json` preserves the two radial variants and adds
one finite outboard rectangular-prism void port. Its worker uses the explicit
axis-aligned box to cut each affected cell, samples points uniformly inside
that box through OpenMC's Python geometry locator, reports component volume
estimates and binomial standard errors, and Rust compares those to the separate
midpoint-quadrature geometry control before normalization. The independent
adaptive-integration report is required for this geometry control; midpoint
convergence alone is not a rigorous bound. Ported material/build values remain
the same authored surrogates and their scientific status remains
`NOT_EVALUATED`.

Spatial refinement is selected with `--mesh-preset`: `coarse` retains the
full-model 12×8×12 mesh; `outboard-local-coarse` (12×6×12) and `outboard-local`
(24×12×24) have identical bounds so they test spatial averaging only. The
separate `outboard-port-window` preset uses a single direct OpenMC mesh tally
over the same box for both the feature-free control and port case: X from
R+1.04 m to the outer-envelope X bound, Y and Z from −0.15 m to +0.15 m. Its
volume average intentionally mixes material, the port void, and box corners
outside the torus along the outboard radial path; it is a transport-window response, not a magnet-material flux or
component failure estimate. This direct tally provides its own standard error.
Do not sum per-bin standard errors to estimate window uncertainty.

Before the independent local comparisons, require exact replay of every
request/artifact, sufficient in-run histories, and the port's independently
checked volume report. Treat a path-window effect as sampling-resolved only if
both independent-seed estimates have RSE ≤10% and their absolute difference
exceeds twice the combined one-standard-error value
`sqrt(SE_control² + SE_port²)`. The 10% and 2-SE gates are internal screening
rules for this demo, not validation tolerances or physical limits; the latter
is an approximate normal-coverage check. Otherwise label the effect unresolved.
Local maps are descriptive: report bin RSEs and do not promote any bin maximum
or coarse-to-fine visual change to a physical conclusion when it misses the
10% internal precision goal. Coarse and fine maps are separate runs with
independent seeds; no paired covariance is assumed for comparisons across
runs.

The million-history records preserve the earlier worker revision. A separate
30,000-history integrated smoke exercised the final bounded log streaming,
statepoint/settings verification, and XML/statepoint hashes. Those changes
do not alter the source, geometry, materials, scoring or explicit estimators.
Generated runs, solver statepoints and nuclear data remain outside Git.

Input identity, OpenMC execution, exact volume arithmetic, and Monte Carlo
standard errors are necessary implementation checks; they do not validate
materials or reactor physics. Before exposing values in the demo, confirm that
OpenMC returns a nonzero H3 score in the salt, component IDs match the exact
scenario bytes, source particles are contained in the plasma cell, no geometry
particles are lost, raw tallies retain their units and estimator, mesh ordering
matches the viewport, and Rust normalization occurs once. Use independently
computed torus/Cartesian volumes and verify the variant geometry differs only
in the declared blanket/shield allocation.

Do not reinterpret the transport result as an operating-history prediction.
Even if the Rust fuel/exposure/energy arithmetic passes its mathematical
controls, its projected inventories, exposure and electricity remain conditional
on unqualified cold-data transport, authored surrogates and explicit operating
assumptions. The earlier absolute WCLL comparison with an 8.5% TBR discrepancy
is not a qualified benchmark and is not used here.

## Evidence

- [Scenario](../scenarios/arc-inspired/cold-reference.scenario.json) and
  [reference physics case](../scenarios/arc-inspired/cold-reference.reference.physics.json)
  bind by exact scenario-byte SHA-256; the breeder-emphasis input binds to the
  same scenario with its own variant ID.
- [Input specification and material sources](DEMO_INPUT_SPEC.md).
- [Read-only OpenMC library audit](../integrations/openmc/audit_library.py) and
  [machine-readable audit/input record](../references/demo-input-spec.json),
  plus the standalone [audited-library inventory](../references/openmc-library-audit.json).
- Sorbom et al., [ARC design paper](https://arxiv.org/abs/1409.3540), for design
  precedent only; Gardner et al., [solid Li2BeF4 study](https://doi.org/10.1107/S1600576725000548)
  for the distinct Li-7-rich crystalline density fit.
- OpenMC 0.15.3 documentation for [independent source constraints](https://docs.openmc.org/en/stable/usersguide/settings.html),
  [torus geometry](https://docs.openmc.org/en/stable/usersguide/geometry.html),
  [tally scores](https://docs.openmc.org/en/v0.15.0/usersguide/tallies.html),
  [coupled heating and energy deposition](https://docs.openmc.org/en/v0.15.0/methods/energy_deposition.html),
  [recommended coupled heating normalization](https://docs.openmc.org/en/v0.15.0/usersguide/tallies.html#normalization-of-tally-results),
  [particle filters](https://docs.openmc.org/en/stable/pythonapi/generated/openmc.ParticleFilter.html),
  [stochastic geometry volumes](https://docs.openmc.org/en/v0.15.1/usersguide/volume.html), and
  [temperature treatment](https://docs.openmc.org/en/stable/methods/cross_sections.html).
