# Activation, waste and materials

How FARIS turns transport results and an irradiation history into inventories, decay heat,
shutdown-dose source terms and waste classes (ACT), and how it holds the material data and
damage limits that decide when a component has had enough (MAT). Hardware and reference
models are in [the index](README.md#reference-hardware-and-models). Transport quantities and
the shutdown-dose method are in [02-radiation-transport.md](02-radiation-transport.md); the
safety screening that uses waste class and decay heat (SAFE) is in 04-plant-systems.md.
Accuracy against FISPACT-II, ORIGEN and fusion benchmarks belongs in
06-accuracy-and-validation.md, and uncertainty propagation in 07-uncertainty.md. ACTINV is the
Avila Labs inventory solver and a separate released tool; FARIS adapts it and inherits its
accuracy contract rather than reimplementing it. FARIS outputs are research screening, not a
licensing or safety claim. A limit with no stated conditions is not a limit: dpa, helium,
fluence and dose limits only mean something with their convention, temperature and spectrum.

## Activation workflow and adapters

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| ACT-001 | FARIS shall run activation and decay through an ACTINV adapter fed by transport spectra and an irradiation history. | End-to-end run on RM-S yields per-component inventory, activity and decay heat; receipt binds ACTINV version, data hash, history hash and spectrum hash. | End-to-end test and receipt schema test. | ACTINV is the in-house inventory solver [internal]; docs/TOOLING.md | F2 | No: ACTINV exists separately; no FARIS adapter yet |
| ACT-002 | FARIS shall hand transport spectra to the activation solver without losing flux. | Group mapping conserves the flux integral to 1e-9 relative and the reaction-rate integral to 1e-6 relative; group structure stored. | Conservation test on 5 spectra (see NUC-029). | FARIS choice | F2 | No |
| ACT-003 | FARIS shall offer an optional FISPACT-II adapter for cross-checks. | Same inputs run through both solvers and a per-nuclide, per-time ratio table produced; adapter absent without a licence leaves the feature disabled with an explanation. | Adapter test on a licensed machine. | FISPACT-II (UKAEA) [V](https://fispact.ukaea.uk) | F2 | No |
| ACT-004 | FARIS shall allow further inventory solvers (ALARA, ORIGEN, ACAB) behind the same adapter. | Adapter interface unchanged; ≥ 1 additional solver proves it by F6 (provisional: choice depends on access; confirm before F6 gate). | Adapter conformance test. | ALARA, ORIGEN and ACAB exist [V](https://github.com/svalinn/ALARA) | F6 | No |
| ACT-005 | FARIS shall produce a cross-solver comparison table on a standard material set. | ≥ 10 fusion alloys and compounds at 1 d, 1 y and 100 y; decay-heat and activity ratios per case; thresholds and verdicts in 06-accuracy-and-validation.md. | Comparison run with checker-derived verdicts. | Within 5 % on 10 alloys is R6's proposed bar [U] | F2 | No |
| ACT-006 | FARIS shall report every nuclide or pathway lacking data and shall not treat a missing pathway as zero. | Missing-data list per run; affected totals marked NOT_EVALUATED or lower bound with reason and next step. | Run with a removed cross-section file. | Fail-closed house rule | F2 | No |
| ACT-007 | FARIS shall run activation at zone or voxel granularity. | ≥ 10⁴ zones on RW; one zone over a 30-year history ≤ 60 s on RL (provisional: no baseline; confirm before F2 gate); per-zone runs scale per PERF-034. | Timing and scaling test. | 10⁵ zones take minutes to hours with parallelism [U] | F2 | No |
| ACT-008 | FARIS shall list the main production pathways. | Top 10 nuclides by contribution to decay heat and to dose at each cooling time, with the reaction paths that make them. | Compare with an ALARA or FISPACT-II pathway report on one case. | FISPACT-II pathway analysis [V](https://fispact.ukaea.uk) | F2 | No |
| ACT-009 | FARIS shall accept arbitrary piecewise irradiation histories. | Pulsed, multi-year and replacement histories; ≥ 10⁴ segments; each segment records flux scaling and duration. | History tests incl. the 30-year demo history. | FISPACT-II histories [V](https://fispact.ukaea.uk) | F2 | Partial: 30-year operating history with outages exists; not wired to activation |

## Irradiation history, depletion and convergence

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| ACT-010 | FARIS shall import the irradiation history from the operating model and record every lumping rule. | Lumping documented; error from lumping ≤ 1 % on decay heat at 1 d and 1 y against the unlumped history (provisional: confirm before F2 gate). | Lumped versus full history comparison. | FARIS choice | F2 | No |
| ACT-011 | FARIS shall show which nuclides are sensitive to pulsing. | List of nuclides whose inventory changes > 1 % between pulsed and averaged history. | Pulsed versus averaged run. | Short-lived nuclides depend on pulse structure [U] | F2 | No |
| ACT-012 | FARIS shall prove step-size convergence for depletion and activation. | Key results (TBR(t), inventories, decay heat) change < 1 % when steps are halved on the reference case; scheme and order documented. | Step-halving test in CI. | R6 depletion row; OpenMC predictor-corrector [V](https://docs.openmc.org) | F2 | No |
| ACT-013 | FARIS shall deplete lithium-6 over plant life and report enrichment drift and its effect on TBR. | Li-6 atom fraction versus time reported; TBR(t) over 30 years with replacements within 1 % of the fine-step run (provisional: confirm before F2 gate). | Step-convergence run on RM-S. | OpenMC depletion [V](https://docs.openmc.org); Li-6 burnup of several to tens of percent per full-power year for FLiBe is [U] | F2 | No |
| ACT-014 | FARIS shall define when a changed material state requires a new transport run. | Rerun triggered when the composition change moves TBR or heating by more than 1 % in a predictive estimate; trigger shown beside results (provisional: threshold is a FARIS choice; confirm before F2 gate). | Test on a Li-6 depletion sequence. | FARIS choice | F2 | No |
| ACT-015 | FARIS shall document the transport-depletion coupling scheme. | Scheme, step choice and replacement handling in the receipt and handbook; deviation from the documented scheme blocks the run. | Receipt audit. | FARIS choice | F2 | No |
| ACT-016 | FARIS shall conserve lithium and tritium atoms across breeding and burnup. | Atoms produced by (n,t) equal atoms removed from Li-6 and Li-7 to 1e-9 relative at every step. | Conservation test. | FARIS choice | F2 | No |
| ACT-017 | FARIS shall conserve nucleon and charge number across all reactions in the transmutation matrix. | 0 violations beyond 1e-12 relative on the matrix for the shipped library. | Matrix audit. | FARIS choice | F2 | No |
| ACT-018 | FARIS shall reproduce exact decay for simple chains. | Single nuclide and Bateman three-member chain agree with the analytic solution to 1e-12 relative. | Analytic test. | Standard analytic solution | F2 | No |
| ACT-019 | FARIS shall support cooling-time grids from 1 s to 1e9 s. | Grid selectable per study; results at requested times only, no interpolation unless labelled. | Grid test (see NUC-034). | FARIS choice | F2 | No |

## Decay heat, activity and shutdown dose source terms

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| ACT-020 | FARIS shall compute decay heat versus time per component, in W/m³ and kW. | At 1 s, 1 h, 1 d, 1 y at least; totals equal the sum over zones to 1e-9 relative; accuracy vs FISPACT-II in 06-accuracy-and-validation.md. | Sum test; cross-code table (ACT-005). | FISPACT-II reference [V](https://fispact.ukaea.uk); within 10 % is R1's proposal [U] | F2 | No |
| ACT-021 | FARIS shall split decay heat by nuclide, element and radiation type. | Photon, beta and alpha parts reported; the parts sum to the total to 1e-9 relative. | Sum test. | FISPACT-II output [V](https://fispact.ukaea.uk) | F2 | No |
| ACT-022 | FARIS shall report activity and contact dose-rate contribution per nuclide. | Activity in Bq and per-nuclide dose coefficient set named and versioned. | Receipt audit; hand calculation on 5 nuclides. | FARIS choice | F2 | No |
| ACT-023 | FARIS shall produce decay-photon source spectra per zone for shutdown-dose transport. | ≥ 100 energy groups or a line list; photons/s integrates to the inventory emission to 1e-6 relative. | Conservation test. | FARIS choice (provisional: group count not sourced; confirm before F2 gate) | F2 | No |
| ACT-024 | FARIS shall quantify activation-data uncertainty on dominant nuclides. | 1 σ on activity for the top 10 nuclides; method named; see 07-uncertainty.md for propagation. | Compare with a published case. | FISPACT-II uncertainty tools [U] | F6 | No |
| ACT-025 | FARIS shall rank impurity elements by their effect on activation, decay heat and waste class. | ≥ 10 impurities ranked by sensitivity; ranking agrees with finite-difference sensitivities within 20 %. | Finite-difference check. | Impurities in steel (Nb, Ag, Co) dominate waste class [U] | F2 | No |
| ACT-026 | FARIS shall hand decay heat per component to safety screening. | Decay heat versus time from 1 s to 1 y with σ and kind label for SAFE (see 04-plant-systems.md); adiabatic-rise screening is a SAFE requirement, not here. | Interface test. | FARIS choice | F4 | No |
| ACT-027 | FARIS shall track tritium and carbon-14 produced in structural and non-breeder materials. | Inventory per component over life; tritium handed to the fuel-cycle model with atom conservation to 1e-9. | Conservation test. | FARIS choice | F4 | No |
| ACT-028 | FARIS shall check that helium and hydrogen from activation agree with the transport tallies. | Inventory He and H agree with NUC-022 tallies within 3 σ. | Cross-check on RM-S. | FARIS choice | F2 | No |
| ACT-029 | FARIS shall write shutdown-dose source files with a hash and unit stamp. | Source file hash recorded; unit and normalisation (photons/s/cm³) stated; matches NUC-035 conservation. | Receipt and conservation test. | FARIS choice | F2 | No |

## Waste classification and clearance

Waste classification here is research screening against published schemes, not a regulatory determination.

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| ACT-030 | FARIS shall classify activated components as Class A, B, C or greater than Class C against 10 CFR 61.55 using the table-driven sum-of-fractions rule. | Table values read from a stored copy of the regulation with its retrieval date; 100 % of ≥ 20 hand-computed test cases give the right class. | Table test against the stored eCFR snapshot and hand-computed cases. | 10 CFR 61.55 [V](https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55) | F2 | No |
| ACT-031 | FARIS shall encode Table 1 (long-lived nuclides) exactly. | Ci/m³: Nb-94 in activated metal 0.2, Ni-59 in activated metal 220, C-14 8 (80 in activated metal), Tc-99 3, I-129 0.08; encoded values equal the snapshot byte for byte. | Table checksum test. | 10 CFR 61.55 Table 1 [V](https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55) | F2 | No |
| ACT-032 | FARIS shall encode Table 2 (short-lived nuclides) exactly. | Ci/m³ for Class A, B, C: H-3 40 (no B or C limit), Co-60 700 (none), Ni-63 3.5, 70, 700 (activated metal 35, 700, 7000), Sr-90 0.04, 150, 7000, Cs-137 1, 44, 4600; encoded values equal the snapshot. | Table checksum test. | 10 CFR 61.55 Table 2 [V](https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55) | F2 | No |
| ACT-033 | FARIS shall offer several classification schemes with scheme name, version and date on every result. | ≥ 4 schemes: NRC 10 CFR 61.55, IAEA clearance, UK LLW/ILW thresholds, and Fetter specific-activity limits; each with citation; scheme choice stored in the receipt. | Cases with known classification per scheme. | Fetter, Cheng and Mann derive specific-activity limits for long-lived nuclides [V](https://fetter.it-prod-webhosting.aws.umd.edu/sites/default/files/fetter/files/1990-FED-RadWaste.pdf); IAEA and UK schemes [U] | F2 | No |
| ACT-034 | FARIS shall distinguish activated metal from other waste forms. | Waste form stored per component; the activated-metal limits used only where the form applies; form shown in the result. | Test with the same inventory in both forms. | 10 CFR 61.55 Tables 1 and 2 have activated-metal columns [V](https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55) | F2 | No |
| ACT-035 | FARIS shall apply the sum-of-fractions rule when several nuclides are present and show each nuclide's fraction. | Fractions listed and summing to the reported total; class changes at sum = 1 within 1e-9. | Edge-value tests. | 10 CFR 61.55 [V](https://www.ecfr.gov/current/title-10/chapter-I/part-61/subpart-D/section-61.55) | F2 | No |
| ACT-036 | FARIS shall compute a clearance index and the time at which it falls below 1. | Σ(C_i/CL_i) with the clearance-level table named and versioned; clearance time searched over 1 to 100 years. | Test with a single-nuclide decay case. | IAEA clearance sum rule [U] | F2 | No |
| ACT-037 | FARIS shall compute a recycling index against an authored dose-rate criterion. | Criterion labelled authored with its source; index reported with σ; no number shown without its criterion. | Test cases. | FARIS choice | F2 | No |
| ACT-038 | FARIS shall report waste mass and volume by class over plant life including replacements. | Component totals sum to the plant total to 1e-9 relative; every replacement batch counted once. | Sum test on the 30-year history. | FARIS choice | F4 | No |
| ACT-039 | FARIS shall refuse a waste class when required nuclides or impurity data are missing. | Classification returns NOT_EVALUATED with the missing nuclides and next step; never defaults to Class A. | Test with a truncated inventory. | Fail-closed house rule; waste class depends on Nb, Ni, C, Tc and impurity content [U] | F2 | No |
| ACT-040 | FARIS shall classify at a stated disposal time. | Decay-corrected to the stated time; changing the time changes the receipt. | Decay-time test. | FARIS choice | F2 | No |

## Quality, determinism and records

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| ACT-041 | FARIS shall handle a zero or empty spectrum without error. | Zero flux gives exact zero inventories, no NaN and no divide error; a message names the cause. | Zero-spectrum test. | ACTINV zero-spectrum fix, 2026-09-19 [internal] | F2 | No |
| ACT-042 | FARIS shall give identical activation results for identical inputs. | Bit-identical outputs over 10 repeat runs; cache key covers spectrum, history, data and solver version (see AUTO-033). | Repeat and mutation tests. | Core receipts [internal] | F2 | No |
| ACT-043 | FARIS shall recompute decay over a cached inventory instantly when only cooling time changes. | Cooling-time edit on a cached 30-year inventory P95 ≤ 100 ms on RL for plant-level decay heat (see PERF-020). | Benchmark. | PERF-020 | F2 | No |
| ACT-044 | FARIS shall keep an analytic activation test suite. | ≥ 20 cases (Bateman chains, saturation activity, branching) within 1e-9 relative. | CI run. | FARIS choice | F2 | No |
| ACT-045 | FARIS shall record the activation library and decay-data versions in every result. | Library, evaluation year and hash in the receipt; a change blocks cache reuse. | Receipt and mutation test. | FARIS choice | F2 | No |
| ACT-046 | FARIS shall explain every activation Unknown or Estimate. | 100 % carry why and the next step on hover or tap and as text. | UI test. | House rule | F2 | No |
| ACT-047 | FARIS shall mark every activation number with a kind label. | Calculated, authored, literature, conditional or not-evaluated on 100 % of numbers. | Schema and UI audit. | House rule | F2 | No |
| ACT-048 | FARIS shall prove its activation checkers catch faults. | 100 % of injected faults (wrong history scaling, dropped nuclide, stale spectrum) detected. | Mutation harness. | Core mutation harness [internal] | F2 | No |
| ACT-049 | FARIS shall label waste and activation outputs as research screening. | The label appears on 100 % of waste, clearance and dose-source outputs; no output states that a regulation is met. | Export audit. | House rule | F2 | No |

## Material property database

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-001 | FARIS shall hold a versioned material-property database. | Each row has property, value, unit, valid temperature and fluence range, citation, kind label and status; 100 % of rows complete. | Schema and lint test. | FARIS choice | F2 | Partial: candidate recipes file with provenance notes; no database |
| MAT-002 | FARIS shall keep material recipes independent of geometry and display names. | Recipe swap changes only that recipe's hash (see GEO-042). | Hash test. | MATERIAL_BASELINE [internal] | F1 | Partial: recipes for W, Inconel 718, FLiBe, Be, TiH2 recorded as candidates, not qualified |
| MAT-003 | FARIS shall store base composition and maximum impurity specification per material. | Fractions sum to 1 ± 1e-9; impurity limits in wt ppm per element; both labelled nominal, specified, measured or assumed. | Schema test. | FARIS choice | F2 | No |
| MAT-004 | FARIS shall give density and other temperature-dependent properties as cited functions with valid ranges. | Evaluation outside the range returns NOT_EVALUATED with the range and next step; inside it matches the cited table to 1e-6 relative. | Table test; out-of-range test. | MATERIAL_BASELINE: density basis is not documented for enriched salt [internal] | F3 | No |
| MAT-005 | FARIS shall hold the properties other physics needs. | Thermal conductivity, expansion, modulus, yield, creep and electrical resistivity listed per material where cited data exist; missing rows listed. | Coverage report. | FARIS choice | F3 | No |
| MAT-006 | FARIS shall mark any material or property without data as not evaluated with reason and next step. | 100 % of missing entries show NOT_EVALUATED, never a default value. | Audit of every consumer of the database. | House rule | F2 | Partial: policy stated; unaudited |
| MAT-007 | FARIS shall label every property value as authored, literature, calculated, conditional or not evaluated. | 100 % labelled; a literature value cites its source. | Schema test. | House rule | F2 | No |
| MAT-008 | FARIS shall model degradation of properties with dose and temperature as authored curves. | Property(dpa, T) tables with provenance; unlabelled extrapolation blocked. | Table and extrapolation test. | FARIS choice | F3 | No |
| MAT-009 | FARIS shall validate the database in CI. | Schema valid; no duplicate rows; every citation present; every DOI resolves in a monthly job. | CI lint and link check. | FARIS choice | F2 | No |

## Damage limits and conditions

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-010 | FARIS shall hold a limit table where each limit carries its convention, temperature window, spectrum, conditions, citation and status. | Fields: quantity (dpa, He appm, H appm, fluence, dose), convention, irradiation temperature, source, status (design aspiration, code rule or measurement); 100 % of rows complete. | Schema test and review of 100 % of rows by checker. | 20, 50 and 100 dpa and 1 appm He are design aspirations or conservative rules, not material properties [U] | F2 | No |
| MAT-011 | FARIS shall compare a result with a limit only when the dpa convention matches. | NRT, arc-dpa and other conventions separate; mismatch gives NOT_EVALUATED (see NUC-024). | Mismatch test. | NRT overestimates damage in metals against arc-dpa [U] | F2 | No |
| MAT-012 | FARIS shall check helium and hydrogen limits as well as dpa and report whichever is reached first. | Time-to-limit per criterion; the governing criterion named; dpa-only verdicts refused for materials with a gas limit. | Test with a gas-limited case. | He weldability limit of about 1 appm for conventional welding [U] | F2 | No |
| MAT-013 | FARIS shall give time to limit with a sampling-error band and a P95 value. | Years to limit from fluence rate with σ; P95 value shown; feeds replacement scheduling. | Test with synthetic rates. | Existing demo replaces at authored fluence thresholds [internal] | F2 | Partial: replacement at authored thresholds, no σ band |
| MAT-014 | FARIS shall declare a component passing only when the 2 σ upper bound of its damage, including data uncertainty, is below the limit. | Pass needs the upper bound; statistically unconverged tallies give NOT_EVALUATED (see 07-uncertainty.md). | Edge-value tests. | FARIS choice | F2 | No |
| MAT-015 | FARIS shall check that a limit's stated conditions apply to the case. | Temperature, spectrum and material form tested; outside conditions the limit is shown as conditional, not literature. | Out-of-condition test. | REBCO and steel limits depend on irradiation temperature and spectrum [U] | F2 | No |
| MAT-016 | FARIS shall carry limit rows for the materials in the reference build. | At least one limit row, or an explicit not-evaluated row with reason, for EUROFER97, tungsten, CuCrZr, Inconel 718, SiC/SiC, REBCO and magnet insulator. | Coverage test. | EU DEMO starter blanket about 20 dpa then 50 dpa for EUROFER [U] | F3 | No |
| MAT-017 | FARIS shall track tungsten transmutation products and their effect. | Rhenium and osmium fractions per year from ACT; effect on conductivity or embrittlement marked not evaluated unless a cited model exists. | Test with ACT output. | W transmutation to Re and Os [U] | F3 | No |
| MAT-018 | FARIS shall trigger component replacement from the governing limit and record the reason. | Replacement event stores the criterion, value and limit; replacement resets only that component's accumulated damage. | History test. | Operating history semantics [internal] | F4 | Partial: replacement resets named component fluence |
| MAT-019 | FARIS shall refuse to infer service life from a transport proxy. | A proxy (energy-integrated flux) never appears as dpa or life; the label names it a proxy. | Label audit. | Project rule: no life from flux alone [internal] | F2 | Met: demo labels its fluence variable a proxy and states it is not dpa (docs/OPERATING_HISTORY.md) |

## Temperature windows

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-020 | FARIS shall hold an operating temperature window per material with the reason for each bound. | Lower bound reason (embrittlement, phase) and upper bound reason (creep, melting) stored; reduced-activation steel window of about 350 to 550 °C is [U]. | Schema test. | Gaganidze and Zinkle reviews [U] | F3 | No |
| MAT-021 | FARIS shall warn and give a verdict on temperature excursions. | Excursion margin and duration reported; steady value outside window marks the component not within window. | Test with synthetic profiles. | FARIS choice | F3 | No |
| MAT-022 | FARIS shall take temperatures from the thermal model and label authored values otherwise. | Source of every temperature stated (calculated by THM or authored); see 04-plant-systems.md. | Label audit. | House rule | F3 | Partial: material temperatures authored |
| MAT-023 | FARIS shall record irradiation temperature beside every damage value. | 100 % of damage values store temperature; limits compared only where irradiation temperature matches the limit's window. | Test with mismatched temperature. | Irradiation temperature changes REBCO degradation [V](https://doi.org/10.1088/1361-6668/aaadf2) | F3 | No |
| MAT-024 | FARIS shall check the physical state of liquid materials at operating temperature. | FLiBe and PbLi state checked against a cited melting point and an operating window; a frozen or boiling state blocks the case. | State test at 3 temperatures. | ARC FLiBe at 800 K inlet and 900 K outlet [V](https://doi.org/10.1016/j.fusengdes.2015.07.008) | F3 | Partial: policy notes FLiBe is not liquid at 293.6 K; no state check |
| MAT-025 | FARIS shall hold cryogenic windows for magnet materials. | Temperature range per magnet material and test condition; cold-data cases labelled. | Schema test. | FARIS choice | F3 | No |

## Magnet materials

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-030 | FARIS shall store REBCO fluence limits with energy cut-off, irradiation temperature and tape type. | Every limit states cut-off (E > 0.1 MeV), temperature and presence of artificial pinning; Ic degraded below initial after 3.3e22 m⁻² at 40 to 50 K in the cited study; no limit shown without all three. | Schema test and comparison-mismatch test. | Fischer et al. 2018 [V](https://doi.org/10.1088/1361-6668/aaadf2) | F3 | No |
| MAT-031 | FARIS shall refuse to compare a fluence with a REBCO limit that uses a different energy cut-off or temperature. | Mismatch gives NOT_EVALUATED naming the difference (see NUC-017). | Mismatch test. | Fluence quoted without cut-off or temperature is not comparable [U] | F3 | No |
| MAT-032 | FARIS shall apply a selectable Ic(fluence, temperature, field) model and report margin. | Each model carries its source table and valid range; margin reported as retained fraction of Ic with σ band. | Check against published curves (see 06-accuracy-and-validation.md). | Ic first rises then degrades with fluence [V](https://doi.org/10.1088/1361-6668/aaadf2) | F3 | No |
| MAT-033 | FARIS shall show that degradation fluence depends on temperature and pinning. | Higher irradiation temperature and artificial pinning lower the degradation fluence in the cited study; limits at 20 K and below are higher and are not assumed. | Data-table test. | Fischer et al. 2018 [V](https://doi.org/10.1088/1361-6668/aaadf2) | F3 | No |
| MAT-034 | FARIS shall state the critical-temperature loss model used, or mark it not evaluated. | Model named or NOT_EVALUATED; one study reports about 3 % loss per 1e22 m⁻² while another source says about 1 K per few 1e22 and the figures conflict. | Data-table test. | Fischer 2018 [U] (figures conflict between research notes) | F3 | No |
| MAT-035 | FARIS shall model or flag annealing and warm-up effects on irradiated tape. | Option on or off, stated in the result; off marks results conditional on no annealing. | Label test. | Annealing during warm-up changes limits [U] | F3 | No |
| MAT-036 | FARIS shall store the insulator dose limit with its insulator type and basis. | ITER TF insulation specification 1e7 Gy (10 MGy); 10 MGy corresponds to fast fluence up to 3.2e21 n/m² for that design; polyimide near 1e8 Gy is [U]; each row names the insulator. | Schema test. | ITER Design of the Magnets [V](https://www.osti.gov/etdeweb/servlets/purl/20641471); fluence equivalence [V](https://www.tandfonline.com/doi/abs/10.13182/FST09-A8985) | F3 | No |
| MAT-037 | FARIS shall check copper stabiliser damage against a stated limit and resistivity effect. | dpa limit stored with source and temperature; roughly 6e-5 dpa quoted for ITER Cu is [U]; result marked conditional until sourced. | Schema test. | R1 N-014 [U] | F3 | No |
| MAT-038 | FARIS shall combine REBCO, insulator and copper limits and name the one reached first. | Time to each limit with σ; governing item named; magnet lifetime equals the minimum. | Test with synthetic rates. | FARIS choice | F4 | Partial: demo magnet trigger is an authored threshold only |
| MAT-039 | FARIS shall never carry ITER Nb3Sn limits over to REBCO. | Limit rows for Nb3Sn marked not applicable to REBCO; any attempted use gives NOT_EVALUATED. | Test with a Nb3Sn row against a REBCO case. | Magnet limits do not transfer [U] | F3 | No |

## Impurity specifications

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-040 | FARIS shall take an impurity specification as an input and show its effect on activation, decay heat and waste class. | Each specification stored with source and kind (nominal, specified, measured, assumed); effect shown per element. | Run with two specifications. | Impurities in steel dominate waste class [U] | F2 | No |
| MAT-041 | FARIS shall keep a watch-list of activation-critical elements. | Includes Nb, Mo, Ag, Co, Gd, Tb and Ho; each with the reason and the waste-class threshold it endangers. | List test. | Fetter: Nb, Mo, Gd, Tb and Ho limits are severe for first-wall waste [V](https://fetter.it-prod-webhosting.aws.umd.edu/sites/default/files/fetter/files/1990-FED-RadWaste.pdf); Ag and Co [U] | F2 | No |
| MAT-042 | FARIS shall compute the maximum allowed impurity level for a target waste class. | Inverse calculation agrees with the forward classification to 5 % on 10 cases. | Forward-inverse consistency test. | FARIS choice | F2 | No |
| MAT-043 | FARIS shall refuse a waste verdict without a stated impurity specification. | Missing specification gives NOT_EVALUATED with the elements needed and next step; no silent default. | Test with no specification. | Fail-closed house rule | F2 | No |
| MAT-044 | FARIS shall record the provenance of each impurity value. | 100 % of impurity rows labelled nominal, specified, measured or assumed with source. | Schema test. | House rule | F2 | No |

## Data governance and citations

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| MAT-050 | FARIS shall store a citation, page or table reference and quoted value for every limit. | 100 % of limit rows have all three; a checker compares stored values to the quotation. | Lint test. | FARIS choice | F2 | No |
| MAT-051 | FARIS shall version the database and bind that version to every verdict. | Database version in every receipt; a database change invalidates dependent verdicts (see AUTO-033). | Mutation test. | FARIS choice | F2 | No |
| MAT-052 | FARIS shall mark user overrides of a limit as authored overrides. | Override carries author, time and reason; the result is labelled authored override and never literature. | Override test. | House rule | F2 | No |
| MAT-053 | FARIS shall not claim compliance with design codes. | Simplified checks may cite RCC-MRx or ASME III Division 4; no output states full code compliance. | Export audit. | R2 trap: full code compliance not claimed [U] | F3 | No |
| MAT-054 | FARIS shall block extrapolation of a property beyond its cited range. | Extrapolation returns NOT_EVALUATED; interpolation method named. | Range test. | FARIS choice | F2 | No |
| MAT-055 | FARIS shall record the licence and redistribution status of every property source. | No proprietary handbook data (for example restricted material handbooks) in the package; status stored per source. | Package audit. | Licensing practice [U] | F2 | No |
| MAT-056 | FARIS shall export the limit and property tables with citations. | CSV and JSON export; round trip lossless. | Round-trip test. | FARIS choice | F2 | No |
| MAT-057 | FARIS shall hold an optional uncertainty or range for each property for propagation. | Range or σ with basis per property or a not-evaluated flag; see 07-uncertainty.md. | Schema test. | FARIS choice | F6 | No |

## Traps

- A dpa limit without its convention is meaningless. A value in NRT dpa compared with an arc-dpa result will pass or fail for the wrong reason.
- Helium and hydrogen limits can bite before dpa does. A dpa-only verdict for a gas-limited material is a trap.
- Limits such as 20, 50 or 100 dpa are design aspirations or conservative rules. Present them as authored with conditions, not as material properties.
- REBCO fluence quoted without energy cut-off, irradiation temperature and tape type cannot be compared with anything. Neither can a fluence from an energy-integrated flux.
- Waste class swings with a few ppm of Nb, Ag or Co in steel. A class computed with an assumed clean specification is a guess.
- Decay-heat agreement with a reference code at one cooling time says little about other times; check 1 d, 1 y and 100 y.
- A coarse irradiation history that lumps pulses can hide short-lived nuclides. Report which nuclides move when the lumping changes.
- Waste classification uses a stored copy of a regulation. A regulation can change; show the retrieval date next to every class.
