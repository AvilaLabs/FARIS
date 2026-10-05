# R1: Radiation transport and nuclear analysis requirements for FARIS

Date: 2026-10-05. Area: radiation transport and nuclear analysis for fusion.

PROVENANCE (revised 2026-10-05 after live verification pass). First draft was from recall; a second pass checked the
headline numbers via live search/fetch (search-result abstracts and fetched pages, NOT full-paper reading, so table-level
values remain to be checked in the full text). Tags: [V*] = verified live this pass against the cited source (abstract
or page level), [V] = well established, not re-fetched, [R] = recalled / still not verified, [U] = unverifiable or judgement.
Remaining [R]/[U] items are listed in section 4. Original tag meanings:
[V] = well established and stable (high confidence, still re-check the cited URL before it goes in a spec),
[R] = recalled, order of magnitude / range is right but exact value must be re-verified against the primary source,
[U] = unverified or a judgement; do not quote externally. Source URLs are canonical landing pages, not deep links to the
exact table; a requirement that cites a number must have a human fetch the primary source before freeze.

## 1. Best-in-class landscape

### 1.1 Validation benchmarks and achieved accuracy (C/E)
- SINBAD (OECD NEA Shielding Integral Benchmark Archive and Database) is the reference archive of fusion neutronics
  experiments (FNG, FNS/JAEA, OKTAVIAN, ITER-TBM mock-ups, etc.). https://www.oecd-nea.org/jcms/pl_20040/sinbad-shielding-integral-benchmark-archive-and-database [V existence; entry count U].
- FNG (ENEA Frascati, 14 MeV DT generator) benchmarks: ITER bulk shield mock-up, streaming experiments, HCPB/HCLL TBM
  mock-ups, and the FNG-ITER shutdown dose rate (SDDR) benchmark. VERIFIED SDDR results [V*]:
  * OpenMC cell-based R2S vs FNG first campaign: C/E ~0.88 at 1 d (within experimental uncertainty), ~1.16 at 7 d,
    ~1.17 at 60 d, i.e. systematic over-prediction (+16-17%) at longer cooling, "consistent with previous studies";
    Nucl. Fusion 2024 https://doi.org/10.1088/1741-4326/ad32dd (table values via fetched abstract summary; check full text).
  * OpenMC D1S vs the same R2S: D1S within 4% of R2S, 10-15% lower per another statement; Fusion Sci. Technol. 2025
    https://doi.org/10.1080/15361055.2025.2567099 .
  * Another method (N1S) reported C/E 1.34 at the first decay time and within experimental error otherwise
    (search-result snippet, source paper not identified) [R].
  Original FNG SDDR experiment: Fusion Eng. Des. 2002 https://doi.org/10.1016/S0920-3796(02)00128-X (measurement paper
  linked from ScienceDirect page S092037960200128X) [V*]. The earlier "within ~5-20% shield / 30-50% deep" reaction-rate
  statement is NOT verified and is demoted to [U].
- FNS (JAEA Tokai): 14 MeV DT, integral experiments (tungsten, SS316, Li2O/Li blanket). Cited C/E for tritium production
  ~0.9-1.1 with JENDL/FENDL [R]. https://fnshp.tokai-sc.jaea.go.jp/ [U].
- JET DTE2 SDDR: Eur. Phys. J. Plus 139:432 (2024) https://doi.org/10.1140/epjp/s13360-024-05208-w ; ionization chamber
  (octant 1) dose rate vs cooling time after 21 Dec 2021, compared with D1S (MCNP5+FISPACT) and R2S. The abstract states
  prior JET SDR measurements "mostly agree within about +-30%" with Advanced-D1S and R2S [V*]. The DTE2-specific C/E
  numbers could not be retrieved (paywalled); DD-campaign benchmark showed factor-2 D1S/R2S discrepancy in octant 2 at 6 h
  (decay photon flux) [V*]. DTE2 foil C/E claim from draft is NOT verified [U]. DD benchmark dose rates ~8 mGy/h (3 h) to
  ~0.5 mGy/h (14 d) in octant 1 [V*].
