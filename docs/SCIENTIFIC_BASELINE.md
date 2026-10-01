# FARIS M1 scientific baseline

Status: baseline recommendation, 2026-09-30. No FARIS transport result has been
qualified. This document freezes the validation route and decision rules, and
identifies inputs that still need evidence. It does not fill missing reactor
materials or imply that an ARC design has been reproduced.

## Recommended scope and reference ladder

Keep the demo question as a controlled study of the existing ARC-inspired
idealization: fixed outer radial envelope, two blanket/shield allocations, one
shared D-T source and one shared penetration. The geometry is a FARIS research
model, not an ARC reference model. Transport is stationary fixed-source OpenMC;
Rust subsequently consumes the per-source component responses and applies the
declared source strength, fuel inventory, exposure, event, and energy history.

Use three distinct validation layers:

1. **Adapter/reference reproduction:** reproduce the local IAEA-NDS JADE
   `ITER_1D` OpenMC benchmark (input metadata OpenMC 1.1; local tree under
   `development/iaea-open-benchmarks/jade_open_benchmarks/inputs/ITER_1D`).
   Its geometry and materials are ready to inspect and it is explicitly a
   cross-code benchmark template. It is a computational reference, not an
   experimental observation. Compare OpenMC against the benchmark's declared
   reference-code tallies after auditing the exact response definitions,
   library, source, and normalization. Do not call agreement “validation”.
2. **Experimental transport validation:** acquire the OECD-NEA SINBAD FNG
   ITER bulk stainless-steel shield benchmark package and reproduce its
   integral reaction-rate/attenuation responses first. This is the closest
   available experimental check of transport through a fusion-relevant shield
   and magnet mock-up. Separately acquire the FNG HCPB TBM benchmark if FARIS
   will make breeding claims. Its measured tritium production is the relevant
   experimental response, but its heterogeneous solid-breeder system is not
   the FARIS blanket and validates only modeled nuclear responses within its
   own geometry/material/source domain.
3. **Design comparison:** only after the two above, calculate the two FARIS
   allocations. These outputs are design-level model predictions, not
   experimentally validated ARC performance. Experimental checks do not
   validate the whole tokamak, local penetration, homogenization, operating
   history, component failure limit, plant efficiency, or lifetime electricity.

The local open-benchmarks checkout contains ITER_1D OpenMC `geometry.xml`,
`materials.xml`, and source code, and HCPB_TBM_1D OpenMC geometry/materials.
It contains no ITER_1D or HCPB experimental-results directory; its available
experimental files include FNS, Oktavian, and TIARA cases. Preserve a pinned
copy/revision and hashes of any reference inputs actually used. The project
checkout describes the benchmark collection as work in progress and identifies
CC-BY-4.0 licensing; it is useful provenance, not a substitute for auditing the
benchmark specification and source experimental report.

## Evidence and present readiness

