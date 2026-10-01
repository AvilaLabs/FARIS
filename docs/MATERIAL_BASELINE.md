# M1 material baseline

**Status:** recommended candidate inputs for implementation; not a qualified
reactor material specification. This file separates what the ARC publication
reports from what FARIS still has to choose. The current scene's 1.20 m radial
envelope is an authored comparison boundary; it is not ARC's detailed radial
build.

## Recommendation

Represent material recipes independently from geometry and from display names.
For an ARC-inspired study, support these distinct regions: tungsten plasma-facing
armor; Inconel 718 structural walls; an FLiBe channel and bulk liquid blanket;
a separate solid beryllium multiplier; and a TiH2 shield. Use stoichiometric
Li2BeF4 with Li enriched to 90 atom% Li-6 as the **ARC-derived composition
candidate**. Keep the beryllium multiplier separate from Be already in FLiBe.
ARC's paper describes that distinction explicitly. Do not merge the blanket and
multiplier into one homogeneous region unless that is a deliberate alternative
case.

Do not yet encode exact scenario densities for all regions. ARC supplies useful
thermal-state and thickness precedents, but not a complete bill of materials.
The local FENDL-3.2 HDF5 audit covers the target blanket and Fe/Cr/Ni isotopes at
294 K only; tungsten, titanium, and hydrogen isotope coverage still needs a
targeted audit. Therefore choose one of two clearly named transport modes:

1. **Cold-data numerical reference:** use the available 293.6 K nuclear data
   with explicitly authored densities and geometry solely to verify the adapter.
   This does not describe thermodynamic material states: FLiBe is not a liquid
   at 293.6 K. It is not an operating-blanket model or an experimental reference.
2. **Hot ARC-inspired production case:** use documented hot FLiBe operating
   states (ARC reports 800 K inlet / 900 K outlet and a predicted <1000 K peak)
   only after finding or processing and qualifying suitable temperature data
   for every required nuclide and response. Record actual material temperatures
   per region. Do not silently substitute room-temperature cross sections or
   an unverified interpolation/broadening option.

For the first functional comparison, select an openly documented minimum recipe
once the outstanding values below are resolved. A sensible minimal first pass
is an explicit homogeneous Li2BeF4 blanket plus separate armor, structural wall,
multiplier, and shield cells. It omits ARC ribs, ports, the divertor, and
plasma-facing heat transfer. State those exclusions beside the model; TBR is
particularly sensitive to intervening material and first-wall specification.

## Published ARC facts versus FARIS choices

