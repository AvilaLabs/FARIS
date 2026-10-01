# Cold-data ARC-inspired transport reference

**Scope:** executable OpenMC fixed-source numerical reference for checking FARIS
geometry export, explicit material construction, source sampling, raw response
mapping, and Rust normalization. Corrected production fixtures are
`arc-cold-coupled-control-001` and `arc-cold-reference-port-001`. They retain
two authored radial allocations and explicit cold-data material recipes. The
historical `arc-cold-reference-001` fixture is not the identity used by the
corrected coupled production records. This is **not** the ARC reactor, an
operating liquid blanket model, an experimental benchmark reproduction, or a
qualified design prediction. Keep scientific qualification `NOT_EVALUATED`.

## Scenario and physical inputs

The full circular-torus model uses major radius 3.3 m, plasma minor radius
1.0 m, 0.08 m plasma-to-first-wall gap, and 1.20 m radial build. The reference
allocation has 0.45 m each for blanket and shield; breeder-emphasis has 0.55 m
blanket and 0.35 m shield. Shared shell thicknesses and identities are fixed.
The control fixture has no ports, divertor, supports, flow channels, coil
windings, or other resolved penetrations. Its separate ported sibling adds one
authored rectangular void bore. The viewport cutaway is display-only.

The authored recipes in the two `cold-coupled-control.*.physics.json` files are:

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
The full-model 12×8×12 mesh uses bin numbering `i + nx*(j + ny*k)`. Final primary
records use the outboard-local 24×12×24 mesh; coarse local and one-bin window
meshes are separate diagnostics. Bounds come from the Rust engine. Analytic torus
shell and Cartesian bin volumes accompany raw scores for normalization.

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
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --mesh-preset outboard-local \
  --audit references/openmc-library-audit.json \
  --cross-sections /path/to/cross_sections.xml \
  --python /path/to/openmc-env/bin/python --openmc /path/to/openmc-env/bin/openmc \
  --particles 10000 --batches 100 --seed 123456789 --threads 1 \
  --output runs/cold-coupled-control-reference-001

cargo run -- reactor inspect \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --run runs/cold-coupled-control-reference-001/run.json
```

The supplied audit records this workstation's local cache. Another installation
must run `audit_library.py` and supply its own audit; library content differences
cannot be silently accepted. `reactor run` refuses an existing output directory.
Ctrl-C cancels the owned process group. Neither process completion nor accepted
normalization sets a scientific PASS. The CLI allows up to ten million histories,
32 threads and a one-hour timeout; job resource and artifact caps are recorded per
run. These are resource bounds, not a sandbox.

Native replay uses the same case definitions and rechecks its recorded inputs,
raw artifact, sampling, audit, adapter identity, volumes, and normalization:

```bash
cargo run -p faris-app -- \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --physics scenarios/arc-inspired/cold-coupled-control.breeder-emphasis.physics.json \
  --run runs/cold-coupled-control-reference-001/run.json --field-view flux-slice
