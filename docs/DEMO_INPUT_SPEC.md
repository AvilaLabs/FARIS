# M1 cold-data demonstration input specification

**Status:** executable candidate for a bounded, conditional neutronics comparison.
It is an idealized ARC-inspired model with authored surrogate materials, not ARC
reproduction, blanket qualification, or reactor design validation. The
machine-readable input and targeted library audit are in
[`references/demo-input-spec.json`](../references/demo-input-spec.json); rerun
the read-only audit with [`audit_library.py`](../integrations/openmc/audit_library.py)
against an identified `cross_sections.xml` before preparing a transport job.

## Recommended candidate case

Use OpenMC 0.15.3 continuous-energy fixed-source transport with the cached local
FENDL-3.2 HDF5 candidate. Physical material temperatures are unspecified; the
physics case targets the numeric stored nuclear-data temperature nearest
`293.6 K`, which the audited files report as `293.59430848016336 K`. OpenMC
0.15.3 accesses that dataset through its rounded `294K` group label. The worker
checks actual kT against the target within 0.1 K, then uses a separate 1 K
OpenMC label-selection window because its runtime lookup uses the rounded label.
That is label handling, not thermal interpolation or an assertion that the
materials are at exactly 294 K. **The FLiBe-like salt is then solid Li2BeF4, not a liquid
operating blanket.** This deliberately narrow case is suitable for checking
that the Rust scenario, OpenMC export, source, atom densities, geometry regions,
reaction responses, unit conversion, and variant comparison work end to end.
The absolute results only describe the stated cold, idealized model.

Material recipe candidates are deliberately simple and buildable from available
data:

| Region | Candidate recipe and density | Status and limit |
| --- | --- | --- |
| Solid breeder-like salt | Li2BeF4 stoichiometry, 90 atom% Li-6 / 10 atom% Li-7; one Li2BeF4 formula unit has atom fractions Li-6 0.2571428571, Li-7 0.0285714286, Be-9 0.1428571429, F-19 0.5714285714. Set `rho = 2137.175 kg/m3` as an authored model assumption. | The 2025 X-ray density trend for **solid** Li2BeF4 was 2179.1(3) - 0.114(1) T_C kg/m3 at 7Li/6Li = 10500(500), with natural Be/F and BeF2 mole fraction 1/3 over 20–449°C. At 293.6 K, the published isotope-basis value is ~2176.8 kg/m3. The candidate value mass-reweights the formula unit to 90%-Li6 at fixed measured crystal-cell volume. It is not a measured 90%-Li6 density; isotope-related lattice change, batch chemistry, impurities, and uncertainty of this transfer are unquantified. The source measured a crystal, so it also does not establish a homogenized operating blanket bulk density. ARC separately tabulates 1940 kg/m3 at 950 K for FLiBe; its isotope/composition basis is not specified well enough to replace this cold-state model assumption. |
| First-wall armor | Natural-isotope elemental tungsten, `rho = 19,300 kg/m3`. | Pure dense W proxy, sourced at near-room temperature from NIST. Does not encode tile joints, coating, texture, impurities, or high-temperature expansion. ARC used 1 cm W in one design calculation; its thickness is a precedent, not this case's validated requirement. |
| Structure | Natural-isotope pure iron shell, `rho = 7,874 kg/m3`. | Deliberate composition surrogate for the ARC-named Inconel 718 structure; it is **not Inconel 718**. NIST's reported pure Fe density is 7.874 ±0.001 g/cm3. Alloying/ribbing/welds/structural qualification are excluded. |
| Separate neutron multiplier | Natural Be, represented by Be-9, `rho = 1,850 kg/m3`. | Dense elemental Be reference from NIST. The ARC paper's separate multiplier is a 1 cm non-structural layer; any implemented thickness must be explicitly identified as the selected FARIS model value. It is distinct from Be already present in the salt. |
| Neutron shield | Ideal stoichiometric TiH2, natural Ti and H isotope abundances, `rho = 3,750 kg/m3`. | Theoretical crystalline density from a primary powder-compaction study. ARC describes TiH2 powder; do not equate theoretical crystal density with actual packed-bed bulk density. No powder void, canister, or water channels are represented by this recipe. H/Ti stoichiometry and hydrogen retention are assumptions. |
| Magnet response surrogate | Natural-isotope pure Cu toroidal shell, `rho = 8,960 kg/m3`. | NIST selected-metal value at 295 K. It is an outer response tally region, not a REBCO coil, insulation package, structural winding, or failure model. Report transport fluence/damage-energy response in this surrogate only; never label it coil failure or service life. |