| Item | Baseline decision | Status / required evidence |
|---|---|---|
| Transport code | OpenMC continuous-energy fixed source; pin executable, API version, commit/build, package identity, and execution settings in each run | A working neighboring environment provides OpenMC 0.15.3 (commit `27e38e894697bb32a1dac7848d2618818b6b8daf`), Release build with GNU 14.3.0, MPI, parallel HDF5, and DAGMC. FARIS does not yet configure this environment; the machine-readable readiness audit records its package identity |
| Data family | Prefer the OpenMC-distributed IAEA FENDL-3.2 HDF5 library as a frozen initial candidate; do not silently substitute later FENDL revisions. Keep sensitivity studies explicit and versioned | The neighboring environment has an archive and extracted XML/HDF5 library whose inventory matches OpenMC's published FENDL-3.2 description (converted from IAEA-NDS ACE). The 253 XML file references resolve; the listed Li, Be, O, F, Fe, Cr, and Ni isotope files open and contain one 293.6 K dataset each. Hashes and nuclide/reaction coverage are in `references/openmc-fendl-readiness.json`. The local archive has no acquisition receipt or publisher checksum, and no data license statement was found alongside it. The OpenMC web page provides a download, but confirm that the local bytes came from that release and resolve redistribution terms before packaging |
| Data delivery | External local data path; record library version, source URL, each file/hash, processing route, temperatures, and required reactions in the run manifest | License/redistribution and exact library identity must be checked before packaging; OpenMC's official guide supports external HDF5 libraries and ACE conversion workflows |
| ITER_1D | JADE OpenMC 1.1 input, with corresponding MCNP/Serpent input versions 1.0 in the local benchmark metadata | Read inputs and reconcile expected responses before treating this as a regression reference; it has no experimental benchmark role |
| Shield experiment | FNG ITER bulk SS shield, SINBAD SBE 10.002 / NEA-1553/69 | Benchmark summary is public; complete downloadable experiment/input files and response-specific uncertainty tables must be acquired and pinned |
| Breeding experiment | FNG HCPB TBM (2005) SINBAD package | Published analyses report total uncertainty; use exact response-specific results from package/publication, not a single rounded statement |
| FARIS materials | No reactor composition/density/enrichment/temperature is frozen here | Must derive from a primary ARC/blanket design source or explicitly classify as a FARIS-authored scenario assumption; never copy benchmark materials into the plant model by implication |
| Source | D-T neutron source, physically tied to the stated fusion power only at the normalization boundary | Set energy/angular/spatial model from a cited plasma/source prescription; benchmark sources remain benchmark-specific and must not be reused as the tokamak source distribution |

IAEA describes FENDL as fusion-evaluated data developed and assessed against
integral experiments. That history is a reason to include it as the leading
candidate, not proof that any particular OpenMC conversion, response, or FARIS
composition is accurate. The OpenMC-distributed FENDL-3.2 data are listed at
293.6 K. The local target files contain a single `294K` dataset (`kT =
0.0253 eV`); this supports room-temperature benchmark reproduction only.
OpenMC's temperature interpolation needs data at bounding temperatures, so
elevated material temperatures require additional appropriately processed data
or a declared approximation supported by a separate uncertainty analysis. An
XML manifest existing on disk does not establish source provenance or
qualification.

## Benchmark claims and acceptance protocol

Keep validation claims response-specific. Record measured and calculated
quantities, location/volume, reaction/score, energy integration, source
normalization, experimental uncertainty, Monte Carlo uncertainty, nuclear-data
uncertainty (when available), and geometry/material uncertainty separately.
Compare like-for-like quantities and include covariance where the source report
provides it.

For scalar response *i*, report the calculation-to-experiment ratio and a
combined uncertainty using one consistent uncertainty convention. Convert
published expanded uncertainties to standard uncertainties only when their
coverage factor and distribution convention are known; otherwise compare on the
published expanded-uncertainty scale or state that a statistical compatibility
test cannot be formed. The predeclared comparison rule is consistency within
the combined expanded uncertainty, using the benchmark's stated coverage
convention:

`|C_i - E_i| <= k * sqrt(u_C,i^2 + u_E,i^2 - 2 Cov(C_i,E_i))`.

Use `k=2` only when inputs are standard (approximately 1-sigma) uncertainties
and the benchmark supports a roughly 95% expanded interval; otherwise use the
published coverage factor/convention. Monte Carlo sampling contributes to
`u_C`; nuclear-data covariance, geometry, composition and source uncertainties
must be included where quantifiable. Set covariance to zero only when the
calculation and measurement uncertainty components are defensibly independent
for that response. Shared source normalization, common nuclear data, shared
calibration, or other common-mode effects require covariance or a conservative
bound; unsupported correlation cannot be presumed independent. If uncertainty
is unavailable, or known correlation cannot be bounded, report the numeric
discrepancy and mark the uncertainty compatibility check `INCONCLUSIVE`. Do
not create an acceptance band from the observed discrepancy or set a universal
percent tolerance in advance of response-specific uncertainty extraction.