```

Repeat `--run` for the other arrangement. For port runs, select
`cold-reference-port.scenario.json` and its matching port physics cases. Add
`--python`, `--openmc`, `--audit` and `--cross-sections` to enable the native
**Run transport** worker. Closing the
native application cancels and joins owned workers; camera interaction remains
available while a job runs.

The display uses a fixed flux scale across arrangements. Each displayed mesh
value is a full-bin volume average, including void; the selected local 24×12×24
primary mesh does not resolve point peaks. Gray bins have zero samples;
desaturated bins have more than 30% relative standard error. Neither treatment supplies a zero-flux
bound or total uncertainty. Component coloring represents region averages,
not a resolved within-component field. Spectra remain supplementary raw files.

### Corrected campaign results and precision limits

Four corrected primary records each transport 1,000,000 histories (100 batches ×
10,000 particles, one thread) with independent seeds 81130011–81130014. The two
unported control variants and two port variants use the 24×12×24 outboard-local
mesh over identical bounds. Wall time was about 19.6–22.8 minutes per run. Exact
run, input, worker, statepoint, raw tally, spectra, audit, executable, scenario,
physics and data identities are recorded in
[`transport-refinement-results.json`](../references/transport-refinement-results.json).
Those records are replayable numerical results, not qualified physics evidence.

Evidence levels are distinct: source-rate, unit-conversion, shell-volume,
mesh-volume, ownership and tally-normalization controls are analytical/software
checks; independent Rust midpoint and Python/SciPy integration are geometry
implementation cross-checks, not independent transport solvers. This campaign
does not reproduce an experimental neutron-transport benchmark or establish
agreement with measurements. The ARC paper is design precedent only, not an
as-built model specification; material, data-temperature, source, and geometry
applicability for an ARC-like plant remain unevaluated. There is no design-level
validation or qualification here.

The normalized whole-model gross H3 production was 1.2945, 1.3085, 1.2952 and
1.3117 H3 atoms per source neutron for control-reference, control-breeder,
port-reference and port-breeder, respectively. Their direct Monte Carlo RSEs
were 0.088%, 0.086%, 0.106% and 0.091%. Coupled total deposited heating was
492.74, 491.65, 492.92 and 491.95 MW with RSE 0.050%, 0.046%, 0.061% and 0.050%.
These are the model's gross tritium and OpenMC deposition responses; whole-model
H3 includes all modeled regions and is not an extraction-corrected TBR. Heating
is not a thermal balance. The source convention injects one 14.1 MeV neutron per
reaction while normalizing with 17.6 MeV per D-T reaction; the implied neutron
source power is 420.6 MW, and the difference from scored deposition is not an
energy-closure or power-conversion result.

Before these runs, the worker declared exploratory RSE goals of at most 5% for
integrated whole-model H3 and component heating, and at most 10% for local
component heating and magnet/mesh flux. All four passed the direct whole-model
checks, but each has 5,133–5,236 unmet checks out of 6,925; the failed checks are
mostly local mesh bins and low-response material regions. Magnet-flux RSE ranges
from 13.9% to 35.0%, and magnet-surrogate integrated heating RSE from 11.2% to
12.6%. Therefore `all_goals_met` is false for every primary. Show local values
with their direct SE and retain the visible unresolved status. The targets are
internal sampling goals, not validation tolerances or physical limits.

Independent arithmetic reconstructed all 6,954 normalized responses in each
fine primary from raw tally units, domain volumes and the 525 MW source-rate
convention, independently computing `P_fusion / (17.6×10⁶ eV × 1.602176634×10⁻¹⁹ J/eV)` from each exact scenario and physics input. It matched each recorded source rate at the stored precision. Mean, integrated mean and volume matched exactly; tally-only SE reconstruction differed by at most 1.54×10⁻⁵ relative for ported records because the normalized record separately propagates the Monte Carlo port-volume SE.
Independent torus-shell arithmetic matches unported component volumes and the
whole torus to floating-point precision; mesh-bin volumes and sums match the
Cartesian bounds. Each run's pre-transport OpenMC `Geometry.find` ownership and
clearance audit passed all 117 probes. Both port-primary independent adaptive
quadrature reports passed all six component checks and remain bound to the exact
run/worker/raw/input identities. These test implementation and geometry
partitioning only.

A separate two-record direct window experiment compared the feature-free and
ported reference allocation with independent seeds. Each used one direct tally
bin spanning X=[4.34,5.58] m and Y,Z=[−0.15,0.15] m. This volume average mixes
material, port void, and corners outside the torus. The flux estimates were
1.419±0.0323×10¹⁸ and 2.531±0.0585×10¹⁸ n·m⁻²·s⁻¹ (one-standard-error values;
RSE 2.28% and 2.31%). Under the predeclared internal screen—both RSE ≤10% and
the absolute difference greater than twice `sqrt(SE_control² + SE_port²)`—the
authored window difference passes at 16.64 combined SE. This resolves only that
specific model-window contrast, not a physical port streaming factor or
component maximum.

Local coarse and fine maps used the same bounds at 12×6×12 and 24×12×24 with
independent seeds and one million histories per run. Fine-to-coarse ratios of
summed cell means were 0.9981 (control) and 0.9994 (port). Per-cell relative
differences were broad (median 8.2%/11.0%, 90th percentile 79.7%/100%), with
large sparse-bin tails; covariance across tally bins is unavailable. No
quantitative map-convergence tolerance was predeclared, so map convergence is
`NOT_EVALUATED`. The ratios are descriptive point estimates only: do not sum
bin SEs, infer peak convergence, or use the apparent aggregate agreement as a
statistical acceptance test.

The ported input is intentionally a separate branch, not a changed baseline:
`cold-reference-port.scenario.json` preserves the two radial variants and adds
one finite outboard rectangular-prism void port. Its worker cuts each affected
cell, classifies port-box points against a separate unperforated OpenMC
geometry, confirms the same points map to the explicit port-void cell in the
transport geometry, and reports component volume estimates with binomial
standard errors. Before transport starts, OpenMC `Geometry.find` also checks
the actual plasma, declared 0.08 m clearance, and every component's inner,
interior, and outer ownership at multiple directions away from the port. The
recorded geometry-ownership audit binds exact scenario and input hashes and
includes expected and observed cell and material identifiers. Rust compares
component volumes against a separate midpoint quadrature control, and the
independent adaptive-integration report is required before packaging; midpoint
convergence alone is not a rigorous bound. These are geometry checks only.
Ported material/build values remain authored surrogates and their scientific
status remains `NOT_EVALUATED`.

The first 1M coupled campaign exposed a real geometry defect before primary
delivery: the transport worker began the first-wall material at the plasma
surface (1.00 m), filling the scenario's explicitly declared 0.08 m void
clearance instead of starting the wall at its 1.08 m inner radius. Its first
port-volume implementation also reused a mutable CSG region and queried only
the already-cut geometry, reporting zero removed component volume. We corrected
the cell partition, separated the sampling control geometry, verified final
port-to-void mapping, and added pre-transport ownership probes. The prior
coupled smoke, coarse 1M control records, two port 1M attempts, and the earlier
in-repository neutron-only million-history pair are retained as superseded or
rejected diagnostics; none is primary evidence for the
corrected geometry. Exact run/input/artifact hashes and rejection states are in
[`geometry-correction-superseded-runs.json`](../references/geometry-correction-superseded-runs.json). The
older arithmetic receipt is marked superseded in
[`cold-reference-verification.json`](../references/cold-reference-verification.json);
its arithmetic pass is not a physical or geometry validation.

Spatial refinement is selected with `--mesh-preset`: `coarse` retains the
full-model 12×8×12 mesh; `outboard-local-coarse` (12×6×12) and `outboard-local`
(24×12×24) have identical bounds so they test spatial averaging only. The
separate `outboard-port-window` preset uses a single direct OpenMC mesh tally
over the same box for both the feature-free control and port case: X from
R+1.04 m to the outer-envelope X bound, Y and Z from −0.15 m to +0.15 m. Its
volume average intentionally mixes material, the port void, and box corners
outside the torus along the outboard radial path; it is a transport-window
response, not a magnet-material flux or component failure estimate. This direct tally provides its own standard error.
Do not sum per-bin standard errors to estimate window uncertainty.

Before the independent local comparisons, require exact replay of every
request/artifact, sufficient in-run histories, and the port's independently
checked volume report. Treat a path-window effect as sampling-resolved only if
both independent-seed estimates have RSE ≤10% and their absolute difference
exceeds twice the combined one-standard-error value
`sqrt(SE_control² + SE_port²)`. The 10% and 2-SE gates are internal screening
rules for this demo, not validation tolerances or physical limits. The reported
±2 combined-SE band is a descriptive sampling screen only, not a calibrated
confidence interval or physical-model bound; it represents Monte Carlo sampling
uncertainty only. Otherwise label the effect unresolved.
Local maps are descriptive: report bin RSEs and do not promote any bin maximum
or coarse-to-fine visual change to a physical conclusion when it misses the
10% internal precision goal. Coarse and fine maps are separate runs with
independent seeds; no paired covariance is assumed for comparisons across
runs.

Generated runs, solver statepoints and nuclear data remain outside Git. Only
the corrected-geometry worker revision, with its exact hashes, is eligible for
the corrected local-fine primary runs, identified in the refinement-results
receipt. The geometry audit and port-volume report are required receipts alongside
transport and normalization; none is a physical validation.

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

- Corrected [control scenario](../scenarios/arc-inspired/cold-coupled-control.scenario.json) and [port scenario](../scenarios/arc-inspired/cold-reference-port.scenario.json) retain separate SHA-256 identities; their physics cases bind each allocation by variant ID. Historical `cold-reference.scenario.json` runs are not primary evidence.
- The bounded [transport campaign and refinement receipt](../references/transport-refinement-results.json) binds every run, raw tally, worker, statepoint, spectra sidecar, audit, data-library digest and independent port-volume report.
- [Superseded-geometry records](../references/geometry-correction-superseded-runs.json) preserve the earlier geometry defect and list the corrected primary receipt hashes.
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