For elemental natural-abundance recipes, expand isotopes through the pinned
OpenMC 0.15.3 material-data API, then preserve the resulting nuclide vector in
the exported record. The expansion used by this candidate is enumerated in the
JSON. Do not depend on undocumented fallback when a library nuclide is absent.
FLiBe enrichment is explicit, not natural lithium. Li2BeF4 uses the sourced
2:1 LiF:BeF2 formula; elemental natural Be-9/F-19 assumptions remain explicit.

These recipes let M1 exercise every requested elemental family while keeping
the material names honest. They are intentionally not industrial Inconel,
manufactured tungsten armor, enriched-salt density assay, or compacted TiH2
shield specifications. Do not attach engineering tolerances to the authored
densities or compositions.

## Source and normalization

For a reproducible minimum source, use a uniform birth distribution over the
authored toroidal plasma volume, isotropic in direction and monoenergetic at
14.1 MeV per neutron. This is an explicit idealization, not an extracted ARC
source distribution. Use identical source phase space for both allocations.
Normalize per source neutron first; if absolute steady rates are displayed,
define the scenario's 525 MW as total D-T fusion power, assume 17.6 MeV released
per reaction and one source neutron per reaction, then derive

`source_rate = 525e6 W / (17.6e6 eV/reaction * 1.602176634e-19 J/eV)`
`= 1.8618137864e20 source neutrons/s`.

Retain per-source tallies and derivation alongside absolute rates. The 525 MW,
14.1 MeV, 17.6 MeV, uniform profile, isotropy, and steady operation are fixed
scenario assumptions for the comparison, not validated ARC operational inputs.
There is no time-averaged duty cycle or plasma-evolution model in this source.

## Supported and unavailable responses

The read-only audit confirms all requested neutron files are present, HDF5- and
OpenMC-readable, with 294 K data. It confirms MT=301 (`heating`) and MT=444
(`damage-energy`) exist for every inventoried target nuclide. The Li-6/7 files
contain derived MT=205 `(n,Xt)` data at 294 K; OpenMC's `H3-production` score
maps to that tritium-product reaction class. This is a dataset/code-path
readiness check, **not yet a run-level tally test or measured TBR validation**.
The eventual adapter must verify an actual nonzero score, nuclide attribution,
units and normalization before accepting breeding results.

The audited nuclide files also parse secondary-photon product records with
angle/energy distributions, and all requested photon-atomic files are present
and HDF5-readable. Coupled neutron-photon transport and energy deposition still
need a small real OpenMC run with a per-region energy balance check. In
particular, MT=901 (`heating-local`) is absent for all inventoried nuclides, so
the FENDL candidate cannot supply OpenMC's neutron-only local-heating response.
MT=301 neutron heating alone is not total deposited component heat. Do not
calculate thermal efficiency, plant electricity, or local total component
heating from it. If coupled neutron-photon heating is not implemented and
verified, report total nuclear heating as unavailable.

MT=444 supplies damage-energy response, not displacements-per-atom without a
declared displacement-energy model, and neither quantity by itself predicts
magnet/coil failure. Report the Cu response as “surrogate-region fluence” or
“surrogate-region damage-energy response” with tally volume/averaging, spectrum
cut, units, standard error and source normalization. Do not convert it to a
service limit unless an applicable component-specific threshold and uncertainty
model are separately established.

## Nuclear-data identity and limits

The candidate OpenMC installation is 0.15.3, commit
`27e38e894697bb32a1dac7848d2618818b6b8daf` (identified by the local runtime
readiness record). `references/demo-input-spec.json` records the cached
`cross_sections.xml` SHA-256 and per-target HDF5 file hashes, HDF5/openmc-data
readability, available temperature groups, reaction data and photon-data
inventory. The audit executable accepts a different explicit XML path and
requested nuclide set so it can be rerun against a future library without
copying data into this repository.

