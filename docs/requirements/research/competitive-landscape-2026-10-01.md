# FARIS competitive landscape (2026-10-01)

Method: ~25 web searches and 8 page fetches on 2026-10-01. Public sources only. Cells are Yes / Partial / No / Unknown. "Unknown" means I found no public statement, not that the capability is absent. Several tools (PROCESS, FUSE, bluemira, FERMI, SuperMC) are large and I read only landing pages and papers, so "No" for a deep feature is "not documented in what I read". A few pages (ScienceDirect) returned 403, so I relied on search abstracts for those.

## Tool table
Columns: 3D MC neutronics | Fluence-driven replacements over plant life | Tritium inventory vs time | Net electricity vs time | Interactive GUI | Instant what-if | Uncertainty flagged | Provenance/verification | Project file | Licence

| Tool | 3D MC | Life/replace | T inventory | Net elec | GUI | Instant what-if | Uncert. | Provenance | Proj. file | Licence |
|---|---|---|---|---|---|---|---|---|---|---|
| PROCESS (UKAEA) | No (0D/1D) | Partial: fluence-based lifetimes -> availability factor, not time-resolved history ([doc](https://ukaea.github.io/PROCESS/eng-models/plant-availability/)) | No | Partial: steady-state net power ([doc](https://ukaea.github.io/PROCESS/)) | No (CLI/Python) | Partial: runs in seconds, no live UI | Partial (some UQ tooling, unverified) | Unknown | Input file | Open source |
| bluemira (UKAEA/KIT) | No (not mentioned) | Unknown | Partial: "simplified dynamic tritium fuel cycle model" ([docs](https://bluemira.readthedocs.io/en/latest/introduction.html)) | Unknown | No | No | Unknown | Unknown | Config files | LGPL-2.1+ ([GitHub](https://github.com/Fusion-Power-Plant-Framework/bluemira)) |
| FUSE (General Atomics) | Partial: reduced neutronics model plus link to external 3D MC ([arXiv](https://arxiv.org/html/2409.05894v1)) | Not documented | Not documented | Partial: power balance, time-dependent plasma sims | No (Julia) | Partial | Yes: uncertainty propagation workflows (same paper) | Unknown | Julia scripts | Apache-2.0 (open) |
| SYCOMORE (CEA) | No (reduced blanket/shield model, [paper](https://www.sciencedirect.com/science/article/abs/pii/S0920379614003354)) | Unknown | Unknown | Yes (power balance to net electric, [IOP](https://iopscience.iop.org/article/10.1088/0029-5515/55/7/073011)) | Unknown | Unknown | Unknown | Unknown | Unknown | Institutional (not confirmed open) |
| FRESCO | No | Unknown | No | Cost of electricity ([OSTI](https://www.osti.gov/etdeweb/biblio/22225843)) | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| ARIES, GASC, TREND, MIRA | Not assessed in depth | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Institutional |
| OpenMC + Paramak + DAGMC (fusion-energy org) | Yes ([workflow](https://github.com/fusion-energy/fusion_neutronics_workflow)) | No | No | No | No (Python/notebooks) | No (minutes per run) | Yes (MC standard error) | No | Scripts | Open source |
| FERMI (ORNL/ARPA-E) | Yes (OpenMC, MCNP, Shift) plus CFD, structures ([ORNL](https://www.ornl.gov/project/fermi)) | Partial: "structural lifetime" as a metric, no plant timeline | Unknown | Unknown | Unknown (Python API, ParaView) | No (HPC multiphysics) | Unknown | Unknown | Unknown | Mixed open and proprietary |
| SuperMC (FDS team) | Yes, CAD-based, with GUI ([paper](https://www.sciencedirect.com/science/article/pii/S0306454914004587)) | No | No | No | Yes (modelling and visualization GUI) | No | Yes (MC) | Unknown | Unknown | Restricted licence ([NEA](https://www.oecd-nea.org/tools/abstract/detail/iaea1437/)) |
| MCNP + Visual Editor / Moritz / McCad | Yes | No | No | No | Yes (geometry editors) ([RSICC](https://rsicc.ornl.gov/codes/psr/psr6/psr-618.html)) | No | Yes (MC) | No | Input decks | Restricted/commercial |
| Serpent, TRIPOLI, Shift, FISPACT-II, Attila, COMSOL | Not assessed in depth; general-purpose transport/activation/multiphysics | No | No | No | Varies | No | Varies | No | Varies | Mixed |
| PathSim/PathView tritium workflow | No (OpenMC listed as future work) | Partial: event-based maintenance/outages over days ([arXiv](https://arxiv.org/html/2603.25751v1)) | Yes (ARC example) | No | Yes (web GUI) | Partial (interactive modelling) | No (not addressed) | No | Yes (model graphs) | Open source, CC BY paper |
| TRICYS (OpenModelica) | No | Unknown | Yes ([GitHub](https://github.com/couuas/tricys)) | No | Partial | Partial (sensitivity analysis) | Partial | Report generation | Unknown | Open source |
| FFCSim (Fusion Fuel Cycles / Kyoto Fusioneering) | No | Unknown | Yes (dynamic fuel cycle, [paper](https://www.sciencedirect.com/science/article/pii/S0920379625003424)) | No | Unknown | Unknown | Unknown | Unknown | Unknown | Appears to be company tool; open status unclear |
| TMAP8 / FESTIM | No | No | Yes (transport in components) ([TMAP8](https://www.sciencedirect.com/science/article/pii/S0920379625000766)) | No | No | No | No | No | Inputs | Open source |
| Fusion availability/RAMI literature models (Taylor-Ward 1999; Morris 2015; 2025 component-lifetime paper) | No | Yes (analytical models) ([paper](https://www.sciencedirect.com/science/article/pii/S0920379625004272), abstract only) | No | No | No | n/a | Unknown | No | No | Papers / code inside PROCESS |
| UKAEA/Intel/Dell STEP digital twin | Unknown | Unknown | Unknown | Unknown | Partial ("industrial metaverse") ([UKAEA](https://www.ukaea.org/news/supercomputing-ai-and-the-industrial-metaverse-essential-for-uk-fusion-energy-powerplant-development/)) | Unknown | Unknown | Unknown | Unknown | Internal programme |
| Thea Energy Helios digital twin (NVIDIA/Synopsys-Ansys/ANL/PPPL) | Partial: Ansys blanket and ANL neutronics data ([ANS](https://www.ans.org/news/2026-06-17/article-8128/thea-energy-collaborates-with-ai-companies-to-develop-stellarator-digital-twin/)) | Unknown | Unknown | Unknown | Yes (Omniverse) | "Real-time analysis" claimed | Unknown | Unknown | Unknown | Internal-only |
| CFS / Tokamak Energy / Proxima / Type One | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Internal; public releases are plasma codes (TORAX, GSFit, VMEC++) |

Not found: any "fusion design platform" commercial product selling blanket/lifetime/tritium plant studies. Search phrase returned only the programs above. Ansys/Siemens/Dassault have fusion engagements (ITER electromagnetics, Thea blanket) but no off-the-shelf fusion plant-history product surfaced.

Related literature that does the same coupling by hand: Infinity Two (Type One) tritium cycle with OpenMC ([JPP](https://www.cambridge.org/core/journals/journal-of-plasma-physics/article/breeder-blanket-and-tritium-fuel-cycle-feasibility-of-the-infinity-two-fusion-pilot-plant/248C49CCA0B7ABEA2F7BF7031290EDC4)), ARC FLiBe/FESTIM tritium work, ARC original design ([arXiv 1409.3540](https://arxiv.org/pdf/1409.3540)), TMAP8 pilot-plant surrogate study ([arXiv](https://arxiv.org/html/2604.19647v1)). These are papers/scripts, not interactive tools.

## 1. Already well served
- Parametric geometry plus OpenMC TBR/heating/flux with MC error: Paramak + DAGMC + OpenMC, fusion-neutronics-workflow, FERMI, SuperMC, MCNP stack. 1M histories is routine.
- Whole-plant 0D/1D design with net electricity and cost: PROCESS, SYCOMORE, FUSE, bluemira, FRESCO.
- Fluence-limited component lifetimes feeding availability: PROCESS (Morris model), plus the RAMI literature. Analytical, not time-resolved.
- Dynamic tritium fuel cycle: FFCSim, TRICYS, PathSim/PathView, bluemira (simplified), TMAP8/FESTIM. These are more detailed than FARIS (processing chain, hold-up, permeation).
- Uncertainty propagation in design frameworks: FUSE.
- 3D neutronics GUIs: SuperMC and MCNP editors (geometry-centred, not plant-history).
- Interactive web model-building for fuel cycle: PathView.
- Open source and local: almost everything above.

## 2. Combinations that look unoffered (confidence)
a) 3D MC results directly driving a year-by-year operating history (fluence-limited replacements with outages, tritium inventory, net electricity) in a single tool, then painted back onto the 3D model by timeline. Confidence moderate-high (about 75%) that no public tool does this end to end. Evidence of absence: no hit in searches for fusion digital twin, fusion design platform, RAMI/AvailSim, fuel cycle simulators, FUSE/bluemira/PROCESS docs. Every nearest neighbour does one slice: PROCESS aggregates lifetimes to a factor, fuel-cycle tools have no neutronics, neutronics tools stop at TBR/heating. Caveat: internal company tools (CFS, Thea, STEP twin, Kyoto Fusioneering) are unknowable.
b) Interactive instant re-run of the whole multi-decade history from sliders on top of precomputed transport. Confidence about 70%. The closest are PathView and PROCESS, which have no precomputed-MC coupling.
c) Per-number kind labelling (calculated / assumption / literature / conditional / not evaluated) with 2σ-aware delta flagging in a compare view and a generated takeaway. Confidence about 80% not offered as a standard feature in fusion tools. I did not search other industries.
d) Hash-based verification receipts plus a reopenable project file for a fusion plant study. Confidence about 85% (nothing found). Generic workflow tools (provenance, notebooks) exist, but I found none in fusion design.
e) Native local desktop 3D viewer for MC results on a plant radial build (no Python or HPC): partial novelty. ParaView and SuperMC viewers exist for results; plant-history linkage is the novel part.

## 3. Closest competitor
FUSE (General Atomics). It is open (Apache-2.0), plant-level, has net electricity, a reduced neutronics model with an optional bridge to external 3D MC, and built-in uncertainty propagation, plus costing. It has no GUI and, in what I read, no lifetime or tritium history. Runner-up on the workflow side is PROCESS (the Morris availability model) for the replacement logic, and PathView/FFCSim for the tritium side. Overall no single tool covers more than about 3 of FARIS's 10 columns.

## 4. What a fusion engineer would call missing or naive
- Geometry is idealized (concentric radial build); no real CAD, ports, segmentation, penetrations, divertor, or streaming paths. Port effect from one outboard port is a 1D/2D-style stand-in; real studies use CAD to DAGMC with full 3D source (Paramak/DAGMC/McCad are standard).
- Only a 14 MeV D-T neutron source in a simplified plasma profile; no real plasma source distribution (OpenMC fusion source), no burn-up/depletion of Li-6 over the 30 years, no blanket Li-6 enrichment optimisation or tritium breeding feedback (breeding changes as the blanket burns up).
- Damage: fluence is not dpa. Engineers use dpa, He/H appm (transmutation), displacement damage for steels and REBCO, with per-material cross-sections. One REBCO limit at 3e22 n/m² (and its energy threshold, e.g. >0.1 MeV) is a single literature number with strong disputes.
- No activation, decay heat, shutdown dose rate, or waste classification (FISPACT-II, D1S/R2S standard) which dictate maintenance times and port access.
- No thermal-hydraulics, tritium permeation/hold-up in blanket and FLiBe chemistry (TMAP8/FESTIM level), no balance-of-plant thermodynamic model beyond an efficiency number, no recirculating power model (heating, cryo, current drive) as in PROCESS.
- Maintenance: replacement durations as a slider, no RAMI (failure rates, spares, unplanned outages, remote-handling constraints); availability is deterministic.
- No cost model (CAPEX/OPEX/LCOE; the fusion costing standard in PROCESS/bluemira work), no plasma physics consistency (power, Q, density limits).
- Validation: no benchmark against FNG/ITER benchmark cases; uncertainty covers MC statistical error only, not nuclear data, geometry, or model-form uncertainty. 1M histories gives coarse mesh statistics near magnets.
- Single design (ARC-inspired); no design optimisation or sweeps beyond one allocation parameter.

## Honest bottom line
The novelty is integration and presentation (3D MC to operating history to recolouring, instant sliders, provenance), not any single physics model, where existing tools are deeper. The claim should be phrased as "no public tool I found links these in one interactive, local, auditable workflow", with confidence limited by invisible internal tools.

## Search log (abbreviated)
fusion design platform / digital twin; PROCESS availability; FUSE; SuperMC GUI/digital twin; RAMI/AvailSim; Kyoto Fusioneering/FFCSim; UKAEA IBM STEP twin; bluemira; Paramak/OpenMC workflow; ARC FLiBe REBCO; SYCOMORE/TREND/FRESCO; OpenMC GUI; FERMI; Thea Helios; MCNP GUIs; coupled neutronics-tritium-availability pilot plant studies; company in-house codes; fusion software startups. Not searched: MIRA, GASC, ARIES internals, Dassault/Siemens specifics, Chinese CFETR/FDS digital-twin products, ITER-internal RAMI tools.

## Correction, 2026-10-06: bluemira

The bluemira row above understates it. Its documentation shows:

- A `LifeCycle` module that generates operating timelines with component replacement driven by damage and fluence
  limits (the EU-DEMO fuel-cycle example sets `"blk_1_dpa": 20` and `"tf_fluence": 3.2e21`), availability that
  improves along a learning curve (`GompertzLearningStrategy`), random outages (`LogNormalAvailabilityStrategy`),
  and 50 Monte Carlo timelines feeding a dynamic tritium fuel-cycle model that reports start-up inventory and
  doubling time ([example](https://bluemira.readthedocs.io/en/latest/examples/fuel_cycle/EUDEMO_fuelcycle.html);
  Coleman et al. 2019, Fusion Eng. Des.).
- OpenMC transport on its own geometry, CSG or DAGMC from equilibrium-based CAD, with a tokamak plasma source
  ([CAD neutronics example](https://bluemira.readthedocs.io/en/latest/examples/radiation_transport/run_cad_neutronics.html)).

So fluence-limited replacement over plant life and tritium inventory versus time are offered (Yes, stochastic), and
3D MC transport is offered. What remains unoffered in what we read: the timeline driven by the transport results
of the same model with their covariance, an interactive desktop with instant reruns, and hash-bound receipts. The
section 2(a) claim must be narrowed to that coupling, and the 0.2 geometry plan would trail bluemira's CAD path.