Passing an uncertainty compatibility check means the result is statistically
consistent with the experiment at the stated coverage level. It is one piece of
validation evidence, not proof of model qualification. Qualification of a
specific FARIS response additionally requires a defined applicability domain,
adequate benchmark relevance, checks for systematic bias and model-form error,
complete material/source/geometry uncertainty treatment, and independent
numerical verification. Preserve these as separate findings and scopes.

For response ratios/attenuation profiles with correlated source normalization,
preserve the correlation rather than treating every depth point as independent.
Assess a profile with the benchmark's covariance/chi-square method if supplied;
otherwise publish pointwise residuals and uncertainties without a global pass.
Monte Carlo precision is a numerical allocation target, not a validation
tolerance: production runs must have sampling uncertainty small enough that it
does not dominate the experimental comparison or obscure the allocation effect.
Set numeric per-response targets only after a pilot and report achieved
uncertainty; use independent seeds or batch estimates as a stability check.

### What each evidence class establishes

| Evidence class | Permitted claim | Does not establish |
|---|---|---|
| Analytic numerical controls | Correct source-rate conversion, reaction-energy arithmetic, units, simple geometry volumes, volume/strength normalization, and Rust ledger conservation in limiting cases | Nuclear data accuracy or reactor-design accuracy |
| Code-to-code benchmark (JADE ITER_1D) | Independent implementation/input agreement for identical modeled problem, after reconciling physics/data/settings | Agreement with nature; independence may be reduced by shared data and common modeling assumptions |
| Experimental benchmark (FNG SINBAD) | Agreement of specified response(s) for measured mock-up, conditional on experiment/model uncertainty and exact benchmark reproduction | Whole-device performance, dissimilar materials/geometries, blanket self-sufficiency, magnet life, or economic/operating projections |
| Design-level FARIS calculation | Conditional comparative consequence of the declared two FARIS inputs under the declared stationary transport and Rust assumptions | Qualified ARC design, lifetime, component survivability, thermal-hydraulic/stress safety, or tritium self-sufficiency in an engineered plant |

The documented HCPB mock-up tritium prediction uncertainty is reported as about
8–10% at 2-sigma including experimental, calculation-statistical, and nuclear
data components; a separate reported data-related contribution is about 4% at
2-sigma. These values are specific to that mock-up/data evaluation and cannot
be recycled as a universal FARIS TBR tolerance. That study reports a 5–10%
average underprediction; treat this as evidence that systematic bias can exist,
not as a correction factor for FARIS. No absolute FARIS WCLL TBR is qualified;
the neighboring fusion-energy-ledger WCLL comparison failed at +8.5% and is
excluded as a reference result.

## Response definitions and downstream boundary

Define and freeze each exact tally/metric before implementation. Minimum
transport outputs are per-source tritium production by breeder region and total,
breeder-integrated TBR numerator definition, neutron spectrum/flux for named
regions, and magnet-region response(s). Name the exact damage/exposure metric
and data response before calling it DPA, displacement damage, or a service
limit. If unavailable or not validated, show a transport proxy (for example,
grouped flux or an identified reaction rate) without renaming it exposure or
failure life. Heating must state neutron-only versus coupled neutron-photon
transport and the score's local-deposition assumptions; heating is not
temperature. OpenMC defines `H3-production` as a product score, `heating` via
MT 301, and `damage-energy` via MT 444. The local requested nuclide files
contain the MT 301/444 reaction groups, but presence of these groups does not
establish that a tally is supported, statistically useful, or valid for a
component limit. IAEA FENDL guidance recommends TENDL-2017 activation-library
MT 444 data for DPA rather than the FENDL transport-file damage-energy data; a
DPA/lifetime claim therefore needs a separately pinned damage response and
benchmark route. The FENDL photon files provide photoatomic interaction data,
while production and energy deposition still need a declared coupled-transport
or local-heating treatment.