- TBR mock-up experiments: FNG HCLL/HCPB mock-ups, MCNP with FENDL-2.1 and JEFF-3.1.1: "good agreement between
  measurements and calculations within the total uncertainty of 5.9% at 1-sigma" for tritium production
  (Nucl. Fusion 52 (2012) 083014, https://doi.org/10.1088/0029-5515/52/8/083014) [V*]. C/E band of 0.95-1.05 from the
  draft is NOT verified: use "within 5.9% (1 sigma)" instead. LIBRA statement dropped [U].
- Nuclear-data uncertainty on TBR, DEMO HCPB [V*]: +-3.2% (JEFF-3.2 covariances), +-5.6% (JEFF-3.3T1), +-8.6%
  (TENDL-2014); dominant contributors O-16, Li-6, Li-7 (EPJ Web Conf. ND2016 09025,
  https://www.epj-conferences.org/articles/epjconf/abs/2017/15/epjconf-nd2016_09025/epjconf-nd2016_09025.html);
  another estimate 3-4% for HCPB DEMO. Mock-up tritium production data uncertainty ~4% (2 sigma), Be data dominant
  [V*, snippet]. So "a few %" holds but is library-dependent: 3-9%. No FENDL-3.2b/ENDF/B-VIII.1 TBR covariance figure found [U].
- EU DEMO TBR design margin ~0.05 [R, unchanged].
- ITER MCNP models: 40-degree series A-lite, B-lite, C-lite, C-model (reflective boundaries, repeated 9x); E-lite (2020)
  is the first 360-degree ITER MCNP model, built via SpaceClaim simplification, SuperMC 7 translation, lost-particle
  debugging with MCNP and D1SUNED [V*] (https://doi.org/10.1038/s41467-024-52667-x). A C-lite model has ~10,000 volumes
  [V*, search snippet, source paper not opened]. E-lite cell/surface counts NOT found (my earlier "1e6 cells" is withdrawn) [U].
  Other pieces: the ITER SDDR requirement is
  ~100 uSv/h at 1e6 s (~12 d) cooling in the port interspace/bioshield-accessible locations and an occupational limit
  of 10 mSv/y [R, not re-verified]; ITER nuclear analysis reports use per-voxel relative error targets ~<5-10% in regions of interest and
  require MCNP's statistical checks to pass [R/U]; documented applied safety/uncertainty factors on nuclear responses
  (e.g. 1.3-2x on peak heating / SDDR) [U].
- Typical acceptable C/E (working consensus, not a standard) [U]: neutron flux/reaction rates +-10-20% near source, +-30%
  deep; nuclear heating +-15-20%; tritium production +-5-10%; SDDR +-20-30%; activation foils +-10-20%.

### 1.2 Codes and capabilities
- MCNP6.3 (LANL): CSG + unstructured mesh, weight windows (WWG, WWINP), 10 statistical checks, FMESH tallies,
  DAGMC-capable builds (DAG-MCNP5/6). https://mcnp.lanl.gov [V]. Export-controlled distribution (RSICC) [V].
- OpenMC (open source, MIT): CSG + DAGMC (CAD surface meshes via MOAB) + unstructured mesh tallies (libMesh/MOAB),
  weight windows, FW-CADIS-style via random ray (since ~0.14/0.15) [R], depletion, R2S workflows
  (OpenMC R2S, openmc-activation tools), event-based GPU transport via OpenMP offload under development (Tramm et al.),
  GPU results [V*] (Tramm et al., https://doi.org/10.1051/epjconf/202430204010 ; ANL workshop slides
  https://events.cels.anl.gov/event/610/contributions/1629/attachments/699/2367/OpenMC_Workshop_GPU.pdf): one A100 ~ 200
  Xeon cores on a depleted-fuel reactor (fission) benchmark; ~70x vs dual-socket Xeon node on Intel GPUs for exaSMR;
  these are fission-reactor problems, not fusion/photon-heavy models. No fusion-model GPU speedup found [U]. DOI of the
  epjconf paper is inferred from its URL; confirm.
  https://docs.openmc.org ; https://github.com/openmc-dev/openmc [V]. Fixed-source CPU throughput on a fusion CSG model
  is O(1e3-1e4) neutron histories/s/core depending on photon transport and tallies [U; measure locally].
- Serpent 2 (VTT): primarily fission, usable for fixed-source; https://serpent.vtt.fi [V].
- TRIPOLI-4 (CEA) and TRIPOLI-5: fusion capable, used by F4E/CEA; https://www.cea.fr [U].
- Shift (ORNL, GPU-capable) and Denovo (deterministic S_N): used within ADVANTG/CADIS and for exascale; [U].
- Attila/Ansys: deterministic S_N on unstructured mesh, no MC noise, ray effects/mesh dependence; [U].
- FW-CADIS / ADVANTG (ORNL): global VR via adjoint; MAGIC (UKAEA) weight-window generation; reported figure-of-merit
  gains for ITER [V*] (Wagner, Peplow, et al., Nucl. Technol. 175(1) 2011 https://doi.org/10.13182/NT175-251): FOM
  x275 vs analog and x9 vs traditional VR for inboard TF coil nuclear heating; x21,000 for prompt dose outside bioshield.
  ADVANTG on ITER prompt dose benchmark: speedups up to 7.1 (neutrons) and 59.6 (photons) [V*, search snippet, source
  not identified]. The draft range "1e2-1e6" is replaced by these.

### 1.3 Geometry
- CAD to Monte Carlo: cad_to_dagmc (https://github.com/fusion-energy/cad_to_dagmc) [V], GEOUNED (CAD->CSG for MCNP/OpenMC,
  https://github.com/GEOUNED-org/GEOUNED) [V], McCad (KIT) [V], SuperMC (FDS Team, China) [V], DAGMC
  (https://svalinn.github.io/DAGMC/) [V]. Conversion time from CAD to a faceted DAGMC for a full tokamak is minutes to
  hours; CSG conversion of hundreds of thousands of surfaces is hours [U].
- Geometry QA: lost-particle count (MCNP aborts at 10 lost; OpenMC aborts at 10 by default via max_lost_particles
  setting and relative fraction) [V/R]; overlap checks via plot/overlap check and `openmc --geometry-debug` [V].

### 1.4 Nuclear data
- FENDL-3.2b released Feb 2022 [V*] (https://www-nds.iaea.org/fendl_library/websites/fendl32b/); validation paper
  Nucl. Data Sheets 2024, https://doi.org/10.1016/j.nds.2024.01.001-type DOI NOT confirmed; arXiv:2311.10063 states
  FENDL-3.2b is at least as good as and mostly better than FENDL-2.1 on experimental benchmarks [V*]. ENDF/B-VIII.1
  released 30 Aug 2024 [V*] (https://www.osti.gov/biblio/2571019; Nucl. Data Sheets 2026 article
  https://www.sciencedirect.com/science/article/pii/S0090375226000268). JEFF-4.0 (2025), TENDL-2023, JENDL-5 [R].
- Uncertainty: covariance-based TMC / SANDY / sensitivity tools (SANDY, NJOY covariance via ERRORR, TSUNAMI-style for MC
  via adjoint/perturbation, OpenMC sensitivity via ongoing work) [R/U].

### 1.5 Responses needed in fusion analysis
- TBR (global/local), nuclear heating (neutron + photon + e-/e+, kerma vs energy deposition), fast flux E>0.1 MeV, fluence,
  dpa (NRT per Norgett-Robinson-Torrens 1975 doi:10.1016/0029-5493(75)90035-7; arc-dpa per Nordlund et al., Nat. Commun.
  2018 doi:10.1038/s41467-018-03415-5), He/H production (appm), magnet limits [R]:
  * ITER TF insulation: 10 MGy lifetime dose corresponds to fast fluence up to 3.2e21 n/m2 (source:
    https://www.tandfonline.com/doi/abs/10.13182/FST09-A8985 and search snippet) [V*]. The "1e22 n/m2" figure is quoted as
    the ITER design fluence in Fischer et al. (below) [V*]. Peak winding-pack nuclear heating 2 mW/cm3 appears in an
    ITER/TIBER TF requirements table (https://www.osti.gov/servlets/purl/6729950, 1980s design) [V*, old]. Total TF heating
    is NOT settled: sources show ~7.3 kW and ~17 kW (BPP) in different documents; my "14 kW" is withdrawn [U].
    Cu dpa limit ~6e-5 [R, not verified].
  * REBCO: Fischer et al., Supercond. Sci. Technol. 31 (2018) 044006, https://doi.org/10.1088/1361-6668/aaadf2 [V*]:
    fluence counted for E>0.1 MeV (fast part is 36% of their reactor spectrum); Ic first rises then degrades; tapes
    irradiated at 40-50 K degrade below initial Ic at ~3.3e22 m-2 (3.3e18 n/cm2); degradation fluence is lower at higher
    T and for tapes with artificial pinning centres; Tc falls ~3% per 1e22 m-2. Irradiation T matters: limits at 20 K or
    lower are higher; no single limit exists. The 20 K value in the draft is unverified [R].
- SDDR via R2S (rigorous 2-step) or D1S; activation inventory, decay heat, waste classification (ITER/IAEA/NRC 10CFR61
  classes), streaming through ports/penetrations, neutron wall loading [V].

### 1.6 Activation / inventory codes
- FISPACT-II (UKAEA, https://fispact.ukaea.uk) [V]: pathways, uncertainties, TENDL/EAF data. The "10-20% agreement"
  claim is NOT verified [U]. OpenMC's activation step matched FISPACT-II 4.0 "within fractions of a percent" on decay
  photon spectra for a shared inventory [V*, https://doi.org/10.1088/1741-4326/ad32dd].
- ALARA (UW-Madison, https://github.com/svalinn/ALARA) [V], ACAB (UPM) [V], ORIGEN (ORNL, SCALE) [V]. Runtimes: seconds to
  minutes per zone; for 1e5 mesh zones minutes to hours with parallelism [U].
- ACTINV (Avila Labs) is the in-house adapter; its accuracy contract should be inherited by FARIS.
- Li-6 depletion over life: time-dependent breeding blanket composition, burnup of Li-6 (enrichment drift); for ARC-class
  molten-salt FLiBe, Li-6 burnup of several % to tens of % per full-power-year depending on enrichment [U].

### 1.7 Source modelling
- Parametric plasma source (Fausser et al.; used in OpenMC via `openmc-plasma-source`,
  https://github.com/fusion-energy/openmc-plasma-source) [V]: Ballabio D-T Gaussian approximation of neutron energy
  distribution with ion-temperature dependent mean and width [R], DD: 2.45 MeV [V], DT: 14.06 MeV [V].

### 1.8 UQ and convergence
- MCNP's 10 statistical checks and relative-error guidance: <0.10 generally reliable (non-point-detector), <0.05 for point
  detectors, >0.5 garbage; VOV < 0.1; FOM stable (not trending); tally PDF slope > 3 [V]. https://mcnp.lanl.gov
- OpenMC reports standard error, trigger-based batch control (relative error triggers) [V].

## 2. Candidate requirements

Note on verification: rows citing a number now use the verified values from section 1 where available. Rows still
carrying [R]/[U] are not backed by a primary source; targets in them are design choices, not literature facts.

Legend for "Reference": see provenance warning. "P" after a target = provisional.

| ID | Area | Requirement | Metric | Best-in-class reference + source | Proposed FARIS target | How to verify |
|---|---|---|---|---|---|---|
| N-001 | Validation | FARIS shall maintain a versioned benchmark suite with FNG-ITER SDDR and FNG TBR/shield mock-ups, run in CI nightly | Number of SINBAD/IAEA benchmarks automated, pass/fail | SINBAD (OECD NEA) [V] https://www.oecd-nea.org/jcms/pl_20040/sinbad-shielding-integral-benchmark-archive-and-database | >=10 benchmarks automated by v1.0; >=3 FNG, >=1 FNS, >=1 JET foils (P: data availability) | CI job output; benchmark receipts hashed |
| N-002 | Validation | FARIS shall report C/E for each benchmark with propagated experimental uncertainty and flag outside tolerance | C/E per detector, tolerance bands | Consensus [U] | Flux/reaction rate within +-20% near source, +-30% deep; heating +-20%; tritium production +-6% (1-sigma total of FNG HCLL experiment, Nucl. Fusion 52 083014 [V*]); SDDR +-30% | Automatic table in validation report; checker verdict |
| N-003 | Validation | FARIS shall publish its measured C/E (not target) against each benchmark in documentation with code, data, and version | Doc page per release | MCNP/OpenMC V&V reports [U] | 100% of releases include a generated V&V report | Release gate checks report existence and hash |
| N-004 | Validation | FARIS shall state the validated domain: reject / warn when a user's scenario leaves it (e.g. LiPb vs FLiBe, thick vs thin shield) | Domain-of-validity flags | ITER practice of applying margins [U] | Every result carries "inside / outside validated domain" label | Unit test on known out-of-domain scenario |
| N-005 | TBR | FARIS shall compute global TBR with total relative standard error <=0.5% (1 sigma, MC) | rel. error | Standard practice <1% [U] | <=0.5% default; 1M-histories default meets for the ARC-type model (measure) | Run, compare to reported sigma; coverage test over 20 seeds |
| N-006 | TBR | FARIS shall report TBR with a nuclear-data uncertainty band (separate from MC error) | 1-sigma data uncertainty | 3.2-8.6% depending on library [V*, N-053 source] | Reported per library; show FENDL-3.2b vs ENDF/B-VIII.1 vs JEFF-4 difference | Cross-library run; difference reported |
| N-007 | TBR | FARIS shall produce local TBR (per blanket module / toroidal and poloidal zone) and cumulative TBR with port/penetration losses | Zones, error per zone | ITER TBM / DEMO modules [U] | Per-zone TBR with sigma <=2% for zones >5% of total | Sum of zones equals global within 3 sigma |
| N-008 | TBR | FARIS shall separate Li-6 and Li-7 contributions and (n,t) by reaction | Reaction rate breakdown | OpenMC tallies [V] | Always available | Sum matches total |
| N-009 | Heating | FARIS shall compute nuclear heating with neutron, photon, electron, positron contributions summed, never photon-filtered only | Component breakdown, sum | OpenMC pitfall (project memory: photon-filtered heating is ~1e-4 of physical) [V internal] | Sum is the default; unit test against energy-conservation | Energy-balance check within 1% of source energy |
| N-010 | Heating | FARIS shall state whether heating is kerma-based or energy-deposition and shall provide both for a reference case | Method label, difference | Heating-number conventions [R] | Both available; difference reported; default per validated library | Compare against analytic slab case |
| N-011 | Heating | FARIS shall verify energy balance: total deposited + leakage = source energy released + Q-value gain | Closure fraction | Standard [U] | Closure within 1% (P) | Automated check in every run |
| N-012 | Heating | FARIS shall report magnet nuclear heating in W/m3 (peak and per coil) and total kW at operating temperature | W/m3, kW | ITER TF total heating not settled (7.3-17 kW across sources), peak 2 mW/cm3 in old requirements table [V*, see 1.5] | Per-coil sigma <=5% (peak voxel sigma <=10%) | Mesh tally and sum check |
| N-013 | Fluence | FARIS shall report fast fluence (E>0.1 MeV) at coil surfaces and winding pack with per-FPY and lifetime scaling | n/m2, relative error | ITER 1e22 n/m2 design fluence per Fischer 2018 [V*]; 10 MGy insulator = 3.2e21 n/m2 [V*] | Peak sigma <=10%; limit shown as authored with source label | Cross-check with flux tally integrated over source |
| N-014 | Magnet limits | FARIS shall evaluate REBCO fluence limit, insulator dose, Cu-stabilizer dpa limit with the limit value labelled by source and conditions (T, field) | Limit table with source, status | Fischer 2018 https://doi.org/10.1088/1361-6668/aaadf2 [V*]; ITER insulation 10 MGy [V*] | Each limit stored with citation + conditions; calculation uses worst-case voxel with 2-sigma upper bound | Review of limit table; test with synthetic fluence |
| N-015 | Magnet limits | FARIS shall flag a magnet "pass" only when the 2-sigma upper bound (stat + data) is below the limit | Pass/fail rule | [U] | Pass requires upper bound; "not-evaluated" if statistically unconverged | Unit tests with edge values |
| N-016 | Damage | FARIS shall compute dpa with NRT and arc-dpa, selectable, using material-specific threshold displacement energies | dpa/FPY | Nordlund 2018 doi:10.1038/s41467-018-03415-5 [V] | NRT and arc-dpa both for Fe, W, Cu, Cr, Ni; difference reported | Compare to published Fe, W per-neutron dpa values (within 10%) |
| N-017 | Damage | FARIS shall compute He and H gas production (appm/FPY) from MT 203/207 or equivalent | appm | EUROFER/W blanket design [U] | Tally per material; error <=5% bulk | Compare to published ITER FW values (P) |
| N-018 | Damage | FARIS shall compute displacement for 14 MeV and spectrum-weighted cases from the same transport run | Tally consistency | [U] | Single run yields all | Test |
| N-019 | SDDR | FARIS shall compute shutdown dose rate by R2S (rigorous two-step) with mesh-based neutron flux, activation, and photon transport | Sv/h at cooling times 1 d - 1 y (incl. 1e6 s) | OpenMC R2S on FNG: C/E 0.88 (1 d), 1.16 (7 d), 1.17 (60 d) [V*, doi 10.1088/1741-4326/ad32dd]; JET prior +-30% [V*]; ITER 100 uSv/h [R] | C/E within +-30% on FNG-ITER SDDR at all cooling times; report sign of bias (known over-prediction at long cooling) | Benchmark run |
| N-020 | SDDR | FARIS shall provide D1S as a fast alternative and state the difference from R2S for the same case | Ratio D1S/R2S | OpenMC D1S vs R2S within 4% (cell-based), 10-15% lower per same source [V*, doi 10.1080/15361055.2025.2567099] | D1S within 15% of R2S on FNG case (OpenMC: 4-15% [V*]) | Cross-check test |
| N-021 | SDDR | FARIS shall output dose rate maps with relative error per voxel and a mask for voxels with error >10% | Map + mask | ITER <5-10% in regions of interest [R/U] | Default threshold 10%; show uncertain voxels hatched | Visual + numerical test |
| N-022 | SDDR | FARIS shall compute SDDR at user-defined access locations and compare with the limit (authored, labelled) | uSv/h vs limit | ITER 10 mSv/y, 100 uSv/h [R] | Pass/fail with upper bound | Test |
| N-023 | Activation | FARIS shall compute activation inventory per zone/material with pathways to the top 10 nuclides by contribution to decay heat and dose | Inventory, pathways | FISPACT-II [V] https://fispact.ukaea.uk | Match FISPACT-II reference cases within 10% on decay heat at 1 s..1e9 s (P) | Cross-code benchmark vs FISPACT-II/ALARA published cases |
| N-024 | Activation | FARIS shall support multiple irradiation histories (pulsed, multi-year, replacements) | History resolution | FISPACT-II [V] | Arbitrary piecewise history; 30-year run <=60 s for 1 zone (P) | Performance test |
| N-025 | Activation | FARIS shall quantify activation-data uncertainty (cross-section covariance / pathway) on dominant nuclides | Uncertainty on activity | FISPACT-II uncertainty [R] | 1-sigma reported for top nuclides | Compare with published case |
| N-026 | Decay heat | FARIS shall compute decay heat time dependence and total in kW per component | W/m3, kW vs time | FISPACT-II [V] | Decay heat at 1 s, 1 h, 1 d, 1 y; error <=10% vs reference case | Benchmark |
| N-027 | Waste | FARIS shall classify waste against selectable regulatory schemes (ITER/IAEA/NRC 10CFR61/UK) with date/version of scheme labelled | Class, margin | [U] | At least 3 schemes; each with stored citation | Test cases with known classification |
| N-028 | Waste | FARIS shall compute recycling/clearance indices (clearance at 50-100 y) | Index | [U] | Provided | Reference test |
| N-029 | Depletion | FARIS shall deplete Li-6 (and report enrichment drift) over reactor life by coupling TBR to burnup | Li-6 atom fraction vs time | OpenMC depletion [V] | TBR(t) over 30 y with replacement; TBR change <=1% tolerance vs fine step | Step-size convergence test |
| N-030 | Depletion | FARIS shall use step-size convergence test for depletion (predictor-corrector) and report error | Error vs dt | OpenMC [V] | Documented default | Test |
| N-031 | Source | FARIS shall generate a plasma neutron source from parametric (Fausser/Ballabio) with D-T and D-D | n/s, energy spectrum | openmc-plasma-source [V] | DT mean 14.06 MeV; FWHM versus T_i within 2% of Ballabio [R] | Compare with formula |
| N-032 | Source | FARIS shall accept externally supplied source (spatial + energy + angular) with unit and normalisation check | Total n/s | [U] | Normalisation error <0.1% | Test |
| N-033 | Source | FARIS shall compute neutron wall loading map (MW/m2) and total power from the source and verify against fusion power (14.1/17.6 fraction) | MW/m2 | ITER nominal 0.5 MW/m2 [R] | Closure <1% | Test |
| N-034 | Source | FARIS shall support time-dependent source (pulsed, ramp, outages) mapped to flux scaling | Source vs time | [U] | Piecewise-linear | Test |
| N-035 | Convergence | FARIS shall run MCNP-equivalent statistical checks (relative error, VOV, FOM trend, PDF slope) on each tally and label failing tallies | Checks pass/fail | MCNP 10 checks [V] https://mcnp.lanl.gov | All 10 or equivalents; fail => "not-evaluated" | Synthetic bad-tally test |
| N-036 | Convergence | FARIS shall support batch triggers: run until target relative error reached or budget exhausted | Trigger | OpenMC triggers [V] | Default 1% TBR, 10% deep mesh | Run test |
| N-037 | Convergence | FARIS shall report sigma as 1-sigma standard error and use correct (non-Gaussian-assumption) intervals for low-count tallies | Coverage | [U] | 95% coverage >=93% in synthetic tests | Coverage test |
| N-038 | Convergence | FARIS shall provide mesh convergence studies (refine voxel size, report change) | Change in total | [U] | Refinement check workflow | Test |
| N-039 | Variance reduction | FARIS shall generate weight windows automatically (FW-CADIS-like or random-ray) for deep shielding | FOM gain | FW-CADIS FOM x275 vs analog (x9 vs traditional) for ITER inboard TF heating [V*, doi 10.13182/NT175-251] | >=100x FOM improvement vs analogue on the magnet-behind-shield reference (P) | Compare FOM with and without |
| N-040 | Variance reduction | FARIS shall report FOM and its stability for every tally | FOM, trend | MCNP [V] | FOM reported; change <20% over last half (P) | Test |
| N-041 | Variance reduction | FARIS shall guarantee that weight windows do not bias results (compare to analogue on a small case within 3 sigma) | Bias check | [U] | Included in CI | Test |
| N-042 | Variance reduction | FARIS shall record all VR parameters in the receipt | Receipt fields | Avila Core receipts [internal] | 100% | Receipt schema test |
| N-043 | Geometry | FARIS shall import STEP/BREP via cad_to_dagmc / GEOUNED and report conversion time and mesh quality | Time, #surfaces | cad_to_dagmc [V] | 1000-solid model converts <10 min (P) | Timing test |
| N-044 | Geometry | FARIS shall perform overlap and gap checks before transport and block runs with >0 undefined regions | Overlap count | `openmc --geometry-debug` [V] | Zero overlaps allowed unless waived with warning | Test with injected overlap |
| N-045 | Geometry | FARIS shall report lost-particle fraction and fail a run above 1e-6 (P) | Lost fraction | MCNP aborts at 10 lost [V] | Fail >1e-6 per history, warn >0 | Injected-gap test |
| N-046 | Geometry | FARIS shall support CSG and DAGMC in the same model and state which is used for each cell | Label | [U] | Always shown | Test |
| N-047 | Geometry | FARIS shall check material assignment: density, temperature, composition, void coverage; fail on missing | Check | [U] | 100% cells with material or void | Test |
| N-048 | Geometry | FARIS shall measure geometry fidelity vs analytic reference volume (volume error <0.1% for tori, via stochastic volume) | Volume error | OpenMC volume calc [V] | <=0.1% | Test |
| N-049 | Geometry | FARIS shall import/export MCNP, OpenMC XML, and DAGMC h5m for interoperability | Formats | [V] | Round-trip equality of materials and cell volumes within 0.1% | Round-trip test |
| N-050 | Data | FARIS shall support selectable libraries: FENDL-3.2b, ENDF/B-VIII.1, JEFF-4.0, TENDL-2023, with checksum and version in the receipt | Libraries, hash | IAEA FENDL [V] | 4 libraries, hashed | Receipt test |
| N-051 | Data | FARIS shall use a single consistent library for neutron and photon/electron data unless explicitly overridden, and warn otherwise | Library consistency | Demo currently mixes FENDL-3.2 + ENDF/B-VII.1 photons [internal] | Warning shown on mix; mixed state labelled | Test |
| N-052 | Data | FARIS shall include thermal scattering (S(alpha,beta)) for hydrogenous/graphite/Be/FLiBe materials | S(a,b) per material | [V] | Applied where applicable | Test |
| N-053 | Data | FARIS shall propagate nuclear-data uncertainty via TMC or sensitivity (SANDY/perturbation) for TBR and magnet heating | Data sigma | DEMO HCPB TBR data uncertainty 3.2% (JEFF-3.2) to 8.6% (TENDL-2014) [V*, EPJ Web Conf. ND2016 09025] | TBR data-sigma within the 3-9% published band for the HCPB reference | Compare to published |
| N-054 | Data | FARIS shall flag nuclides lacking covariances or photon production data | Flag | [U] | All flagged | Test |
| N-055 | Data | FARIS shall verify temperature handling and cross-section interpolation (Doppler) for cryogenic/hot materials | Temperature | [U] | Test | Compare to a reference |
| N-056 | Performance | FARIS shall report transport throughput (histories/s/core) and parallel efficiency, and keep a regression benchmark | histories/s | OpenMC fixed-source CPU throughput: no source found [U]; measure locally | No more than 10% regression between releases | Performance CI |
| N-057 | Performance | FARIS shall scale to >=80% parallel efficiency to 32 threads on laptop-class CPU (P) | Efficiency | OpenMC strong scaling [R] | >=80% | Scaling test |
| N-058 | Performance | FARIS shall support GPU/event-based transport adapters if available and report speedup on identical seeds | Speedup | OpenMC A100 ~200 Xeon cores on fission benchmark [V*]; fusion unknown [U] | Optional; label as experimental | Test where available |
| N-059 | Performance | FARIS shall provide a "preview" tier (<=60 s) and a "production" tier with explicit uncertainty and target error | Tier runtime | OpenBNCT sprint rule [internal] | Preview <=60 s on laptop (P) | Timing test |
| N-060 | Performance | FARIS shall bound memory per run (cgroup cap) and refuse runs that would exceed it | RSS | Laptop memory guard [internal] | Refuse above cap; message with alternative | Test |
| N-061 | Reproducibility | FARIS shall record seeds, library hashes, code version, hardware threads, and make runs bit-reproducible at fixed threads | Reproducible | Core receipts [internal] | Same seed + threads => identical tallies | Run twice, compare |
| N-062 | Reproducibility | FARIS shall state statistical results are reproducible within sigma across thread counts | Agreement | [U] | Agreement within 3 sigma | Test |
| N-063 | UQ | FARIS shall propagate geometry/material uncertainties (density, enrichment, thickness) to TBR and heating by sampling/surrogate | Sensitivities | [U] | Sensitivity coefficients for the 10 largest parameters | Compare with finite differences |
| N-064 | UQ | FARIS shall separate MC error, nuclear-data error, modelling (geometry simplification) error in all totals | Three-way split | Best practice [U] | Always shown | Test |
| N-065 | Streaming | FARIS shall analyse port/penetration streaming with local mesh refinement and dedicated VR | Streaming enhancement factor | ITER streaming experiments FNG [R] | Test vs FNG streaming benchmark within +-30% | Benchmark |
| N-066 | Streaming | FARIS shall make cavity/void regions auditable (void cells flagged, no VR collapse) | Flags | [U] | Test | Test |
| N-067 | Mesh | FARIS shall support structured and unstructured mesh tallies with conservation check (integral over mesh = cell tally within 3 sigma) | Check | OpenMC [V] | Pass | Test |
| N-068 | Mesh | FARIS shall support mapping of mesh results to thermal/mechanical solvers with conservative transfer (energy conserved to 1e-6) | Conservation | [U] | 1e-6 | Test |
| N-069 | Units | FARIS shall carry units and per-source-neutron vs per-second normalisation explicitly through all tallies | Unit errors | Common trap [U] | Dimensional check; 0 unit errors in test suite | Unit tests |
| N-070 | Units | FARIS shall use per-cm3 vs per-m3 consistently; heating tallies per source particle converted with total source rate | Test | MC tallies already per cm3 (project memory) [internal] | Test | Test |
| N-071 | Safety | FARIS shall display dose/limits with sources and warn that results are not licensing-grade | Disclaimer | [U] | Always | Visual test |
| N-072 | Model | FARIS shall represent homogenised vs heterogeneous blanket models and report the TBR difference | Delta TBR | HCPB/HCLL homogenisation effect ~ few % [R] | Delta reported | Test |
| N-073 | Model | FARIS shall report model-simplification error from a reference high-fidelity model (e.g. 3D vs 1D radial build) | Delta | ITER C-lite vs E-lite [R/U] | Delta reported for the demo model | Test |
| N-074 | Model | FARIS shall support a 1D/2D/3D ladder with documented accuracy relative to 3D | Ladder | [U] | 1D within 10% of 3D on TBR for reference (P) | Test |
| N-075 | Photon | FARIS shall include photon production, electron/positron treatment (TTB or full), and gamma heating | Flags | MCNP/OpenMC [V] | Documented; TTB error <5% heating (P) | Test |
| N-076 | Adjoint | FARIS shall provide adjoint/importance maps as design guidance (where did the shield matter) | Importance map | CADIS [V] | Available | Reciprocity test |
| N-077 | Interop | FARIS shall import/export OpenMC statepoints, MCNP meshtal, and VTK/HDF5 for ParaView | Formats | [V] | 3 formats | Round-trip |
| N-078 | Interop | FARIS shall expose the transport adapter interface so MCNP/Serpent/TRIPOLI/Shift can be added | Interface | [U] | One non-OpenMC adapter proof by v1.0 (P) | Adapter test |
| N-079 | QA | FARIS shall run analytic transport tests (infinite medium, slab attenuation, 1/v, point source in void) with exact comparisons | Tests | [U] | >=20 analytic tests, all within 3 sigma | CI |
| N-080 | QA | FARIS shall run code-to-code (MCNP/OpenMC) comparison on ITER benchmark models (e.g. ITER Benchmark Model, FNG) | Delta | [U] | Within 3 sigma or 2% | Test |
| N-081 | QA | FARIS shall run mutation tests on checker (inject bias/lost geometry and confirm detection) | Detection | Avila Core mutation harness [internal] | 100% of injected faults detected | Test |
| N-082 | UX | FARIS shall explain every Unknown/Estimate (why + next step) in nuclear results | Explanation | Internal rule | 100% | UI test |
| N-083 | UX | FARIS shall show per-result uncertainty visually (error bars, mesh error map) and never display a number without sigma | Display | [U] | 100% | UI test |
| N-084 | UX | FARIS shall estimate run time and cost before a run (histories vs target error) | Prediction | [U] | Estimate within +-30% (P) | Test |
| N-085 | Security | FARIS shall treat imported CAD/library files as untrusted (size limits, parsers fuzzed) | Fuzz | [U] | No crash in 1e6 fuzz inputs | Fuzz test |
| N-086 | Export | FARIS shall export controlled data warnings (MCNP/RSICC data and export control; do not bundle) | Warning | RSICC [V] | No restricted data bundled | Review |
| N-087 | Time | FARIS shall support fine-grained temporal cooling-time grids and event sequences (replacement outages) for SDDR | Grid | [U] | Arbitrary | Test |
| N-088 | Documentation | FARIS shall document every nuclear model choice with reference and range of validity | Docs | [U] | 100% of options | Doc test |

## 3. Traps

1. Photon-filtered heating: in OpenMC a photon-only filtered heating tally is ~1e-4 of the physical value; the correct figure is photon + electron + positron (+neutron kerma) combined [V internal].
2. Per-source-neutron vs per-second normalisation, and per-cm3 vs per-m3; MC tallies are already per cm3 per source particle.
3. Small relative error is not accuracy. The relative error measures only sampling precision of the tally; geometry holes, undersampled regions, or biased weight windows give small errors and wrong values. Always pair with VOV/FOM and an analogue cross-check.
4. Low-statistics voxels show 0 or error "0" when no scores occurred; treat zero scoring as not-evaluated, not as zero dose.
5. Mixed nuclear data libraries (neutron from one, photon from another) silently change gamma heating and SDDR.
6. TBR depends strongly on model fidelity (ports, structure, homogenisation, 3D vs 1D); a 1D TBR can overshoot by 5-10% [U]. A "good" TBR from an idealised model must be labelled idealised.
7. Kerma vs energy deposition differ near interfaces and for photon heating; unqualified "heating" is ambiguous.
8. NRT dpa overestimates damage in metals at high energies versus arc-dpa; limits quoted in dpa are only valid for the same dpa convention.
9. Magnet limits (ITER Nb3Sn numbers) do not transfer to REBCO; limits depend on temperature, field and measurement conditions.
10. D1S assumes a single set of correction factors; it can mis-estimate when the irradiation history or material composition changes (needs R2S).
11. FOM improvement can be illusory when VR parameters are tuned on the same histories (use independent runs) and when weight windows are generated at the wrong energy/spatial resolution.
12. Statistical checks passing on 1M histories do not guarantee deep-shield convergence; many histories never reach the magnet.
13. Lost particles: a handful of lost particles may hide a leaky geometry; with CAD-to-DAGMC faceting, watertightness is the main failure mode.
14. GPU speedups quoted are often vs one CPU core, on simple models, without photon/electron transport; do not assume parity for full fusion models.
15. Benchmark C/E from the literature are from specific code+library+model versions; claiming "validated" without running them locally is a trap. Run and record your own.
16. Source-term simplifications (uniform ring source, no impurity/alpha spectrum) bias flux near the first wall.
17. Copyright/export-control: SINBAD data and MCNP libraries have access conditions; do not redistribute.

## 4. Verification status after the live pass

Verified [V*] (abstract/snippet level): OpenMC R2S FNG C/E (0.88/1.16/1.17); D1S vs R2S 4-15%; JET prior SDR agreement
~+-30%; FNG HCLL tritium production total uncertainty 5.9% (1 sigma); HCPB TBR data uncertainty 3.2-8.6% by library;
FW-CADIS FOM x275/x9/x21,000 (ITER); OpenMC GPU A100 ~200 Xeon cores and 70x (Intel GPU, fission problems); REBCO fluence
behaviour (E>0.1 MeV, 3.3e22 m-2 at 40-50 K); ITER insulation 10 MGy = 3.2e21 n/m2; FENDL-3.2b Feb 2022; ENDF/B-VIII.1 30
Aug 2024; C-lite ~10,000 volumes; E-lite is 360 degrees via SuperMC 7/SpaceClaim.

Still NOT verified, treat as unsourced: JET DTE2-specific C/E numbers (paywalled); SINBAD/FNS C/E ranges for flux, heating;
E-lite cell count; ITER TF total heating and Cu dpa limit; REBCO limit at 20 K; ITER 100 uSv/h and 10 mSv/y requirements;
ITER per-voxel error practice; FISPACT-II agreement band; fusion-model GPU speedups; OpenMC CPU histories/s/core;
FENDL-3.2b/ENDF-B-VIII.1 TBR covariance uncertainty; JEFF-4.0/TENDL-2023 release details; OpenMC feature-version claims
(random-ray, R2S) were not rechecked; MCNP statistical-check thresholds recalled from the manual, not re-fetched.
Acceptable-C/E "consensus" ranges in 1.1 remain [U] and should be framed as FARIS targets, not literature facts.