The ARC design paper is Sorbom et al., *ARC: A compact, high-field, fusion
nuclear science facility and demonstration power plant with demountable
magnets*, arXiv:1409.3540 (journal DOI
[10.1016/j.fusengdes.2015.07.008](https://doi.org/10.1016/j.fusengdes.2015.07.008)).
ARC is a design study, not an experimental qualification dataset. Its paper reports an
axisymmetric MCNP model and states that the particular first-wall/divertor
configuration was one of several possible experimental configurations.

Published ARC precedents relevant to material setup:

- First wall: 1 cm tungsten armor over a 1 cm structural Inconel 718 layer.
- Double-wall vessel: a 2 cm FLiBe channel, separate 1 cm non-structural
  beryllium multiplier, and 3 cm outer Inconel 718 wall. The inner wall is
  1 cm in the detailed thermal-analysis description. This geometry is not
  equivalent to merely splitting FARIS's blanket/shield allocation.
- Bulk blanket: FLiBe with 90% Li-6 by lithium isotopic abundance. ARC reports
  800 K inlet, 900 K outlet and peak below 1000 K from its thermal calculation.
  The blanket is flowing liquid and the tank is immersed in it.
- Shield: TiH2 around the blanket tank, with more thickness on the inboard
  side. The paper's cost table calls it powder; the compact design does not
  provide a FARIS powder packing fraction.
- Neutron and photon heating: ARC explicitly includes both. Its Table 6 is a
  design calculation, not a FARIS acceptance value.
- Divertor: neutronics used a deliberately conservative 8 cm tungsten layer on
  17% of the lower vessel surface. The authors say the actual design was
  open-ended and likely thinner. Do not represent this as a validated divertor.

FARIS choices, not facts from ARC: using the ARC enrichment in a different
geometry; treating FLiBe as homogeneous; omitting support columns/ribs/ports;
assigning compositions/densities not supplied by the paper; and using the
current two-allocation envelope. The ARC TBR >= 1.1 is not a blanket-material
threshold transferable to the FARIS geometry or library.

## Candidate recipes and unresolved fields

The machine-readable companion `references/material-candidates.json` gives the
same decisions and nulls in structured form. Composition fractions there are
atom fractions unless the field explicitly says weight percent.

| Region | Candidate representation | Sourced values | Must resolve before a physical case |
| --- | --- | --- | --- |
| FLiBe channel and blanket | Liquid Li2BeF4 = 2 LiF + 1 BeF2; candidate Li isotope atom fractions Li-6 0.90 / Li-7 0.10. Stoichiometry then gives atom fractions Li-6 0.2571428571, Li-7 0.0285714286, Be-9 0.1428571429, F-19 0.5714285714 **if** the elemental-isotope assumptions are declared. | ARC composition and 800/900 K channel/blanket states. Vidrio et al. measured density of a near-natural-lithium composition only from 447–820 °C; a later primary diffraction paper reproduces the measured fit `rho [kg/m3] = 2245(7) - 0.424(17) T[°C]` for x(BeF2)=0.3359(5), Li7/Li6=13.544(4). | Exact enrichment assay basis; Be/F isotope recipe; enriched-salt density vs temperature; isotope-appropriate temperature cross sections; impurity chemistry; bulk void/gas fraction, flow/thermal gradient, and whether channel/blanket have distinct densities or temperatures. Do not apply the near-natural fit as an exact 90%-Li6 density. |
| Armor | Pure tungsten candidate, separate from structural wall. | ARC's 1 cm reference thickness, not a complete grade/density specification. | Purity/alloy, isotope expansion rule, density/porosity, temperature and thermal expansion, required W isotope data and energy responses. |
| Inner/outer structure | Inconel 718 as a named alloy; transport recipe must be elemental/isotopic fractions. | ARC names 1 cm inner and 3 cm outer wall in its detailed thermal configuration. Special Metals gives limiting chemistry in weight-% and typical annealed/aged density 0.296/0.297 lb/in3 (about 8.20/8.22 Mg/m3). | Select a documented nominal heat chemistry (or explicit chemistry within ranges); partition Nb/Ta in “Nb+Ta”; Fe balance; trace elements; convert selected weight-% to nuclide atom fractions; wrought density and porosity; thermomechanical state and temperature. Manufacturer table says values are typical and composition/condition dependent. |
| Multiplier | Separate solid Be candidate, nominally Be-9. | ARC's non-structural 1 cm layer; Be-9(n,2n) is identified as multiplying/moderating neutrons. | Exact purity/isotope content, grade, density, porosity/joints, temperature, swelling/irradiation state and data coverage. Do not infer solid density from FLiBe. |
| Shield | TiH2 candidate, separate from any thermal insulation/coolant. | ARC identifies TiH2 and says shield includes powder; its mass and volume table imply ~3.76 Mg/m3 on rounded totals, which is not a defensible packed-bed bulk density or specification. | TiH2 stoichiometry/hydrogen-to-titanium ratio, phase, enriched/natural H/Ti isotope inventory, crystalline versus tapped/bulk density, porosity, temperature/dehydrogenation assumptions, containment/void and H/Ti data. The ARC reference shield's water cooling is a separate thermal system, not automatically material inside the transport cell. |
| Magnet/outer regions | Keep outside material definition unless transport geometry includes them. | ARC uses REBCO conductor and reports shield/coil response. | Explicit solids, voids, insulation, support steel/copper and representative homogenization are needed before calling magnet fluence/heating a magnet response. A point detector or coarse “coil” material is not a coil damage model. |

Elemental natural-abundance expansion is not itself a complete material
specification: record the isotope vector actually passed to OpenMC so enrichment,
alloy balance, and isotope assumptions are replayable. For Li2BeF4 arithmetic,
the listed formula fractions use 2 Li atoms, 1 Be, and 4 F atoms per formula
unit; assigning Be-9-only and F-19-only remains an explicit implementation
assumption pending a selected isotope convention.

## Source, response and heating scope

ARC plasma physics is an I-mode design point, but that does not define a neutron
source distribution. The ARC paper describes MCNP transport on a simplified
axisymmetric model. FARIS must define its own source phase space: D-T energy
distribution (monoenergetic 14.1 MeV is a common idealization, but not to be
claimed here as ARC's exact source), angular law, spatial birth density / plasma
profile, and normalization (per source neutron or to fusion rate). The same
source must be held fixed for the two blanket/shield allocations. If source rate
is converted from fusion power, state the D-T energy convention and account for
one source neutron per D-T reaction; do not use the 525 MW scenario value as
source intensity without making that derivation explicit.

For a design-level energy result, neutron-only deposition omits photon energy
transport from capture/inelastic events. ARC states neutron and photon heating
are both needed for accurate component heating. Thus either implement coupled
neutron/photon transport and score response with documented energy-deposition
semantics, or mark total component nuclear heating unavailable. The demo can
still present verified neutron responses (e.g. tritium production, fast fluence)
without calling them total heating.

## Nuclear-data and readiness gate

The local candidate is OpenMC 0.15.3 / FENDL-3.2 HDF5; the read-only audit is
`references/openmc-fendl-readiness.json`. That audit identifies Li-6/7, Be-9,
O-16, F-19, Fe/Cr/Ni isotopes and photoatomic O/F/Fe/Cr/Ni/Li/Be files; audited
neutron targets have only the 294 K data set. This is a file-coverage/readability
audit, not FENDL qualification, acquisition provenance, redistribution
permission, or a proof that the intended OpenMC scores are supported. It did not
audit the additional W, Ti, or H nuclides implied by the ARC candidate recipe.
Before enabling the candidate model, expand the inventory to actual atom
fractions after alloy selection, inspect each HDF5 dataset and relevant
reaction/Q-value behavior (including Li tritium production), and record all
response/temperature capability. Photon heating needs both suitable neutron
secondary photon production and photon interaction data; presence of
photoatomic files alone is insufficient.

OpenMC documents temperature selection/interpolation and windowed multipole
handling; neither makes unsupported target data exist. Use exact supported data
temperatures or explicitly document and verify any generation/processing path.
Hot blanket data and high-temperature structural data are prerequisites for
physical hot-case claims. A room-temperature reference case is useful to check
the adapter, source, geometry, and scoring while those prerequisites remain
open, but its absolute hot design results are not meaningful.

## Machine-facing material schema recommendation

Before a recipe can be used in transport, the shared scenario should preserve:

- stable material ID and region IDs, phase/state and whether recipe is a
  literature input, assay, or explicit assumption;
- nuclide atom fractions after isotope expansion (and original elemental or
  weight-basis recipe plus conversion provenance); enrichment basis;
- bulk density with units, density basis (crystal, solid part, or homogenized
  bulk), reference temperature, and porosity/void fraction separately;
- material temperature/state and the nuclear-data temperature actually loaded;
- source document/version, URL, relevant table/page, extraction note, and
  unresolved fields; data-library identity remains attached to the run rather
  than folded into material identity.

Unknown values must remain unknown in the authoring model. A run may become
ready only after every required material and nuclear-data input has a concrete
value and provenance; guessed defaults should be visible assumptions rather
than hidden values.

## Primary references

1. Sorbom et al. 2015, ARC paper, [arXiv:1409.3540](https://arxiv.org/abs/1409.3540),
   DOI [10.1016/j.fusengdes.2015.07.008](https://doi.org/10.1016/j.fusengdes.2015.07.008).
2. Vidrio et al. 2022, “Density and Thermal Expansivity of Molten 2LiF-BeF2
   (FLiBe): Measurements and Uncertainty Quantification,” DOI
   [10.1021/acs.jced.2c00212](https://doi.org/10.1021/acs.jced.2c00212).
3. Gardner et al. 2025, primary neutron/X-ray study comparing its solid measurements with Vidrio's liquid-density data
   [Solid structure of Li2BeF4 (FLiBe) from room temperature to melting studied by neutron and X-ray diffraction](https://doi.org/10.1107/S1600576725000548).
4. Special Metals, [INCONEL alloy 718 technical bulletin](https://www.specialmetals.com/documents/technical-bulletins/inconel/inconel-alloy-718.pdf),
   Tables 1–2. Manufacturer limits and typical density only; not a heat analysis.
5. OpenMC, [continuous-energy data and temperature treatment](https://docs.openmc.org/en/stable/methods/cross_sections.html).
6. IAEA-NDS, [FENDL library page](https://www-nds.iaea.org/fendl/) and local
   package identity/coverage audit in `references/openmc-fendl-readiness.json`.
