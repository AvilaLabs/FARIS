# R2: Plant engineering domains beyond neutronics (FARIS requirements research, 2026-10-05)

Provenance note (updated after coordinator reminder). A first draft was written from recall; a verification pass then checked the items marked [V] below via live search/fetch (IOP and ScienceDirect pages are bot-blocked, so several [V] rest on abstracts/search summaries, with the URL given). Anything still tagged [A] (approximate, from recall) or [U] (unverified) was NOT checked and must not become a hard requirement without checking. Every number is tagged:
[V-lit] = I am confident of the figure and citation (still re-check before quoting externally);
[A] = approximate/recalled, verify before it becomes a hard requirement;
[U] = I could not establish a number, or the item is unverified (tool/acronym existence or content uncertain).
URLs given are canonical landing pages or DOIs I am reasonably sure of; DOIs marked "(DOI unverified)" must be checked. Nothing here was contacted or signed up for.

---------------------------------------------------------------------------------------------------
## 1. Best-in-class landscape

### 1.1 Systems codes (what the reference tools model)

| Code | Owner / licence | What it covers | Notes and source |
|---|---|---|---|
| PROCESS | UKAEA, open source (MIT-style; github.com/ukaea/PROCESS) | 0D plasma (confinement scalings, L-H power threshold, bootstrap, beta limits, density limits, impurity radiation), TF/CS/PF sizing (LTS and HTS options), blanket+shield build, divertor heat load (simplified), heat transport / primary and secondary thermal cycle, power balance (gross/net/recirculating), cost model (1990-based Sheffield-style model and 2015 update, STEP-oriented model), availability model (planned/unplanned, replacement durations), pulsed (CS flux swing, dwell) and steady state. Optimiser: VMCON (SQP) with figure of merit (e.g., major radius, capital cost, COE) and ~dozens of iteration variables and constraint equations. | Kovari et al., Fusion Eng. Des. 89 (2014) 3054, DOI 10.1016/j.fusengdes.2014.09.018 [V-lit]; Kovari et al., 'PROCESS: A systems code for fusion power plants, Part 2: Engineering', FED 104 (2016) [V: https://www.researchgate.net/publication/292680005]; PROCESS COE scales ~ A^-0.6 with availability A [V via search, Kovari]; VMCON use is [A]. Python-wrapped and with a regression test suite and tracked "input file -> solution" benchmarks. |
| FUSE (Fusion Synthesis Engine) | General Atomics, Julia, open source (fuse.help) | Integrated actors (ActorFluxMatcher with TGLF/QLKNN transport, equilibrium (FRESCO is the GA free-boundary equilibrium solver inside FUSE), pedestal, current drive, neutronics (simple), blanket/TF/CS stress, balance of plant, costing with ARIES-style accounts, optimisation via NSGA-II-like multi-objective). 1.5D steady-state plasma plus 0D plant. | Meneghini et al. (Sept 2024), arXiv:2409.05894 [V: https://arxiv.org/abs/2409.05894]; Julia, Apache 2.0, first-principle+ML+reduced models, steady-state to time-dependent, multi-objective optimisation [V]. Specific actor list (TGLF/QLKNN, NSGA-II) is [A]. Note: "FRESCO" in GA context = free-boundary equilibrium (Meneghini); there may be another unrelated code of the same acronym [U]. |
| bluemira | Fusion Power Plant Framework, open source (github.com/Fusion-Power-Plant-Framework/bluemira, LGPL) | Python framework: 3D-aware parametric geometry (CAD via OCC), equilibria (free boundary, coil optimisation), first-wall profile and heat-flux (field-line tracing, Eich-based), radial build, neutronics hooks (OpenMC/PROCESS coupling), balance of plant (EU DEMO-oriented) , fuel cycle (tritium flow model), maintenance/ports layout. | Created from BLUEPRINT (Coleman and McIntosh, FED 139 (2019) 26-38) and MIRA (Franza et al., Nucl. Fusion 62 (2022) 076042, 'MIRA: a multi-physics approach to designing a fusion power plant'); repo github.com/Fusion-Power-Plant-Framework/bluemira, LGPL-2.1 [V]. Directly relevant as a Python-side peer for FARIS adapters. |
| SYCOMORE | CEA (France) | Modular system code with plasma (METIS-like 0D/1D), TF/PF, blanket, power conversion, costing; used for EU DEMO and French pilot plant studies. | Reux et al., Nucl. Fusion 55 (2015) 073011, DOI 10.1088/0029-5515/55/7/073011 [V-lit]. |
| ARIES systems code | UCSD/GA/PPPL (Najmabadi, Kessel) | Systems code with ARIES cost accounts (Account 20 land through 26 heat rejection; 22 reactor plant equipment; 23 turbine; 24 electric; 25 misc; 26 heat rejection), COE, availability, replacement-cost accounting, ARIES-AT, ARIES-ACT1/ACT2 studies. | Najmabadi et al., FED 80 (2006) ARIES-AT [A]; Kessel et al., Fusion Sci. Technol. 67 (2015) 1 (ARIES-ACT) [A]. Cost basis ARIES-AT ~5 c/kWh (1992-style $) [A]; ACT1 higher [U]. |
| GASC | General Atomics Systems Code | Predecessor-style tokamak systems code used for FNSF/pilot design (Stambaugh et al., Fusion Sci. Technol. 59 (2011) 279 [A]). | Largely superseded by FUSE at GA. |
| MIRA | UKAEA multi-physics design code (Franza et al., NF 62 076042, 2022), merged with BLUEPRINT into bluemira [V]. | | |
| TREND | Could not establish owner/scope [U]; do not cite. | | |
| Others worth citing | SYCOMORE peers: PROCESS, "TOKSYS"? [U]; Chinese CFETR systems code; Japan "SlimCS" (Tobita et al., FED 2009 [A]); EU "DEMO PROCESS baseline 2018" (Federici et al., Nucl. Fusion 59 (2019) 066013, DOI 10.1088/1741-4326/ab1178 [A]). | | |

Common feature checklist across best-in-class systems codes (basis for rows below): plasma 0D/1.5D; radial build with TF/CS stress checks; power balance with a documented recirculating-power list; cost accounts with escalation, contingency, IDC; availability with replacement schedule; constraint/optimisation with a reproducible solution record; documented validation vs ITER/DEMO/ARC/ARIES published points. Gap in all of them that FARIS can occupy: coupling to real transport (OpenMC/ACTINV), uncertainty carried end to end, and receipts. PROCESS/FUSE do not do Monte Carlo transport.

### 1.2 Tritium fuel cycle

- Abdou et al., Nucl. Fusion 61 (2021) 013001, DOI 10.1088/1741-4326/abbf35 [V: https://ui.adsabs.harvard.edu/abs/2021NucFu..61a3001A/abstract; figures via search summary of abstract/talk]: eta_f*f_b > 2% and processing time 1-4 h needed for self-sufficiency with reasonable confidence [V]; self-sufficiency impossible if availability factor < 10% for any eta_f*f_b, possible if AF > 30% and 1% <= eta_f*f_b <= 2%, reasonable confidence if AF > 50% and eta_f*f_b > 2% [V]; ITER burn fraction ~0.36% with 50-50 DT mix, 1.8-3.6% with tritium-only core pellets [V]; baseline eta_f 60% (25-70%), f_b 5% (1-10%), TBR 1.15 (1.1-1.25) [V]. Startup inventory (3000 MW plant) per Abdou: <7 kg if f_b*eta_f > 5% and processing time < 2 h; >20 kg if f_b*eta_f < 1% and processing > 6 h; ~11 kg in some mid cases, <5 kg optimised [V via search summaries of Abdou talks/papers, e.g. https://www.osti.gov/biblio/1787501; primary figure/page not read]; doubling-time windows computed for 1, 5, 10 years [V qualitative]; specific doubling-time figures [U]. Tritium decay 5.47%/yr follows from t1/2 12.32 y [derived]. The earlier recalled '1.05-1.15 required TBR' is superseded: the paper's baseline TBR is 1.15 with range 1.1-1.25.
- Meschini, Ferry, Delaporte-Mathurin, Whyte, Nucl. Fusion 63 (2023) 126005, DOI 10.1088/1741-4326/acf3fc [V] (https://www.osti.gov/pages/biblio/2000085-modeling-analysis-tritium-fuel-cycle-arc-step-class-fusion-power-plants): time-dependent system-level model in Matlab Simulink. Result [V]: required TBR achievable (TBR_r < 1.2) if tritium burn efficiency (TBE) reaches 0.5-1%; FPP must reach availability > 70%, tritium processing time < 4 h, and direct internal recycling (DIR). Startup-inventory kg for Meschini: not retrievable from open abstract [U]. Open related work: Malone et al., FED 2024 (https://www.osti.gov/pages/biblio/2403075-approach-startup-inventory-viable-commercial-fusion-power-plant): 500 MWth plant burns 77 g/day T; baseline startup inventory ~327 g plus ~642 g operating reserve (24 h DIR-only); inputs eta_f 60%, f_b 5%, TBR 1.15, DIR 75%, reserve 24 h (6-48) [V via search; note 'baseline 327 g' appears to be the startup excluding some terms: verify definition before use].
- Other: EUROfusion dynamic tritium plant modelling (Day and Giegerich, FED 88 (2013) 616 [A]); Kuan and Abdou (1999) [A]; Hörstensmeyer/Pearson tritium cycle models [U]; FFCSim = Fusion Fuel Cycle Simulator, Kyoto Fusioneering with Canadian Nuclear Laboratories / Fusion Fuel Cycles Inc., modular dynamic physics-based closed-loop simulator (ScienceDirect S0920379625003424) [V]; TRICYS = open-source OpenModelica-based tritium cycle simulator with parameter scans, SALib sensitivity, plant-wide mass-conservation audit (github.com/couuas/tricys) [V]. Hydrogen transport at component level: FESTIM (Delaporte-Mathurin et al., open source FEniCS-based, Int. J. Hydrogen Energy 2023 [A]); TMAP7/TMAP8 (INL, open source) [A]; permeation barrier and trapping physics (McNabb-Foster) are the fidelity ceiling.
- Best-in-class fidelity today: system-level compartment ODE model with residence times, hold-up, fractional losses, radioactive decay, and a startup/reserve inventory; component-level permeation (PbLi, steel, W) at FEM level handled outside the system model. No tool couples component-level FEM permeation to a lifetime plant model in one verified, UQ-aware chain: opportunity for FARIS.
- ITER site tritium limit < 4 kg and in-vessel limit 1 kg (700 g for PFCs after allowances) [V via search; primary page not deep-linked: see https://www.sciencedirect.com/science/article/abs/pii/S092037960500178X].

### 1.3 Materials and damage

| Item | Reference value | Source / status |
|---|---|---|
| EUROFER97 / RAFM structural dpa | EU DEMO: starter blanket ~20 dpa (NRT), later up to ~50 dpa, aspiration >100 dpa; qualification only to ~ few tens of dpa in fission irradiation | Federici et al., 2019 [A]; Gilbert et al., Nucl. Fusion 2012 [A] |
| RAFM operating window | ~350-550 C (lower bound embrittlement, DBTT shift; upper bound creep) | Gaganidze/Zinkle reviews [A] |
| He limit for weldability (steels) | ~1 appm He (conservative) for conventional fusion welding; higher with special techniques (laser/ low-heat) | Kurtz et al.; widely cited [A] |
| He in RAFM | ~ 10-12 appm/dpa at fusion spectra (10x fission) | [A] |
| W | recrystallisation/embrittlement; neutron-induced Re/Os transmutation; limit order 5-10 dpa-ish before significant embrittlement, fluence limit not settled | [U] |
| CuCrZr heat sink | ITER ~ few dpa; DEMO divertor limit ~5 dpa target (hardening, loss of ductility and conductivity) | Federici et al., 2019 [A] |
| SiC/SiC | ~tens of dpa, thermal conductivity degradation, swelling at <1000 C | Katoh/Snead [A] |
| ODS steels | ~ >100 dpa aspiration, limited data | [U] |
| REBCO | Fischer et al. (2018) SuST, 'The effect of fast neutron irradiation on the superconducting properties of REBCO coated conductors with and without artificial pinning centers' [V: https://repositum.tuwien.at/handle/20.500.12708/815]: Ic degradation measured after 3.3e22 m^-2 at 40 and 50 K [V]; the ~3e22 n/m2 (3e18 n/cm2) ARC design fluence value is [A]; any '% loss at 4-20 K' claim is [U]; Tc degradation roughly ~1 K per few 1e22 n/m2 [U]; gamma/ion damage and annealing at cycled temperatures matter (Fischer 2018, Prokopec/Unterrainer) | Sorbom et al., FED 100 (2015) 378, DOI 10.1016/j.fusengdes.2015.07.008 [V-lit] gives ARC design fluence assumption [A] |
| Insulator dose | ITER TF insulation specification 1e7 Gy (10 MGy), cyanate-ester/epoxy blend [V via search, https://www.osti.gov/etdeweb/servlets/purl/20641471 'Design of the ITER Magnets']; ~1e8 Gy for polyimide [U] | |
| Design codes | ITER SDC-IC (Structural Design Criteria, Class IC); RCC-MRx (AFCEN) with fusion supplement; ASME BPVC Section III Division 4 (Fusion Energy Devices, published 2021+) [A]; Eurofer data in MPDB/ITER Materials Properties Handbook | Standards pages (afcen.com, asme.org) |
| Creep/fatigue | primary+secondary stress limits (Sm, 3Sm), creep-fatigue interaction diagrams, irradiation-modified Sm | Code rules [A] |

### 1.4 Magnets

- ITER TF nuclear heating: total in the 18 TF coils limited to 17 kW, peak winding-pack power density 0.1 kW/m3 [V via search of Sawan/ITER magnet papers, e.g. https://fti.neep.wisc.edu/fti.neep.wisc.edu/presentations/mes_japmed0905.pdf; exact table not read; the earlier 14 kW figure is WRONG]. ITER cryoplant average 75 kW at 4.5 K, 87 kW pure refrigeration, 1300 kW at 80 K [V: https://www.iter.org/machine/supporting-systems/cryogenics]; LHe plants need ~24 MW compressor power, i.e. ~320 W/W at 4.5 K [derived from search-summary of 'ITER cryogenic system' paper, https://www.academia.edu/29751434/ITER_cryogenic_system; the 24 MW figure not read directly].
- Cryogenic penalty: Carnot ~70 W/W at 4.2 K from 300 K; modern large plants reach ~0.3 of Carnot (~230-360 W/W), 31% of Carnot quoted for a 40 MW plant [V via search: https://arxiv.org/pdf/1501.07154 and https://www.osti.gov/servlets/purl/928493]; Carnot at 20 K is ~14 W/W (computed); practical W/W at 20 K: [U, no primary source found]. HTS at 20 K trades cryo penalty against higher nuclear-heat tolerance (ARC/SPARC designs).
- Quench: ITER hot-spot limit 150 K for Nb3Sn CICC [A]; HTS quench detection is harder (slow normal-zone propagation); Peak stress: TF steel structures ~ 660 MPa Tresca ITER limit [A]; REBCO strain limit ~0.4-0.5% [A]. Demountable joints: MIT/ SPARC and VIPER cable experiments, joint resistance 1-10 nOhm per joint [A], total joint heating in 10s-100s kW.
- SPARC TFMC test: 20 T, 2021 (Hartwig et al., SuST 2024 [A]) is the reference HTS validation point.

### 1.5 Thermal-hydraulics and heat exhaust

| Item | Reference | Status |
|---|---|---|
| Divertor heat flux limit | ITER vertical targets 10 MW/m2 steady, 20 MW/m2 slow transient (~10 s); qualification 5000 cycles at 10 and 300 at 20 MW/m2 [V: https://www.iter.org/machine/divertor]; EU DEMO design limit ~5 MW/m2 [A] | ITER Divertor papers |
| SOL width | Eich et al., Nucl. Fusion 53 (2013) 093031, DOI 10.1088/0029-5515/53/9/093031: lambda_q [mm] = (0.63 +- 0.08) * B_pol,MP^-1.19, R^2=0.86 (JET, DIII-D, AUG, C-Mod, NSTX, MAST) | [V: https://iopscience.iop.org/article/10.1088/0029-5515/53/9/093031] |
| Exhaust figure of merit | P_sep*B/(q95*A*R) ~ 9 MW T/m (ITER ~ 9-10 range) [A]; P_sep/R ~ 15-17 MW/m for DEMO class [A] | [A] |
| First wall heat flux | ITER FW normal panels ~1 MW/m2 (enhanced heat flux panels up to 4.7 MW/m2) | [A] |
| Coolants | He 8 MPa 300->500 C (HCPB/ HCLL EU); water 15.5 MPa 295->328 C (WCLL); PbLi up to ~700 C (DCLL); FLiBe ~ 600-700 C (ARC); coolant choice sets Rankine (~33-37%) vs He Brayton (~40-45%) efficiency | [A] |
| Pumping power | He loops ~ 3-8% of thermal power in DEMO studies | [A] |
| Neutron wall loading | EU DEMO ~1 MW/m2 average (peak outboard ~1.3) ; ARC-class several MW/m2 | [A] |

### 1.6 RAMI / availability and maintenance

- Reference practice: ITER RAMI (RBD, FMECA, MTBF/MTTR in Reliability Block Diagram models), DEMO RAMI (Federici/Maviglia; Fusion Eng. Des. series, e.g., Maviglia et al., FED 2022? [U]); EU DEMO: starter blanket 20 dpa then second blanket 50 dpa in EUROFER first wall; availability low initially rising to about 30% or more [V: Federici talk https://nucleus.iaea.org/sites/fusionportal/Technical%20Meeting%20Proceedings/3rd%20DEMO/website/talks/G_Federici.pdf and Federici et al. NF 57 092002 https://iopscience.iop.org/article/10.1088/1741-4326/57/9/092002]; PPCS 75% target [V, same Crofts source]; ARIES-AT availability 0.85 [V via search, https://www.sciencedirect.com/science/article/abs/pii/S0920379605007210]; utility 75-85% range otherwise [A].
- Maintenance durations [V via search of Crofts et al., 'Maintenance duration estimate for a DEMO fusion power plant', FED 2014, https://arxiv.org/pdf/1412.4008 and https://www.sciencedirect.com/science/article/abs/pii/S0920379614000398]: full blanket replacement ~10 months (421 modules, 4 ports, 4 parallel systems, incl. 20% contingency), ~6 months in VR-simulation best case; divertor then blanket sequential, ~20 months total for internal components; ~1000 h per sector, four RH systems in parallel needed to meet PPCS 75% availability target. Remote handling availability is a first-order driver.
- Simulation tools: AvailSim4 (CERN, open source, Python, discrete-event Monte Carlo, phase-dependent failure/repair, root-cause module, quasi-MC and importance splitting) [V: https://indico.cern.ch/event/1507105/contributions/6342553/attachments/3008536/5303766/CB-SM_AvailSim4-Feb2025.pdf]; commercial RBD tools (BlockSim, Isograph); academic fusion RAMI: Cadwallader (INL), Gandhi/Reux? [U]. Best practice is event-driven Monte Carlo with failure laws (Weibull), repair crews, spares, and 10^4+ trials with confidence intervals on availability.
- Unplanned outage models: component MTBF from fission/accelerator/ITER analogues; scheduled replacement driven by fluence (the FARIS demo already does fluence-limited replacement).

### 1.7 Plant operation

- Pulsed vs steady: EU DEMO baseline pulsed, pulse ~2 h, dwell ~ 10-20 min [A]; CS fatigue cycles of order 1e4 [A]; thermal storage (molten salt/ steam accumulators) to smooth output; steady-state option raises recirculating power through current drive (eta_CD ~ 0.3-0.5 A/W-class values vary; wall-plug efficiency 30-50% [A]).
- Load following, startup/shutdown, black start, grid connection, scheduled outage planning.
- Power balance: P_gross = eta_th * (P_n*M + P_alpha_used + P_aux + pumping etc.); P_net = P_gross - P_recirc; Q_eng = P_gross / P_recirc [A: definitions vary by tool; FARIS must publish its definition]. Recirculating loads: heating and CD wall-plug, cryoplant, coolant pumps, tritium plant, vacuum, building HVAC, magnet power supplies and resistive losses.

### 1.8 Safety, licensing, waste

- US: NRC proposed rule 'Regulatory Framework for Fusion Machines' published in the Federal Register 2026-02-26 (90-day comment to 2026-05-27) under 10 CFR Part 30 byproduct framework, amending Parts 20, 30, 37, 50, 72, 110, 150, 170, 171 [V: https://www.federalregister.gov/documents/2026/02/26/2026-03865/regulatory-framework-for-fusion-machines]; 2023 Commission decision and ADVANCE Act definition [A]; final-rule status as of 2026-10 not checked [U]. Public dose limit 100 mrem/yr (1 mSv/yr) 10 CFR 20.1301 [V-lit]; 10 CFR 61.55 waste classification (Class A/B/C/GTCC, with Table 1 long-lived and Table 2 short-lived isotopes; Nb-94 in activated metal 0.2 Ci/m3, Ni-59 in activated metal 220, C-14 8 (80 in activated metal), Tc-99 3, I-129 0.08 Ci/m3 [V: eCFR 10 CFR 61.55 Table 1, https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55]; Table 2 col.1/2/3 (Class A/B/C limits): H-3 40 (no B/C limits), Co-60 700 (none), Ni-63 3.5/70/700, Ni-63 activated metal 35/700/7000, Sr-90 0.04/150/7000, Cs-137 1/44/4600 [V same page]).
- UK: Energy Act 2023 treats fusion as outside nuclear site licensing (ONR) and regulated by the Environment Agency and HSE under existing regimes; UK government "Fusion regulation" policy statement (2021, 2023) [A].
- IAEA: fusion safety guidance, IAEA-TECDOC series, clearance levels in IAEA RS-G-1.7 / GSR Part 3 [A]. ITER: dose at boundary limits for postulated events ~ 10 mSv for design basis and 50 mSv beyond-design [A]; no-evacuation criterion for DEMO ~10 mSv [A].
- Waste: Fetter, Cheng, Mann, 'Long-term radioactive waste from fusion reactors: Part II', Fusion Eng. Des. 13 (1990) [V, full text read: https://fetter.it-prod-webhosting.aws.umd.edu/sites/default/files/fetter/files/1990-FED-RadWaste.pdf]: uses effective dose equivalent to derive specific activity limits for all long-lived nuclides up to Cm-248; its table lists Nb-94 (20 ky) SAL 2E-01 Ci/m3 matching 10 CFR 61 value 0.2; Nb, Mo, Gd, Tb, Ho limits in first wall are severely restrictive for fusion-specific waste disposal ratings (WDR); EU "recycling/clearance" concepts (IAEA clearance index summation rule sum(C_i/CL_i) < 1); decay heat for LOCA: EUROFER decay heat peaks tens of kW/m3 order early after shutdown dropping by orders of magnitude in days [A]; safety analysis codes include MELCOR-fusion, RELAP-fusion, ECART [A].
- Tritium release: ITER annual limits for airborne tritium (tens of g/yr never; actually order of mg-g) [U]; FARIS must source site-specific limits and record them as authored.

### 1.9 Economics

- Cost accounting standards: GEN-IV EMWG Cost Estimating Guidelines Rev 4.2 (2007), account structure 10 pre-construction, 20 direct, 30 indirect, 40 owner's, 50 supplementary, 60 IDC, 70 O&M, 80 fuel, 90 financial [V-lit structure; DOI n/a, GIF EMWG document]; ARIES accounts 20-26; Sheffield and Milora, "Generic Magnetic Fusion Reactor Revisited", Fusion Sci. Technol. 70 (2016) 14 [A]. Overnight cost vs total capital with IDC; fixed charge rate; replacement cost levelisation.
- Cost points [V via search]: ARIES-ACT COE 67-69 mills/kWh (Kessel et al., Fusion Sci. Technol. 67(1) 2015, https://wx1.ans.org/pubs/journals/fst/v_67:1); Schwartz et al., Joule 2023 (https://www.cell.com/joule/fulltext/S2542-4351(23)00075-2): 100 GW of fusion needs capital cost below $2,700-7,500/kW net electric; Entler et al. 2018 (https://www.sciencedirect.com/science/article/pii/S0360544218305395): 1 GW EU-DEMO-like LCOE ~$175/MWh (2018 USD), direct capital $7.4B; Lindley et al. 2023 via secondary source: early plants >$150/MWh [U, secondary]; NREL ATB 2024 nuclear overnight $6,809/kW (2020) falling to $5,532/kW 2050, fixed O&M $114/kW-yr, variable $4/MWh [V via search, https://atb.nrel.gov/electricity/2024/nuclear]. A single '$50-100/MWh target' band has NO primary source here [U]. Capacity factor of 50-90% modelled; replacement costs for blanket/divertor are 5-20% of COE in ARIES/PROCESS studies [A].

### 1.10 Plasma/source coupling at plant level

- Plant needs: P_fus, P_alpha, P_aux, neutron source strength S_n = P_fus/17.6 MeV * 14.07/... (3.54e17 n/s per MW_fus [V-lit: 1 MW of D-T fusion = 3.55e17 reactions/s]), toroidal/poloidal neutron source profile, wall loading map (MW/m2 peaked outboard midplane, peaking factor ~1.2-1.5 [A]), radiation fraction and divertor power split, plasma duration, ramp. Reference: bluemira, FUSE 1.5D, PROCESS 0D.

---------------------------------------------------------------------------------------------------
## 2. Candidate requirements

Notation: "Tier" = fidelity expected for best in class. T0 = authored/scalar; T1 = 0D/analytic with documented closure; T2 = 1D/compartment ODE or reduced model; T3 = 2D/3D FEM/coupled multiphysics. IDs are PL-xxx. Targets marked provisional where uncertain.

### 2.1 Systems code, power balance, plasma 0D/1.5D

| Area | Requirement | Metric | Best-in-class ref | Proposed FARIS target | How to verify |
|---|---|---|---|---|---|
| PL-001 Power balance | FARIS shall compute gross electric, net electric, recirculating power, Q_eng and Q_plasma from a listed set of loads, each load individually reported. | Number of itemised recirculating loads; closure residual | PROCESS reports itemised recirculating power [A] | >=10 itemised loads; |sum - total| <= 1e-9 relative; every term carries a status label | Unit test of closure; hand calc for ARC-class case |
| PL-002 Definition | FARIS shall publish and version its Q_eng/Q_plasma/ net efficiency definitions in the project file. | Definition id embedded in every export | Tool definitions differ [A] | 100% of exports include definition id | Export-schema test |
| PL-003 Benchmarks | FARIS shall reproduce published PROCESS EU DEMO 2018 and ARIES-ACT1 gross/net electric within a stated tolerance. | |rel. error| | PROCESS regression reproduces own baseline [A] | <=5% on P_net, <=10% on recirc (provisional: different closure) | Benchmark suite in CI; inputs from published tables |
| PL-004 Plasma 0D | FARIS shall provide a 0D plasma model (H-factor confinement scalings IPB98(y,2), L-H threshold Martin, Greenwald, beta_N limits, bootstrap fraction, radiation) with selectable scalings. | Number of scalings; reproduces cited reference points | PROCESS [V-lit] | >=3 confinement scalings; reproduces ITER Q=10 point within 3% on P_fus given inputs | Test vs ITER baseline table (ITER Physics Basis 1999 [A]) |
| PL-005 Plasma 1.5D (stretch) | FARIS shall import steady-state profile outputs (n, T, j) from FUSE/bluemira/TRANSP and use them as authored sources. | Import success; profile hash recorded | FUSE 1.5D [A] | Import of >=2 formats; hash in receipt | Round-trip test |
| PL-006 Operating point solve | FARIS shall solve for a self-consistent operating point (P_fus vs confinement vs power balance) with convergence tolerance and report residuals. | Residual norm; iterations | VMCON/PROCESS | residual <=1e-8; fail-closed on non-convergence | Property tests; negative test |
| PL-007 Optimiser | FARIS shall provide a constrained optimiser over >=20 variables with recorded seeds, constraints, active set and Lagrange multipliers. | Reproducibility | VMCON SQP, NSGA-II in FUSE | Bit-identical result across 3 runs on same machine; feasibility tolerance 1e-6 | Determinism test |
| PL-008 Sensitivity | FARIS shall report local sensitivities (d output / d input) and a global ranking (Sobol or Morris) for P_net, TBR requirement, COE. | Number of inputs ranked | PROCESS-UQ/FUSE [U] | >=30 inputs; Sobol CI reported | Analytic toy functions |
| PL-009 UQ | FARIS shall propagate input distributions through the full plant chain and report P5/P50/P95. | Sample count; CI | Few tools do end-to-end UQ | >=1000 samples in <60 s on reference laptop for reduced chain (provisional) | Convergence test vs analytic |
| PL-010 Source handoff | FARIS shall derive the neutron source strength S_n and spatial/energy source from P_fus and publish conversion constants. | S_n per MW_fus | 3.55e17 n/s/MW [A] | agree with 17.6 MeV energy to 4 s.f. | Unit test |

### 2.2 Magnets

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| MG-001 Nuclear heating budget | FARIS shall compute coil nuclear heating (W, kW) from the radiation tally and compare to a stated cryogenic budget. | Heating in W with MC SE | ITER TF ~14 kW [A] | per-coil and total, 2-sigma flagged | OpenMC tally vs analytic slab |
| MG-002 Cryo wall-plug | FARIS shall convert cold loads (4 K, 20 K, 50-80 K) to wall-plug using a selectable COP (Carnot fraction or fixed W/W). | COP value reported | 250-300 W/W @4.5 K [A] | formula + source label; user-overridable | Hand calc |
| MG-003 Fluence/dose limits | FARIS shall track REBCO fluence (n/m2, E>0.1 MeV), insulator dose (Gy), Cu stabiliser dpa and report time-to-limit. | Years to limit with SE band | Fischer 2018 [A] | Show P95 bound; limit values authored with citations | Compare to ARC study [V-lit] |
| MG-004 Ic degradation | FARIS shall apply an Ic(fluence, T, B) degradation model and report margin. | Ic retention fraction | Fischer data [A] | Selectable model table with provenance | Check against published curves |
| MG-005 Stress | FARIS shall run analytic hoop/bending stress and Tresca checks on the TF case and CS. | MPa vs limit | ITER 660 MPa [A] | Closed-form thin/thick-shell with <=5% error vs analytic | Lame cylinder test |
| MG-006 FEM stress (stretch) | FARIS shall import FEM stress results (CalculiX/Elmer) as adapter outputs. | Hash binding | -- | adapter with receipt | integration test |
| MG-007 Quench | FARIS shall estimate hot-spot temperature and detection-time budget (Mueller/adiabatic integral) for LTS and HTS. | T_hotspot K | ITER 150 K [A] | adiabatic integral within 3% of analytic | Analytic integral |
| MG-008 Joints | FARIS shall model demountable joint resistance heating as an itemised cold load. | W | SPARC/VIPER nOhm-class [A] | input per joint x count | unit test |
| MG-009 Field | FARIS shall compute on-coil peak field B_max from radial build and ripple and check against conductor limit. | T | ARC 23 T [A] | within 3% of Biot-Savart for circular coils | Biot-Savart test |

### 2.3 Tritium fuel cycle

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| FC-001 Compartment model | FARIS shall solve a dynamic tritium inventory ODE with at least plasma, fueling, exhaust/pumping, isotope separation, storage, blanket, extraction, PFC retention, losses, decay. | Number of compartments | Abdou 2021, Meschini 2023 [V-lit] | >=8 compartments, user-defined residence times | Reproduce published Abdou/Meschini figures within 5% |
| FC-002 Burn x fueling | FARIS shall expose burn fraction and fueling efficiency as independent inputs and report their product. | -- | Abdou 2021 | Display with required-TBR contour | Closed-form check |
| FC-003 Required TBR | FARIS shall compute required TBR, startup inventory and doubling time for given availability, reserve time, processing times. | TBR_req, kg, years | 1.05-1.15 [A] | Match analytic steady-state formula to 1e-6; match Abdou example within 5% | Analytic limit + paper replication |
| FC-004 Decay | FARIS shall apply T half-life 12.32 y (5.47%/yr) on all inventories incl. storage. | -- | NNDC | exact decay law (relative error <1e-12 in integrator for constant coefficients) | Analytic exp test |
| FC-005 Margins on TBR | FARIS shall propagate neutronics TBR statistical error and systematic uncertainty (cross-section, geometry homogenisation) into inventory and doubling time. | P95 doubling | Not typical | distribution of TBR with systematic term, plus margin to required; fail-closed if TBR_calc - 2 sigma < TBR_req | Monte Carlo test |
| FC-006 Permeation | FARIS shall estimate tritium permeation losses through steam generator and coolant boundaries (Sieverts/diffusion-limited) with barrier factor. | g/yr | TMAP/FESTIM [A] | within factor 2 of FESTIM benchmark on a slab (provisional) | FESTIM cross-run |
| FC-007 Hold-up | FARIS shall report tritium hold-up in structure, PbLi, W, and codeposit, with a regulatory cap check (site limit). | kg vs limit | ITER 4 kg site [A] | flagged; limit authored | Test |
| FC-008 Startup | FARIS shall report external supply requirement and the first-fill sequence (reserve, ramp-up). | kg | Abdou/Meschini | included in project timeline | -- |
| FC-009 Interfaces | FARIS shall accept breeding rate from the transport tally in atoms/s with unit-checked conversion from OpenMC reaction rates per source neutron. | -- | -- | unit tests, conversion error <1e-12 | |
| FC-010 Pulsed | FARIS shall support pulsed operation with dwell and inventory swings. | -- | DEMO pulsed | time-resolved | |
| FC-011 Validation | FARIS shall replicate at least 2 published fuel-cycle cases (Abdou, Meschini) in CI. | rel err | -- | <=5% on startup inventory, doubling time | Benchmark suite |

### 2.4 Materials and damage

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| MT-001 dpa | FARIS shall compute dpa/FPY per material (NRT and, where data exist, arc-dpa) from the flux spectrum and damage cross-sections. | dpa/FPY | MCNP/OpenMC dpa; IRDFF-II [A] | Compare OpenMC vs analytic Fe benchmark within 5% | Benchmark |
| MT-002 Gas production | FARIS shall compute He and H appm and appm/dpa ratio per material. | appm | 1 appm weld limit [A] | label limits with source | Benchmark vs FISPACT/ACTINV [A] |
| MT-003 Limit table | FARIS shall maintain a versioned material-limit database (dpa, He appm, T window, source citation, conditions). | Rows with citation | -- | 100% rows have citation and status label | schema test |
| MT-004 Replacement | FARIS shall schedule replacement at limit with P5/P95 time. | years | existing demo | extended with SE | |
| MT-005 Thermal-mech | FARIS shall check first wall thermal stress and creep-fatigue per RCC-MRx/ASME-style simplified rules (primary + secondary). | utilisation ratio | codes | Simplified 3Sm check implemented; full code compliance NOT claimed | Hand calc |
| MT-006 Transmutation | FARIS shall track W->Re/Os and Cu/steel transmutation via ACTINV adapter and report effect on conductivity or activation. | -- | -- | adapter with receipt | |
| MT-007 Temperature | FARIS shall enforce operating temperature window per material and warn on excursions. | K | RAFM 350-550 C [A] | boolean + margin | |
| MT-008 Unknowns | FARIS shall mark any material without data as Not-evaluated with reason and next step. | -- | house rule | 100% | audit |

### 2.5 Thermal-hydraulics and heat exhaust

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| TH-001 Heat flux | FARIS shall compute average and peaked divertor and first-wall heat fluxes with explicit peaking factors and lambda_q source (Eich 2013). | MW/m2 | ITER 10 MW/m2 [A] | Eich fit reproduced to 1% | Formula test |
| TH-002 Exhaust metrics | FARIS shall report P_sep/R and P_sep B/(q95 A R). | MW/m, MW T/m | [A] | exact formula | Test |
| TH-003 Radiation fraction | FARIS shall require radiated fraction f_rad and show detachment requirement. | -- | DEMO f_rad ~0.7-0.9 [A] | input + required value | |
| TH-004 Coolant loop | FARIS shall run a 1D steady loop for each coolant (He, H2O, PbLi, FLiBe): T in/out, mass flow, pressure drop, pumping power, margin to limits. | W | [A] | energy balance residual <=1e-6; pumping vs Darcy-Weisbach within 2% | Analytic pipe test |
| TH-005 Properties | FARIS shall use cited property tables with temperature range checks (IAPWS-IF97 for water; CoolProp for He). | -- | IAPWS | match IAPWS to 1e-6 | Table test |
| TH-006 Cycle | FARIS shall compute thermal conversion efficiency from a selectable cycle (Rankine, Brayton, supercritical CO2) with component efficiencies. | % | 33-45% [A] | within 1% of cycle calc vs CoolProp-based reference | Reference model |
| TH-007 MHD | FARIS shall flag liquid-metal MHD pressure drop as Not-evaluated unless an adapter supplies it. | -- | -- | status label | |
| TH-008 CHF | FARIS shall check coolant CHF margin for divertor tubes (Tong/ Boscary correlations). | margin | ITER [A] | margin >=1.4 flag (provisional) | |
| TH-009 Transient | FARIS shall run a lumped transient for pulse/dwell and ELM/disruption energy deposition (W/m2 s^0.5 heat factor). | -- | W melting FP ~50 MJ/m2 s^-0.5? [U] | authored thresholds | |

### 2.6 RAMI / availability / maintenance

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| RM-001 Availability MC | FARIS shall simulate plant availability via event-driven Monte Carlo with Weibull failure laws, repair crews, spares and RH constraints. | availability with CI | AvailSim4 [A] | >=1e4 trials, 95% CI half-width <=0.5 points; deterministic seeds | Analytic M/M/1 tests |
| RM-002 RBD | FARIS shall support RBD structure (series, parallel, k-of-n, standby). | -- | RBD tools | exact analytic availability for small cases within MC CI | Test |
| RM-003 Replacement durations | FARIS shall take replacement durations (blanket, divertor, port plug) as cited inputs and show sensitivity. | days | DEMO 3-6 months [A] | tornado diagram | |
| RM-004 Scheduled vs unplanned | FARIS shall separate planned, unplanned and fluence-driven outages in output. | -- | -- | 3 categories | |
| RM-005 Targets | FARIS shall compare availability against EU DEMO ~30% [A] and utility >=75-85% [A] targets as labelled thresholds. | -- | -- | pass/fail flags | |
| RM-006 Lifetime effects | FARIS shall couple material fluence to failure hazard (hazard increases near limit). | -- | -- | selectable | |
| RM-007 Scheduling optimisation | FARIS shall optimise maintenance windows (e.g., batch replacements) for min LCOE. | -- | -- | result with recorded seeds | |
| RM-008 Maintenance hazard | FARIS shall report maintenance shutdown dose rate and dose to workers as a cross-reference to the shutdown-dose analysis. | uSv/h | ITER 10 uSv/h at 1e6 s [A] | cross-link | |

### 2.7 Plant operation

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| OP-001 Duty cycle | FARIS shall model pulsed operation with pulse length, dwell, ramp, and duty factor and report average net power. | MWe | EU DEMO ~2 h pulse [A] | closure vs hand calc | |
| OP-002 Fatigue | FARIS shall count cycles for CS and structure and report cycle life margin. | cycles | 1e4-class [A] | counts | |
| OP-003 Storage | FARIS shall include thermal storage sizing to bridge dwell. | MWh | -- | energy balance | |
| OP-004 Load following | FARIS shall simulate dispatch against a user-provided demand/price series. | -- | -- | hourly series, 8760 h in <5 s | |
| OP-005 Startup/shutdown | FARIS shall model startup energy and tritium ramp. | -- | -- | included | |
| OP-006 Schedules | FARIS shall allow editing of an operating schedule (phases, power levels) with undo/redo and receipts. | -- | -- | | |

### 2.8 Safety, licensing, waste

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| SF-001 Waste class | FARIS shall classify activated components against 10 CFR 61.55 (Class A/B/C/GTCC) from ACTINV inventories using sum-of-fractions. | class | 10 CFR 61 [V-lit] | table-driven; matches NRC worked examples | Table test |
| SF-002 Alt classifications | FARIS shall also evaluate IAEA clearance (sum of fractions <1), UK LLW/ILW thresholds, and Fetter WDR. | -- | RS-G-1.7, Fetter 1990 [A] | selectable regime, regime cited | Test |
| SF-003 Decay heat | FARIS shall give decay heat vs time per component for LOCA screening, with ACTINV adapter. | W/m3 | -- | agreement with FISPACT reference within 10% on test case (provisional) | cross-code |
| SF-004 LOCA screen | FARIS shall estimate adiabatic temperature rise of the in-vessel inventory from decay heat. | K | -- | analytic | |
| SF-005 Source terms | FARIS shall maintain inventory source terms (tritium, activated dust, activated corrosion products) as authored/computed labelled values. | -- | -- | labelled | |
| SF-006 Dose to public | FARIS shall compute public dose for an authored release using a documented dispersion model or flag Not-evaluated. | mSv | 1 mSv/yr [V-lit] | scope-limited | |
| SF-007 Regulatory profile | FARIS shall carry a jurisdiction profile (NRC Part 30 byproduct, UK, IAEA) mapping each limit to citations. | -- | NRC 2023 [A] | versioned profile file | |
| SF-008 Safety claims | FARIS shall never emit a licensing claim; outputs are labelled research screening. | -- | house rule | static check | |

### 2.9 Economics

| Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|
| EC-001 Accounts | FARIS shall support GEN-IV EMWG and ARIES account structures with mapping. | accounts | EMWG Rev 4.2 [V-lit] | both mappable | |
| EC-002 LCOE | FARIS shall compute overnight cost, IDC, total capital, FCR-based and discounted-cash-flow LCOE with replacement and decommissioning. | $/MWh | ARIES, PROCESS [A] | two methods agree within 1% in constant-cost limit | Closed-form |
| EC-003 Cost provenance | FARIS shall mark every cost as authored/literature/synthetic; the existing synthetic prices stay labelled. | -- | -- | 100% | |
| EC-004 Escalation | FARIS shall index costs to a base year (CEPCI/GDP deflator) with source. | -- | -- | | |
| EC-005 Sensitivity | FARIS shall give LCOE tornado for >=15 drivers. | -- | -- | | |
| EC-006 Replacement cost | FARIS shall levelise replacement costs (blanket/divertor) with downtime revenue loss. | -- | 5-20% COE [A] | | |
| EC-007 Benchmarks | FARIS shall reproduce ARIES-AT / ACT COE within stated tolerance given published inputs. | rel err | -- | <=10% (provisional) | |

### 2.10 Cross-cutting plant

| Area | Requirement | Metric | Target | Verify |
|---|---|---|---|---|
| XC-001 Traceability | Every plant-level number shall carry status (calculated/authored/literature/conditional/not-evaluated), unit, and provenance. | 100% | schema | audit |
| XC-002 Unknown | Every Unknown shall carry why + next step. | 100% | | |
| XC-003 Recalc | The coupled 30-year recalculation shall remain <=1 s from slider change as models are added (provisional budget: <=2 s). | ms | | perf test |
| XC-004 Units | Dimensional analysis in all formulas (uom checked). | 0 unit errors | | lint |
| XC-005 Validation record | Each model shall list validation cases, tolerance, and last-pass hash. | -- | | |
| XC-006 Reference plants | Ship reference input sets (ARC-class, EU DEMO 2018, STEP-class) with citations. | 3 | | |

---------------------------------------------------------------------------------------------------
## 3. Traps

1. Required TBR as a single number. The "1.05-1.15" band hides the dependence on f_b*eta_f, processing time, availability and reserve. Report a surface, not a number, and do not quote a pass/fail against 1.1.
2. Q_eng, Q_plasma and net efficiency definitions differ between PROCESS, ARIES and FUSE (whether pumping, cryo, TF resistive losses count). Cross-tool comparisons without definition matching are meaningless.
3. TBR from a homogenised/idealised build overstates real TBR; port, penetration, and manufacturing losses (often -0.03 to -0.10 [A]) must be explicit.
4. dpa is model-dependent (NRT vs arc-dpa vs MD-based); He and H gas production are as limiting as dpa. A dpa-only limit is a trap.
5. Limit values (20/50/100 dpa, 1 appm He) are design aspirations or conservative codes, not material properties; label as authored with conditions.
6. REBCO limits depend on irradiation temperature, annealing during warm-up and spectrum; fluence quoted without energy cut-off or temperature is not comparable.
7. Cryo COP of 250 W/W is a plant-scale number; small cold loads have worse COP; Carnot alone understates penalty by ~3-4x.
8. Availability: "MTBF-based Monte Carlo" with invented MTBFs produces false precision; CI width reflects sampling not epistemic uncertainty. Separate aleatory and epistemic parts.
9. LCOE with synthetic prices is a model exercise; label any absolute $ as non-predictive. Learning-curve/first-of-a-kind effects dominate.
10. Waste classification depends on regime, geometry (activated metal vs other) and which nuclides (Nb-94, Ni-59, C-14, Tc-99) are included; impurities in steel (Nb, Ag, Co) dominate; use measured/assumed impurity specs.
11. Divertor 10 MW/m2 is a component limit at the surface for a specific design; plant-level P_sep/R metrics and detachment physics determine achievability. Do not compare unmitigated average flux with a limit.
12. Surrogate peer tools: do not claim parity with PROCESS or FUSE without a reproduced regression case; claim "reproduces X within Y%".
13. Licensing language: a code that outputs "meets NRC limits" creates liability and false confidence; keep as screening.
14. TREND: could not confirm owner/scope; do not cite. (MIRA, FFCSim, TRICYS are now verified, see 1.1/1.2.)
15. Monte Carlo statistics on heating near magnets: deep-penetration tallies converge poorly; report relative error and FOM, and never convert a 1-sigma-error tally into a margin without variance reduction.


---------------------------------------------------------------------------------------------------
## 4. Verification pass 2 (2026-10-05): what changed, what remains [U]

Corrections: ITER TF nuclear heating limit is 17 kW (not 14); Meschini DOI is acf3fc; the Abdou required-TBR claim is superseded by his baseline 1.15 (1.1-1.25) and f_b*eta_f > 2% rule; FC-003/FC-011 reference values must use these.
Newly [V] (deep link in body): 10 CFR 61.55 Tables 1/2 values; Fetter full text; ITER cryoplant 75 kW@4.5 K and ~320 W/W derived; insulator 1e7 Gy; DEMO blanket replacement ~10 mo (6 mo best case), ~20 mo internals; PPCS 75% availability; ARIES-ACT COE 67-69 mills/kWh; ARIES-AT availability 0.85; Schwartz 2023 capital thresholds; Entler 2018 $175/MWh; NREL ATB 2024 nuclear overnight cost; Abdou startup inventory ranges (via summaries).
Caveat: several [V] come from search-engine summaries of paywalled pages, flagged inline; before quoting externally open the primary page.
Still [U] (not found in primary open text): Meschini startup kg; Abdou exact doubling-time numbers; cryo W/W at 20 K; polyimide insulator ~1e8 Gy; quench hot-spot 150 K; REBCO 4-20 K loss; CuCrZr/W/SiC/ODS dpa limits; DEMO divertor 5 MW/m2; ITER/DEMO public dose criteria; tritium release limits; decay-heat magnitudes; a primary '$/MWh target' for fusion LCOE; Lindley 2023 primary; GASC citation; TREND.
Effect on requirements: rows citing [A] values in MG-002 (use ~230-360 W/W at 4.5 K as range, 70 W/W Carnot), MG-001 (use 17 kW), FC-003 (use Abdou ranges), SF-001 (table-driven from eCFR, now fully specified: Table 1 and 2 above), RM-003/RM-005 (DEMO 6-10 month blanket, 75% PPCS), EC-007 (ACT 67-69 mills/kWh reproduce within 10%) are now sourced.