OpenMC tally scores have score-specific units and per-source normalization.
Preserve raw tallies and normalize exactly once. Absolute neutron source rate
must be derived using the chosen fusion reaction energy convention and must not
equate fusion power with neutron power. Verify with an analytic unit control.
The Rust model may then consume identified physical rates and explicitly
assumed recovery/processing delays, losses, initial reserve, burn, outage and
replacement schedules. Those assumptions are scenario projections; they are
not validated by neutronics benchmarks. Electricity requires a separate
declared heat recovery fraction, conversion efficiency and auxiliary load.

## Unresolved prerequisites (do not invent)

Before the first FARIS production result, resolve and record: (1) exact reactor
material compositions, densities, isotope fractions/enrichment, temperatures,
coolant/void and homogenization; (2) D-T source geometry, energy/angular
distribution and fusion-power normalization; (3) evaluated library files and
OpenMC coverage for all materials, reactions and heating treatment; (4) the
penetration's location/shape/fill, affected volumes and no-feature control;
(5) response scores, averaging domains and uncertainty sources; (6) the
SINBAD package's accessible files, precise uncertainty conventions and
redistribution conditions; (7) initial fuel state, recovery/retention model,
service-limit sources and power/energy assumptions; and (8) actual runtime and
resource limits from a named machine. An unresolved input remains explicitly
unresolved in the generated study and blocks dependent physical claims.

## Primary references

- [IAEA FENDL benchmark sublibrary](https://www-nds.iaea.org/fendl20/fen-bench.htm) — fusion benchmark descriptions/data including OKTAVIAN breeding and neutron/gamma response benchmarks.
- [IAEA background on FENDL and its experimental validation](https://www-nds.iaea.org/fendl3/bg-infos.html).
- [OECD-NEA SINBAD: FNG Neutronics Bulk SS Shield Experiment, SBE 10.002](https://cms.oecd-nea.org/science/wprs/shielding/sinbad/FNG_BLKT/FNGBKT_A.HTM) — 94 cm mock-up, source specification, measurement methods and source uncertainty.
- [OECD-NEA SINBAD: FNG HCPB TBM mock-up](https://www.oecd-nea.org/science/wprs/shielding/sinbad/fng_hcpb/fnghcpb-a.htm).
- [Batistoni et al., HCPB mock-up sensitivity/uncertainty analysis](https://doi.org/10.1016/j.fusengdes.2007.08.007) — response-specific prediction and combined uncertainty evidence.
- [IAEA report INDC(NDS)-0631](https://www-nds.iaea.org/publications/indc/indc-nds-0631.pdf) — fusion neutronics experimental benchmark descriptions and measurement context.
- [IAEA-NDS/open-benchmarks repository](https://github.com/IAEA-NDS/open-benchmarks) and [JADE benchmark input conventions](https://github.com/IAEA-NDS/open-benchmarks/blob/main/jade_open_benchmarks/jade_benchmarks.md) — local ITER_1D/HCPB templates, per-code input versions, and CC-BY-4.0 repository licensing (verify local pinned revision before use).
- [OpenMC data configuration](https://docs.openmc.org/en/stable/usersguide/data.html) and [tallies](https://docs.openmc.org/en/stable/usersguide/tallies.html) — supported external data configuration, processing paths, score units, and normalization.
- [OpenMC distributed cross-section libraries](https://openmc.org/data/) — describes the downloadable FENDL-3.2 HDF5 library as converted from IAEA-NDS ACE data, with neutron data at 293.6 K and photoatomic data. This establishes the intended upstream library identity, not a byte-for-byte check of the local archive.
- [IAEA INDC(NDS)-0797](https://www-nds.iaea.org/publications/indc/indc-nds-0797.pdf) — FENDL-3.2 technical meeting recommendations for total KERMA MT 301 heating and using TENDL-2017 activation MT 444 data for DPA; pin the relevant evaluation and response before an exposure claim.