This audit proves only that the locally named files are readable and include
the reported datasets. It does not authenticate their FENDL release or
acquisition provenance, establish redistribution permission, or qualify
FENDL-3.2 for the response. The local package has no acquisition receipt or
verified publisher checksum in the existing audit; nuclear-data terms remain
unresolved. Keep these external heavy files outside Git and do not bundle them
until provenance/licensing are established.

All material and nuclear data are cold-data inputs. This case cannot be used as
the hot ARC-like design case: ARC reports flowing FLiBe around 800–900 K, whereas
the available target nuclear data here are one 294 K group. No temperature
interpolation, Doppler processing or extrapolation has been qualified for this
scenario. The cold solid-salt density, phase, and no-flow assumption are not a
substitute for liquid-salt thermophysical behavior.

## Claim boundary and next gates

This narrow case can support a conditional comparison of OpenMC transport
responses between two explicitly defined arrangements, under identical
authored materials, source, nuclear data and history. It cannot establish:

- ARC's geometry, source recipe, TBR, shield thickness, heating, or magnet life;
- plant blanket performance, self-sufficiency, temperature feedback or power;
- enriched molten-salt density or hot nuclear-data response;
- experimental validation or qualified prediction of a FARIS design;
- coil degradation, lifetime, replacement schedule, or net electricity.

Before treating even the cold comparison as ready, verify the exact region
atom-density export and voids, one real H3-production tally, MT301 neutron
heating semantics, photon-coupled heating if exposed, geometry/material volumes,
and per-source to absolute normalization. For later design claims, require
temperature-suitable nuclide data, material assays/densities at relevant
temperatures, a source model with applicability, response-specific benchmark
evidence, and independent uncertainty treatment. Missing inputs must stay
unresolved rather than be filled from the design paper's rounded totals.

## Primary sources

- Sorbom et al., [ARC paper](https://arxiv.org/abs/1409.3540), DOI
  [10.1016/j.fusengdes.2015.07.008](https://doi.org/10.1016/j.fusengdes.2015.07.008):
  materials, temperatures, model scope, source/material sensitivity, and
  simultaneous neutron+photon heating.
- Gardner et al., [solid Li2BeF4 density/structure study](https://doi.org/10.1107/S1600576725000548),
  Table 4: X-ray solid-salt density fit for Li7/Li6=10500(500) over 20–449 °C. Its isotope
  basis is unlike the 90% Li-6 candidate; our value is explicitly reweighted.
- NIST, [elemental W composition/density](https://physics.nist.gov/cgi-bin/Star/compos.pl?matno=074&mode=text&refer=ap)
  and [Standard Reference Database 71](https://www.nist.gov/document/srd71usersguidev1-2pdf),
  Table A.3 (Be, Ti, Fe, Cu elemental densities).
- NIST, [iron density determination](https://nvlpubs.nist.gov/nistpubs/jres/26/jresv26n1p1_a1b.pdf)
  reports 7.874 ± 0.001 g/cm3 for pure iron.
- NIST Center for Neutron Research, [selected metal properties at 295 K](https://www.nist.gov/ncnr/neutron-instruments/sample-environment/sample-mounting/reference-tables),
  reports copper 8.96 g/ml.
- Du et al., [TiH2 powder compaction study](https://www.mdpi.com/2075-4701/13/2/360),
  gives theoretical TiH2 density 3.75 g/cm3; this is a crystal-density reference,
  not shield bulk packing density.
- OpenMC 0.15, [material composition expansion](https://docs.openmc.org/en/stable/usersguide/materials.html),
  [reaction/production scores](https://docs.openmc.org/en/v0.15.0/usersguide/tallies.html),
  [heating](https://docs.openmc.org/en/stable/methods/energy_deposition.html),
  and [cross-section temperature treatment](https://docs.openmc.org/en/stable/methods/cross_sections.html).
- IAEA-NDS, [FENDL](https://www-nds.iaea.org/fendl/) description; local file
  hashes and inventory do not authenticate release or license.
