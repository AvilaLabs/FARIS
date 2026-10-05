# R5: Software quality, performance, reliability, security, V&V, platforms (FARIS requirements research, 2026-10-05)

Provenance note: a second pass fetched live primary pages for the numbers listed in the 'Verified live 2026-10-05' block below; those are [V] with the deep link given. Everything else tagged [U] (or tagged [V] in the pre-pass text but absent from the block) is from memory and must be re-checked against the cited source before it is copied into a normative requirement. Treat any untagged-in-block number as [U].

Verified live 2026-10-05 [V]:
- SQLite: 100% branch coverage and 100% MC/DC; test code 92,053.1 KSLOC vs library 155.8 KSLOC = 590x; 1,184 testcase() uses; 6,754 asserts; dbsqlfuzz ~500M cases/day; 50,362 TH3 cases. https://www.sqlite.org/testing.html
- OpenSSF gold badge: >=90% statement coverage, >=80% branch coverage, reproducible build MUST, >=2 unassociated significant contributors. https://www.bestpractices.dev/en/criteria/2
- CRA (Reg. (EU) 2024/2847): in force 10 Dec 2024; reporting obligations from 11 Sep 2026; main obligations from 11 Dec 2027; Commission guidance published 27 Jul 2026. https://digital-strategy.ec.europa.eu/en/policies/cyber-resilience-act . The 24 h / 72 h / 14 d clocks and 5-year support period (Arts. 13-14) remain [U]: the EUR-Lex fetch returned only a summary; check https://eur-lex.europa.eu/eli/reg/2024/2847/oj Art. 13-14.
- OSS-Fuzz as of Aug 2023: >10,000 vulnerabilities, 36,000 bugs, 1,000 projects. https://google.github.io/oss-fuzz/
- Bazel test timeouts: small 60 s, medium 300 s, large 900 s, enormous 3600 s. https://bazel.build/reference/test-encyclopedia
- Android vitals bad-behaviour thresholds: user-perceived crash 1.09% overall (8% per phone model), ANR 0.47% overall. https://developer.android.com/topic/performance/vitals
- ICSBEP 2024 edition: 598 evaluations, 5,168 critical/near-critical/subcritical configurations (+51 alarm/shielding, +246 fundamental physics). https://www.oecd-nea.org/jcms/pl_114334/international-handbook-of-evaluated-criticality-safety-benchmark-experiments-new-release (via search result)
- SINBAD: 101 experiments (47 reactor shielding, 31 fusion neutronics, 23 accelerator shielding); fusion set includes FNG-ITER bulk shield, streaming, dose-rate, OKTAVIAN, FNS. https://oecd-nea.org/science/wprs/shielding/sinbad/
- Code-signing cert max validity 460 days (CA/B Forum CSC-31, effective 1 Mar 2026; CAs issuing 459 d from 24 Feb 2026). https://knowledge.digicert.com/alerts/code-signing-certificates-459-day-validity
- SLSA v1.0 Build L1 provenance exists (may be unsigned), L2 hosted platform + signed provenance, L3 hardened (isolated builds, signing secrets unreachable by build steps). https://slsa.dev/spec/v1.0/levels
- OpenSSF Scorecard: ~23 checks each 0-10; aggregate is risk-weighted (critical 10x, high 7.5x, medium 5x, low 2.5x); NO official recommended threshold exists, so my '>=7 healthy' is a FARIS choice, not a benchmark. https://github.com/ossf/scorecard
- Blender autosave default: page fetch did not show the value; [U] 2 min.
- Not verified: ASME/NRC/DOE/IAEA/ANS edition numbers, MCNP/SCALE/OpenMC suite sizes, startup/memory figures of VS Code/Blender/Zed/Figma (no primary cross-app benchmark found; PF-01/02/10/17 targets are provisional and must be baselined), NRC fusion rule status, export-control scope, nuclear data licence terms. Source URLs are the primary documents to check. Nothing here is legal advice (export control, CRA, licensing: confirm with counsel).

---------------------------------------------------------------------------
## 1. Best-in-class landscape

### 1.1 Quality model
- ISO/IEC 25010:2023 product quality model has NINE characteristics (2011 had eight): Functional suitability, Performance efficiency, Compatibility, Interaction capability (renamed from Usability), Reliability, Security, Maintainability, Flexibility (renamed from Portability), Safety (new). [V] https://www.iso.org/standard/78176.html
- ISO/IEC 25023:2016 supplies the measure catalogue (base/derived measures per sub-characteristic: e.g. functional correctness = 1 - A/B, A = incorrect functions, B = functions considered; MTBF; mean recovery time; fault-removal ratio). It still references the 2011 model; 2023 characteristics (Safety, Interaction capability, Flexibility) have no finished 25023 measures yet, so FARIS must define its own for those. [U] https://www.iso.org/standard/35747.html
- Practical rule: every FARIS quality requirement below is tagged to a 25010 characteristic and has a pass/fail or numeric metric.

### 1.2 Nuclear / scientific software QA reference standards
- ASME NQA-1 (2019/2022 editions), Part II Subpart 2.7 "Quality Assurance Requirements for Computer Software for Nuclear Facility Applications": software classification, graded approach, requirements/design/implementation/verification/validation/acceptance test, configuration management, error reporting and corrective action, access control, media control, retirement; also covers acquired (COTS/open-source) software via evaluation/dedication. Requirement 3 (Design control) and Requirement 11 (Test control) tie in. [V content, U edition years] https://www.asme.org/codes-standards/find-codes-standards/nqa-1-quality-assurance-requirements-nuclear-facility-applications
- NUREG/BR-0167 (1993), Software Quality Assurance Program and Guidelines (NRC research/regulatory computer codes: SQA plan, requirements, design, implementation, testing, configuration mgmt, error reporting). [V] https://www.nrc.gov/reading-rm/doc-collections/nuregs/brochures/br0167/
- NRC Regulatory Guides for safety-system software (digital I&C, not analysis codes, but the lifecycle template): RG 1.168 (V&V, reviews, audits; IEEE 1012), 1.169 (config management; IEEE 828), 1.170 (test documentation; IEEE 829), 1.171 (unit testing; IEEE 1008), 1.172 (software requirements specs; IEEE 830), 1.173 (life cycle processes; IEEE 1074). [V] https://www.nrc.gov/reading-rm/doc-collections/reg-guides/
- NRC RG 1.203 (2005) Transient and Accident Analysis Methods: Evaluation Model Development and Assessment Process (EMDAP): 4 elements: establish requirements, develop assessment base, develop the model, assess adequacy; includes PIRT. [V] https://www.nrc.gov/docs/ML0535/ML053580584.pdf (check)
- CSAU (Code Scaling, Applicability, Uncertainty), NUREG/CR-5249 (1989), Boyack et al., Nucl. Eng. Des. 119 (1990): 14 steps in three elements: requirements and code capabilities (PIRT), assessment and ranging of parameters, sensitivity and uncertainty analysis. PIRT: Wilson & Boyack, Nucl. Eng. Des. 186 (1998) 23-37. [V]
- DOE O 414.1D (2011, Chg 2 2020) Quality Assurance, with DOE G 414.1-4 "Safety Software Guide": 10 safety-software work activities (software project management & quality planning, risk & hazard analysis, requirements & design, software design, implementation, verification & validation, configuration management, procurement/supplier, maintenance, training/ ... ), plus Toolbox codes list for DOE accident analysis (MACCS2, ALOHA, etc.) that went through a gap analysis. [V gist, U count] https://www.directives.doe.gov/directives-documents/400-series/0414.1-BOrder-d-admchg2
- IAEA SSG-2 (Rev.1, 2019) Deterministic Safety Analysis for NPPs: section on code validation against separate-effects and integral experiments, user effects, and uncertainty (BEPU). IAEA Safety Reports Series 23 (Accident Analysis for NPPs) and TECDOC-1052 / Safety Report 52 for code qualification. [U] https://www.iaea.org/publications/12342
- ANS standards: ANS-10.4-2008 (R2016) Verification and Validation of Non-Safety-Related Scientific and Engineering Computer Programs for the Nuclear Industry; ANS-10.5-2006 Accommodating User Needs in Scientific and Engineering Computer Software Development; ANS-10.3 documentation/ selection guidance; ANS-19.5? (fusion-relevant) not verified. [U] https://www.ans.org/standards/
- ASME V&V 10-2019 (computational solid mechanics), V&V 20-2009 (R2021) (CFD/heat transfer: validation uncertainty u_val = sqrt(u_num^2 + u_input^2 + u_D^2), comparison error E vs u_val), V&V 40-2018 (credibility of computational models by risk-informed context of use; medical devices but the framework generalises). [V] https://www.asme.org/codes-standards/find-codes-standards/v-v-20-standard-verification-validation-computational-fluid-dynamics-heat-transfer
- Oberkampf & Roy, "Verification and Validation in Scientific Computing", Cambridge UP 2010; Oberkampf & Trucano 2002 (Prog. Aerosp. Sci. 38): verification (solving equations right) vs validation (right equations) vs predictive capability; PCMM (Predictive Capability Maturity Model, Sandia SAND2007-5948) scores six elements (representation/geometric fidelity, physics & material model fidelity, code verification, solution verification, validation, uncertainty quantification) on levels 0-3. [V]
- Method of manufactured solutions: Roache, "Code verification by the method of manufactured solutions", J. Fluids Eng. 124 (2002); Salari & Knupp SAND2000-1444. Observed order of accuracy must match formal order (e.g. 2.00 +/- 0.1 under grid refinement). [V] 
- Code-to-code comparison is verification-adjacent evidence only; independent codes sharing nuclear data or common bugs agree for wrong reasons.

### 1.3 How the transport codes run V&V (reference standards)
- ICSBEP Handbook 2024: 598 evaluations, 5,168 configurations [V]; used for criticality validation. https://www.oecd-nea.org/jcms/pl_24498/
- SINBAD (Shielding Integral Benchmark Archive and Database): 101 experiments (31 fusion neutronics) [V], incl. FNG-ITER, FNG-HCPB TBM, FNS/JAEA benchmarks, ASPIS, OKTAVIAN. [U count] https://www.oecd-nea.org/science/wprs/shielding/sinbad/
- MCNP6.x: criticality validation suite from ICSBEP (several hundred to > 1000 cases in Kiedrowski/LANL reports), a regression suite run on every build, and "Known Issues" lists; MCNP is distributed via RSICC / NEA Data Bank with export screening. [U numbers] https://mcnp.lanl.gov/
- SCALE 6.3 (ORNL): SCALE VALID / VALIDATION with several hundred to a thousand+ ICSBEP experiments across criticality; regression suite ~ thousands of test inputs; Nuclear QA program is NQA-1 compliant (ORNL SCALE QA, "NQA-1 2008 w/ 2009 addenda"). [U] https://www.ornl.gov/scale
- OpenMC: GitHub Actions regression suite (tests/regression_tests, ~ 100-200 test directories) comparing against stored result hashes with fixed seeds; separate `openmc-validation`/ICSBEP and benchmark repos (openmc-dev/benchmarks) [U numbers]. https://docs.openmc.org/en/stable/devguide/tests.html
- Fusion neutronics benchmark C/E targets used in literature: FNG-ITER bulk shield / streaming experiments: calculated vs measured reaction rates typically within +/-5-20 %, shutdown dose rate benchmarks (FNG-ITER dose rate, Villari et al.) within +/-10-30 % with 2-sigma overlapping measurement uncertainty. [U] SINBAD ; Fusion Eng. Des. papers (Villari, Fischer, Loughlin). Defer exact values to R1/R2 neutronics area.
- "Qualified for licensing use": for fission this means NRC acceptance of the evaluation model under EMDAP/CSAU via topical report (typically multi-year effort: code development over many years; NRC review of a topical report planned at ~12 months nominal with RAIs often extending to 18-36 months [U]) plus a QA programme meeting 10 CFR 50 App. B (NQA-1 used as the implementing standard). For fusion there is no code-qualification regime yet: NRC regulates fusion machines under a byproduct-material framework (Part 30 style) following the ADVANCE Act (2024) and SECY-23-0001 / rulemaking in progress (final status unverified, check https://www.nrc.gov/reactors/new-reactors/advanced/fusion). UK treats fusion under HSE/EA, not nuclear site licensing. So "NQA-1-aligned graded QA + documented V&V base" is the credible target, not a claim of "qualified".

### 1.4 Testing practice references
- SQLite: 100% branch coverage and MC/DC on the core library; test code roughly 590x the library source by size; billions of fuzz tests; aviation-grade (DO-178B) practice. [V gist; ratio U] https://www.sqlite.org/testing.html
- DO-178C structural coverage by software level: DAL A = MC/DC + decision + statement; DAL B = decision + statement; DAL C = statement; DAL D = none structural (requirements-based only). [V] RTCA DO-178C.
- OpenSSF Best Practices Gold badge requires >= 90 % statement coverage and >= 80 % branch coverage, reproducible build, 2+ unassociated significant contributors, signed releases. [V] https://www.bestpractices.dev/criteria
- Mutation testing: Google reports mutation-score driven code review at scale; no universal target; common practice thresholds 60-80 % of "covered" mutants killed; Rust tool cargo-mutants. [U] Petrovic et al., "Practical mutation testing at scale" (ICSE 2022 / IEEE TSE).
- OSS-Fuzz: > 1,000 projects, > 10,000 vulnerabilities and ~36,000 bugs found (2023 figure); 90-day disclosure deadline. [U numbers] https://google.github.io/oss-fuzz/
- Test size budgets (Google/Bazel convention): small <= 60 s, medium <= 300 s, large <= 900 s, enormous <= 3600 s. [V] https://bazel.build/reference/test-encyclopedia
- Perf regression tracking: continuous benchmarking (Criterion/divan + CodSpeed/Bencher); rustc-perf flags changes > ~1-2 % on instruction counts (instructions are low-noise, wall-clock is not). [U] https://perf.rust-lang.org/

### 1.5 Performance
- Response-time perception limits (Nielsen/Miller): 0.1 s feels instantaneous, 1 s keeps flow of thought, 10 s limit of attention. [V] https://www.nngroup.com/articles/response-times-3-important-limits/
- Frame budget: 60 Hz = 16.67 ms, 120 Hz = 8.33 ms, 144 Hz = 6.94 ms. Google RAIL: respond < 100 ms, animate frames in < 10 ms of work (16 ms budget), idle work chunks < 50 ms, load < 1 s. [V] https://web.dev/articles/rail
- Startup of best-in-class desktop apps: no reliable primary-source cross-app benchmark found. Zed markets itself as GPU-native with sub-second startup; VS Code cold start is typically 1-3 s on SSD; Blender to usable UI ~1-3 s; Figma desktop is web-based and loads in seconds. [U; treat as anecdotal; FARIS must measure its own baseline (Section 3 requires a published benchmark harness)]. https://zed.dev/ , https://code.visualstudio.com/docs/supporting/FAQ
- Memory: VS Code idle ~300-500 MB, Blender default scene ~200-400 MB, Zed ~100-300 MB [U]. FARIS target set by laptop constraint (see table).
- egui: default is reactive (repaint on input) when using request_repaint correctly; continuous repaint is the main cause of idle CPU burn. [V] https://docs.rs/egui/latest/egui/
- Energy: laptop idle app should be 0 wakeups attributable to FARIS when nothing animates; macOS Energy Impact / powertop on Linux / Windows Task Manager power usage trend as the measurement tools. [V tools]

### 1.6 Reliability
- Crash-free benchmarks: mobile industry targets crash-free users >= 99.9 % (Firebase Crashlytics guidance often cites 99.95 % as "excellent"); Google Play Android vitals "bad behaviour" threshold: user-perceived crash rate 1.09 % and ANR 0.47 % (overall). [U] https://developer.android.com/topic/performance/vitals ; https://docs.sentry.io/product/releases/health/
- Data-loss prevention: write temp file in same dir, fsync file, rename (atomic on POSIX; ReplaceFileW/MoveFileEx on Windows), fsync directory. SQLite WAL/journal provides atomic commit and rollback. Autosave default intervals: Blender 2 min, Word 10 min, Inkscape 10 min, VS Code auto-save off by default but hot-exit preserves unsaved buffers. [V Blender; U others]
- Crash-reporting: opt-in, local minidump + user-reviewed upload (Firefox/Mozilla Crash Reporter, Sentry/Crashpad/Breakpad).

### 1.7 Security and supply chain
- OpenSSF Scorecard: 0-10 per check (~18-20 checks: Branch-Protection, CI-Tests, Code-Review, Dangerous-Workflow, Dependency-Update-Tool, Fuzzing, License, Maintained, Packaging, Pinned-Dependencies, SAST, SBOM, Security-Policy, Signed-Releases, Token-Permissions, Vulnerabilities, Binary-Artifacts, Contributors, CII-Best-Practices); no official threshold is published [V]; FARIS picks its own. https://scorecard.dev/ 
- OpenSSF Best Practices badge: passing / silver / gold tiers. [V]
- SLSA v1.0 Build track: L1 provenance exists; L2 hosted build platform, signed provenance; L3 hardened, isolated builds, unforgeable provenance. (v1.1 exists; same levels.) [V] https://slsa.dev/spec/v1.0/levels
- SBOM: SPDX 2.3 (2022), SPDX 3.0 (2024), SPDX 2.2.1 = ISO/IEC 5962:2021; CycloneDX 1.6 = ECMA-424; NTIA minimum elements (2021): supplier, component name, version, other unique IDs, dependency relationship, author of SBOM data, timestamp. [V] https://spdx.dev/ https://cyclonedx.org/ 
- Rust tooling: cargo-audit (RustSec advisory DB), cargo-deny (advisories, licenses, bans, sources), cargo-vet (audit records), cargo-geiger/`unsafe` counting, Miri (UB detection), `#![forbid(unsafe_code)]` per crate, cargo-semver-checks, cargo-udeps. [V]
- Reproducible builds: `--locked`, vendored/hash-pinned deps, SOURCE_DATE_EPOCH, `--remap-path-prefix`, pinned toolchain (rust-toolchain.toml); verify by two independent builds hashing identically. https://reproducible-builds.org/
- Signing: Sigstore/cosign keyless (OIDC, Fulcio, Rekor transparency log) or minisign; Windows Authenticode (since June 2023 CA/B Forum requires private keys on hardware/HSM; CA/B ballot to cap code-signing cert validity at 460 days from 2026-03 [U]); Apple Developer ID + hardened runtime + notarytool notarization + stapling (USD 99/yr developer programme). [V gist]
- Vulnerability response SLAs in common use: Google Project Zero 90 days (+30 patch adoption); CISA KEV federal remediation 14-21 days; PCI DSS critical 30 days; many OSS projects state "critical fix within 7 days". [V]
- EU Cyber Resilience Act, Regulation (EU) 2024/2847: in force 10 Dec 2024; notified-body provisions 11 Jun 2026; reporting obligations (actively exploited vulnerabilities, severe incidents) apply from 11 Sep 2026 (already in effect as of today); all other obligations 11 Dec 2027. Reporting clocks: early warning within 24 h, notification within 72 h, final report within 14 days of a corrective measure (vulnerability) or 1 month (incident). Support period >= 5 years unless expected use time is shorter; SBOM required in technical documentation; vulnerability disclosure policy and security updates free of charge. Pure non-commercial FOSS outside "commercial activity" is excluded; "open-source software stewards" have a light regime; paid support/hosted accounts may bring a product into scope. [V] https://eur-lex.europa.eu/eli/reg/2024/2847/oj
- Export control: US EAR (15 CFR 730-774) with nuclear items under ECCN 0xxx/1xxx and 10 CFR Part 110 (NRC; exports of nuclear equipment/material incl. tritium) and 10 CFR Part 810 (DOE; foreign assistance to atomic energy activities, technology transfer; revised 2015, generally authorised countries list). Whether fusion plant design/neutronics software is controlled depends on content (tritium handling, enrichment-related). MCNP and some SCALE/ORIGEN distributions are screened through RSICC/NEA Data Bank; ENDF/B-VIII.0 (BNL NNDC) and FENDL-3.2 (IAEA-NDS) are publicly downloadable; license terms differ per library. [U on scope: counsel review needed] https://www.ecfr.gov/current/title-10/chapter-III/part-810 
- Sandbox of untrusted files: parse in memory-safe Rust, size/depth/ratio caps (zip bombs), no path traversal, no auto-exec of embedded scripts/subprocesses without explicit trust, fuzz parsers.

### 1.8 Platforms and distribution
- wgpu backends: Vulkan (Linux/Windows/Android), Metal (macOS/iOS), DX12 (Windows), OpenGL/GLES fallback, WebGPU. https://wgpu.rs/ . Software fallbacks: Mesa llvmpipe/lavapipe (Linux), WARP (Windows).
- Rust platform tiers: Windows 10+ min for current rustc target; macOS min raised periodically (check https://doc.rust-lang.org/rustc/platform-support.html).
- Linux packaging: Flatpak (sandbox, Flathub), AppImage, .deb/.rpm, plain tarball. Windows: MSI (WiX) or MSIX; macOS: signed, notarized DMG (universal2 or per-arch). 
- glibc baseline: manylinux_2_28 (glibc 2.28, RHEL 8 era) is the usual conservative floor. [V] https://github.com/pypa/manylinux
- Auto-update: Sparkle (macOS; EdDSA signatures), Squirrel/MSIX, The Update Framework (TUF) for rollback/freeze/mix-and-match defence. [V] https://theupdateframework.io/
- Support windows: Ubuntu LTS 5 years standard (10 with Pro); RHEL 10 years; Firefox ESR ~ 1 year overlapping; CRA minimum 5 years of security updates. [V]
- Semantic Versioning 2.0.0 https://semver.org/ ; Blender/Maya style project-file back-compat: Blender reads old files (with forward-compat warnings) back across major versions; FARIS should specify an explicit N-back guarantee.

### 1.9 Observability, privacy, licensing
- GDPR: lawful basis + consent (Art. 6-7), privacy by design/default (Art. 25), breach notification to authority within 72 h (Art. 33). DO_NOT_TRACK convention (consoledonottrack.com). Telemetry opt-in only is the defensible default. [V]
- AGPL-3.0 s.13: users interacting over a network must be offered Corresponding Source; compatible with GPLv3-or-later, Apache-2.0, MIT, BSD; NOT with GPL-2.0-only. Rust-crate licence audit via cargo-deny/cargo-about. [V] https://www.gnu.org/licenses/agpl-3.0.html
- Nuclear data licences: ENDF/B (CC0-like US public domain), FENDL (IAEA terms permitting use with citation), JEFF (NEA, public with attribution), TENDL (CC-BY-like) [U: verify each; record per-library licence in a data-licence manifest].

---------------------------------------------------------------------------
## 2. Candidate requirements

Columns: ID | Area | Requirement | Metric | Best-in-class reference | Proposed FARIS target | How to verify
Tags in Area give the ISO 25010:2023 characteristic (FS functional suitability, PE performance, CO compatibility, IC interaction capability, RE reliability, SE security, MA maintainability, FL flexibility, SA safety).

### 2.1 Quality model and measurement (QM)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| QM-01 | Quality model | FARIS shall maintain a quality-requirements register mapping every requirement to one of the nine ISO/IEC 25010:2023 characteristics. | % of requirements tagged | ISO 25010:2023 | 100 % | CI check on requirements file schema |
| QM-02 | Quality model | FARIS shall define for each characteristic at least one 25023-style measure with formula, data source and threshold. | measures per characteristic | ISO 25023:2016 | >= 2 per characteristic, 9 characteristics | Review of measurement plan; script emits value per release |
| QM-03 | Quality model | FARIS shall publish a per-release quality scorecard of all measures. | scorecard present, all fields populated | PCMM (Sandia) | 1 per release, 0 empty fields | Release checklist gate |
| QM-04 | Safety (SA) | FARIS shall state its safety-relevance classification (non-safety, research-grade; not for safety-system design) and show it in the UI/export footer. | classification string in 100 % of exports | NQA-1 2.7 classification | 100 % of exports | Export golden test |
| QM-05 | Maturity | FARIS shall score itself on the six PCMM elements per release. | PCMM level 0-3 per element | SAND2007-5948 | >= level 1 all elements at v1.0, >= 2 for code verification and solution verification | Self-assessment report reviewed by 2nd person |

### 2.2 Nuclear/scientific software QA and V&V (VV)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| VV-01 | QA programme | FARIS shall have a written SQA plan mapped clause-by-clause to NQA-1 Part II Subpart 2.7 (graded approach). | clauses mapped | NQA-1 2.7 | 100 % of 2.7 requirements have a status (met/justified N/A) | Traceability matrix in repo; reviewer sign-off |
| VV-02 | Requirements traceability | FARIS shall trace each requirement to design element, test and result. | % requirements with >= 1 passing test | NUREG/BR-0167; RG 1.172 | 100 % of "shall" requirements | Generated trace report, CI fail if gap |
| VV-03 | Classification | FARIS shall classify each calculation module by consequence of error (e.g. A authored/B screening/C decision-supporting) and apply proportionate V&V. | modules classified | NQA-1 graded approach; DOE G 414.1-4 | 100 % of modules | Module manifest check |
| VV-04 | PIRT | FARIS shall hold a PIRT for each major model (neutronics, activation, tritium cycle, thermal, magnets) listing phenomena by importance and knowledge level. | PIRT exists, phenomena ranked H/M/L | Wilson & Boyack 1998 | 1 per physics domain before that domain is labelled "calculated" | Document gate |
| VV-05 | Code verification | FARIS shall verify each Rust numerical kernel by method of manufactured solutions or analytic benchmark with observed order of accuracy. | observed order vs formal order | Roache 2002 | within +/-0.1 of formal order on >= 3 refinement levels | Convergence test in CI |
| VV-06 | Solution verification | FARIS shall estimate discretisation error (mesh/step) for every field output and flag results with GCI above limit. | GCI (Roache grid convergence index) | ASME V&V 20 | GCI < 5 % else flagged "not converged" | Per-run check + test |
| VV-07 | Validation | FARIS shall report validation comparison E with validation uncertainty u_val for each validation case. | E vs u_val | ASME V&V 20 | publish E, u_val; pass if abs(E) <= 2 u_val or state failure | Validation report auto-generated |
| VV-08 | Validation base | FARIS shall maintain a versioned validation suite of integral benchmarks (e.g. SINBAD/FNG, ICSBEP criticality for adapter sanity, TBR benchmarks). | benchmarks in suite, C/E pass rate | SINBAD 101 exps (31 fusion); ICSBEP 2024 5,168 configs [V] | >= 10 fusion-neutronics benchmarks by v1.0, >= 25 by v2.0 (provisional: effort-bound) | Suite runs in nightly CI, results archived |
| VV-09 | Benchmark C/E | FARIS shall meet published C/E windows for each validation benchmark. | C/E | FNG-ITER literature +/-5-20 % reaction rates, +/-10-30 % SDDR | Equal to or better than OpenMC reference for same inputs, within MC 2 sigma + nuclear data uncertainty | Validation report |
| VV-10 | Adapter consistency | FARIS shall reproduce a native OpenMC run result for the same model (via adapter) with no unexplained difference. | relative difference | n/a | within 2 sigma of combined MC SE (same seed -> bitwise or documented) | Adapter golden test |
| VV-11 | Code-to-code | FARIS shall compare each transport/activation result to a second independent code where one exists (e.g. MCNP/Serpent, FISPACT-II) and log differences. | relative difference | CSAU practice | log 100 % of comparisons; flag > 5 % or > 2 sigma | Comparison scripts in suite |
| VV-12 | Uncertainty | FARIS shall propagate statistical, nuclear-data (via library covariances/sampling), and input uncertainty to headline outputs, or state "not evaluated". | uncertainty components present | CSAU; BEPU 95/95 | every headline number carries >= statistical + label of what is excluded | UI/export test |
| VV-13 | Applicability | FARIS shall record applicability domain per validated model (parameter ranges) and warn when outside. | outside-domain warning | CSAU step 6 | 100 % of validated models have domain metadata; warning emitted in tests | Test with out-of-range input |
| VV-14 | Errors | FARIS shall maintain an error/defect log with severity and disposition, public per release. | open S1/S2 defects at release | NQA-1 2.7 error reporting | 0 open S1, 0 open S2 without documented waiver | Release gate |
| VV-15 | Independent review | FARIS shall have independent (non-author) review of every change to a classified-A/B module. | % changes reviewed | NQA-1 | 100 % | Branch protection + audit |
| VV-16 | Receipts | Evidence receipts shall be hash-bound to inputs, code version, data library versions and tool versions. | fields hashed | Avila Core | 100 % of runs emit receipt; tamper test detects change | Test that mutates 1 byte -> verification fails |
| VV-17 | Data provenance | FARIS shall record nuclear-data library name, version, and checksum for every calculation. | field present | NQA-1 | 100 % | Project file schema test |
| VV-18 | Licensing claim | FARIS shall not claim "qualified for licensing use"; docs shall state precise V&V status. | claim lint | NRC EMDAP | 0 occurrences of unqualified claim | Doc-lint CI |
| VV-19 | Qualification pathway | FARIS shall document what an adopter needs to qualify it (dedication/commercial-grade-dedication evidence package). | document exists | NQA-1 2.7 dedication | published at v1.0; evidence package generated by one command | Doc + CLI test |
| VV-20 | Reproducibility | FARIS shall reproduce any prior result from its project file bit-for-bit for deterministic parts, statistically for MC. | deterministic diff = 0; MC within 2 sigma | OpenMC fixed-seed regression | 0 byte diff on deterministic outputs on same platform | Replay test in CI |
| VV-21 | Cross-platform determinism | FARIS shall give deterministic outputs equal within tolerance across Linux/Windows/macOS. | max rel diff | n/a | <= 1e-12 relative for pure-Rust analytics; <= 1e-9 for ODE stiff solvers (provisional: FMA/libm differences) | Cross-OS CI diff job |

### 2.3 Testing practice (TP)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| TP-01 | Coverage | Safety/physics-kernel crates shall have line+branch coverage at gold-badge levels. | statement/branch % | OpenSSF gold: 90 % / 80 % | kernels >= 90 % statement, >= 85 % branch; UI crate >= 60 % (provisional: GUI hard to cover) | cargo-llvm-cov in CI |
| TP-02 | Coverage high-assurance | Core calculation kernels shall reach MC/DC-style condition coverage on safety-relevant decisions. | MC/DC % | DO-178C level A; SQLite | 100 % on designated "decision" functions (list <= 5 % of code) (provisional) | Instrumented coverage (e.g. LLVM MC/DC in Rust nightly) |
| TP-03 | Mutation | Kernel crates shall hold a mutation score threshold. | % mutants killed (viable) | Google practice; cargo-mutants | >= 80 % kernels, >= 60 % others (provisional) | cargo-mutants nightly, trend report |
| TP-04 | Property tests | Invariants (conservation of mass/energy/tritium, monotonicity, unit consistency) shall be tested with property-based tests. | properties per kernel | proptest/QuickCheck | >= 5 properties per kernel crate; >= 1,000 cases per property per run, 100k nightly | proptest in CI |
| TP-05 | Conservation | Tritium and energy balance shall close in every run. | residual | n/a | abs residual <= 1e-9 relative of total flow (deterministic parts) | Property test + runtime assert |
| TP-06 | Fuzzing | Every parser (.faris, geometry import, nuclear data, OpenMC outputs, CSV, config) shall have a fuzz target. | targets / parsers | OSS-Fuzz | 100 % parsers; >= 1 CPU-hour per target per release, continuous OSS-Fuzz enrolment when public | cargo-fuzz corpus in CI |
| TP-07 | Fuzz outcome | Fuzzers shall find no crash/hang/OOM on release candidates. | open crashes | OSS-Fuzz 90-day policy | 0 | CI artifact |
| TP-08 | Golden files | Exports (PDF/CSV/charts/project) shall have golden-file tests with explicit update review. | golden diffs | insta/snapshot | 100 % export types covered; golden update requires reviewer approval | CI snapshot job |
| TP-09 | Tolerance policy | Numeric comparisons shall use a documented policy: abs+rel tolerance per quantity, ULP for kernels, z-score for MC. | policy doc, tests using it | n/a | policy file; no test uses bare `==` on floats (lint) | clippy::float_cmp deny |
| TP-10 | Stochastic tests | MC-dependent tests shall use fixed seeds and statistical acceptance (|z| < 3, repeated n seeds) rather than exact values. | flake rate | n/a | 0 flaky tests in 1,000 consecutive CI runs (provisional) | Flake tracker |
| TP-11 | Memory-safety tests | Test suite shall run under Miri (for unsafe), sanitizers (ASan/UBSan/TSan where applicable) nightly. | clean runs | Rust project practice | 0 findings | Nightly job |
| TP-12 | Concurrency | Concurrent code shall be exercised with loom or equivalent. | modules covered | tokio/loom | 100 % of custom sync primitives | Loom tests |
| TP-13 | GUI testing | UI shall have automated screenshot/snapshot tests per view and keyboard-only flow tests. | views covered | Avila screenshot loop | 100 % of top-level views at 3 DPI scales | CI screenshot diff |
| TP-14 | End-to-end | A scripted end-to-end scenario (new study -> run -> compare -> export) shall pass on all tier-1 platforms. | pass/fail | n/a | 100 % pass per release | CI matrix |
| TP-15 | Perf regression | CI shall track benchmark results and fail on regressions. | regression % | rustc-perf (instruction counts) | fail > 5 % wall time or > 2 % instruction count regression (provisional: noise) | criterion/divan + CodSpeed/Bencher |
| TP-16 | CI time | Presubmit CI shall be fast. | wall minutes | Google test size limits | presubmit <= 15 min; full matrix <= 60 min; nightly full validation <= 8 h (provisional) | CI timing dashboard |
| TP-17 | Test sizes | Each test shall be labelled small/medium/large with enforced timeouts. | timeout | Bazel convention 60/300/900/3600 s | 100 % tests labelled; unit suite <= 5 min | Harness |
| TP-18 | Docs tests | All documentation code samples shall be executed in CI. | % samples executed | rustdoc doctests | 100 % | CI |
| TP-19 | Static analysis | Zero clippy warnings at `-D warnings`; rustfmt clean; `cargo doc` warning-free. | warnings | Rust practice | 0 | CI |
| TP-20 | Panic policy | Production code shall not contain `unwrap`/`expect`/indexing panics on user data paths. | lint count | n/a | 0 (clippy unwrap_used, indexing_slicing deny on data paths) | Lint |
| TP-21 | Unit consistency | Dimensional analysis shall be type-enforced or tested. | quantities with units | Mars Climate Orbiter lesson | 100 % of public API quantities carry units | API review + tests |
| TP-22 | Repro test data | Test fixtures shall be small. | repo test data size | n/a | <= 50 MB in git; large data content-addressed and fetched | CI size check |

### 2.4 Performance engineering (PF)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| PF-01 | Cold start | FARIS shall show a usable window on cold start. | ms to first interactive frame | Zed/Blender ~1-2 s [U] | <= 2.0 s cold (disk cache flushed), P95 over 20 runs on reference laptop (provisional until baselined) | Startup bench in CI + reference HW |
| PF-02 | Warm start | Warm start. | ms | VS Code ~1 s [U] | <= 0.8 s | Same |
| PF-03 | Open project | Opening a typical project (50 MB .faris) shall be fast. | ms to interactive | n/a | <= 1.5 s; progressive load shows UI within 0.5 s | Bench |
| PF-04 | Input latency | UI input-to-visual response. | ms | RAIL 100 ms; 60 Hz frame | <= 50 ms P95; no main-thread block > 16 ms during interaction | Frame profiler |
| PF-05 | Frame time | 3D viewport frame time on reference integrated GPU at default scene. | P50/P99 frame ms | 60 fps = 16.7 ms | P50 <= 8 ms, P99 <= 16.7 ms, 0 frames > 50 ms during orbit/pan (provisional) | Automated camera path bench, frame-time histogram |
| PF-06 | Frame time heavy | Viewport on a 1M-cell mesh overlay. | P99 ms | n/a | P99 <= 33 ms (30 fps floor), with LOD/decimation | Bench |
| PF-07 | Idle power | When nothing animates FARIS shall not repaint. | idle CPU %, wakeups/s | egui reactive mode | <= 0.5 % CPU, <= 5 wakeups/s idle | powertop/Task Manager/Instruments scripted |
| PF-08 | Battery | Running the 30-year recalculation and orbiting shall not exceed power budget on reference laptop. | W above idle | n/a | measure and publish; no regression > 10 % release over release (provisional) | Power measurement |
| PF-09 | Slider recalculation | Operating-history recalculation from slider drag. | ms | current ~1 s | <= 100 ms for 30-yr history (feel instantaneous, RAIL), <= 1 s for full multi-system (provisional) | Bench |
| PF-10 | Memory baseline | Idle empty-state RSS. | MB | VS Code 300-500 MB [U] | <= 250 MB | Scripted RSS measurement |
| PF-11 | Memory large | Peak RSS on a large study (e.g. 10M-cell mesh tallies). | GB | 30 GB shared laptop constraint | Declared limit: <= 4 GB for default-size study; hard budget parameter; fail-soft message at 80 % of configured cap | Stress test inside cgroup |
| PF-12 | Large-model limits | FARIS shall publish documented size limits and behaviour at limit (CAD triangles, mesh cells, tally bins, history length). | numeric limits documented | Blender/CAD apps | table shipped; tests at 100 % and 110 % of limit show graceful refusal | Limit tests |
| PF-13 | Scalability | Parallel kernels shall scale. | parallel efficiency | n/a | >= 70 % efficiency to 8 cores for sweeps | Scaling bench |
| PF-14 | Background jobs | Long jobs shall run off the UI thread with progress, cancel (<= 1 s response), pause, priority and resource caps. | cancel latency | n/a | UI thread never blocked > 16 ms by jobs; cancel <= 1 s; default max jobs = cores/2 | Test harness |
| PF-15 | Job scheduling | A queue shall serialise heavy external runs (OpenMC) with memory-aware admission. | no OOM | Laptop OOM history | 0 OOM kills in soak; admission refuses job if projected RSS > free memory - margin | Soak test |
| PF-16 | Throttling | Background compute shall yield to interactive use (nice/priority) and honour battery-saver. | behaviour | n/a | Default low priority; auto-pause option on battery | Test |
| PF-17 | Binary size | Installed footprint. | MB | Zed ~100 MB class [U] | <= 150 MB app without external data; nuclear data downloaded separately | Package check |
| PF-18 | Compile/dev speed | Developer loop. | s | n/a | incremental debug build <= 30 s on laptop; `cargo test` unit <= 5 min | CI timing |
| PF-19 | Export speed | PDF report export. | s | n/a | <= 5 s for 50-page report | Bench |
| PF-20 | Profiling | FARIS shall expose a built-in perf overlay (frame time, memory, job queue). | present | Blender/Unity stats | present and scriptable; documented | UI test |

### 2.5 Reliability (RL)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| RL-01 | Crash-free | FARIS shall achieve a crash-free session rate. | % sessions without crash | Crashlytics 99.9-99.95 % | >= 99.9 % sessions, >= 99.5 % users per release (measured from opt-in reports + soak) | Soak test + opt-in telemetry |
| RL-02 | Soak | 72 h automated soak (open/edit/run/close loops) shall produce zero crashes or leaks. | crashes, RSS growth | n/a | 0 crashes; RSS growth <= 5 % after warm-up | Nightly soak job |
| RL-03 | Atomic save | Project saves shall be atomic. | torn files after kill | SQLite/atomic rename | 0 corrupted files in 10,000 randomised kill-during-save trials | Fault-injection test (kill -9, power-loss emulation) |
| RL-04 | Autosave | FARIS shall autosave recoverable state. | max data loss | Blender 2 min | interval <= 60 s of idle-after-edit; max loss <= 1 edit/60 s | Test + recovery test |
| RL-05 | Recovery | After a crash, FARIS shall offer recovery on next start. | recovery success | n/a | >= 99 % of injected crashes recoverable with <= 60 s of work lost; recovery UI <= 5 s | Fault injection |
| RL-06 | Backups | Saving shall keep last N versions. | N | n/a | >= 3 rolling backups; configurable | Test |
| RL-07 | Integrity | Project files shall carry checksums and verify on open. | detection | content-addressed zip | 100 % of single-byte corruptions detected | Mutation test of file bytes |
| RL-08 | Partial recovery | A corrupted .faris shall open in salvage mode recovering undamaged objects. | recovered fraction | n/a | recover all non-damaged blobs; report damaged ones by name | Corruption tests |
| RL-09 | Graceful degradation | If GPU/adapter/external tool is missing FARIS shall still open and show results from cache. | behaviour | n/a | No panic; named degraded-mode banner; 100 % of adapters tested "missing" | Negative tests |
| RL-10 | Panic handling | A panic in a worker/kernel shall not take down the app. | isolation | n/a | worker panics caught, job marked failed, UI alive, in 100 % of injected panics | Fault injection |
| RL-11 | External process | External tool crash/hang shall be contained. | detection time | n/a | hang watchdog <= configurable (default 60 s no progress); kill and report within 2 s | Test |
| RL-12 | Disk full | Behaviour on disk full / read-only FS. | no data loss | n/a | save fails with clear message; previous file intact | Fault injection |
| RL-13 | Undo | Undo/redo depth. | steps | CAD apps | >= 100 steps, survives autosave recovery | Test |
| RL-14 | Recovery time | Start-to-recovered time after crash. | s | n/a | <= 10 s | Bench |
| RL-15 | MTBF | Mean time between failures during work sessions. | hours | 25023 MTBF | >= 200 h of use per crash (soak-derived) | Soak stats |
| RL-16 | Crash reports | Crash reporting shall be opt-in, local-first, human-reviewable before upload. | pass/fail | Mozilla crash reporter; GDPR | default off; dump stored locally; user sees content; upload only on click | UI test + network trace shows 0 requests when off |
| RL-17 | Deterministic failure | Every error shall have a stable code, plain explanation and next step. | % errors with code | house rule (Unknown must explain) | 100 % user-facing errors | Error catalogue test |
| RL-18 | Long runs | Long external runs shall checkpoint and resume. | resume | OpenMC statepoints | resume from last statepoint with <= 1 batch lost | Kill/resume test |

### 2.6 Security and supply chain (SC)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| SC-01 | Scorecard | FARIS public repos shall maintain an OpenSSF Scorecard. | aggregate score | no official threshold (FARIS choice) | >= 7.5 at v1.0, >= 8.5 later (provisional) | scorecard action weekly |
| SC-02 | Badge | FARIS shall hold the OpenSSF Best Practices badge. | tier | passing/silver/gold | passing at first public release, gold by v2.0 | bestpractices.dev |
| SC-03 | SLSA | Release builds shall meet SLSA Build L3 (hosted builder, isolated, signed provenance). | level | SLSA v1.0 | L3 for tagged releases; L2 min | slsa-verifier on artifacts |
| SC-04 | SBOM | Every release shall ship SBOM(s) in SPDX 2.3 (or 3.0) and CycloneDX 1.6, covering all crates and bundled binaries. | completeness | NTIA min elements; CRA | 100 % of linked components; NTIA 7 elements present; validated by official validators | SBOM validation in CI |
| SC-05 | Provenance sign | Release artifacts shall be signed (Sigstore cosign + platform signing). | signed | Sigstore | 100 % artifacts; verification command documented | Verify script |
| SC-06 | Windows signing | Windows installer and binaries shall be Authenticode-signed with timestamp. | pass | CA/B hardware key rules | 100 %; no SmartScreen hard block for new release after reputation (provisional) | signtool verify |
| SC-07 | macOS signing | macOS app shall be Developer ID signed, hardened runtime, notarized, stapled. | pass | Apple | 100 % | spctl -a -vvv; stapler validate |
| SC-08 | Reproducible | Release binaries shall be bit-for-bit reproducible from source on a clean builder. | hash match | reproducible-builds.org | 100 % on Linux tarball; best effort on Windows/macOS (documented) | Independent rebuild job |
| SC-09 | Dep audit | CI shall fail on known vulnerabilities, banned licences, unknown registries. | findings | cargo-audit/deny | 0 unwaived advisories; waivers expire <= 90 days | cargo-deny in CI daily |
| SC-10 | Dep review | New dependencies shall be vetted (cargo-vet or equivalent) and minimal. | % audited | Mozilla/Google cargo-vet | 100 % of direct deps audited or imported-audit; dep count tracked, budget <= 400 transitive crates for core app (provisional) | cargo-vet check |
| SC-11 | Pinning | Dependencies and CI actions shall be pinned by hash. | pinned | Scorecard Pinned-Dependencies | 100 % (Cargo.lock committed; actions by SHA) | Scorecard |
| SC-12 | CI hardening | CI tokens least privilege; no pull_request_target on untrusted code. | pass | Scorecard Dangerous-Workflow/Token-Permissions | Score 10 on both | Scorecard |
| SC-13 | Unsafe policy | `unsafe` shall be forbidden in all crates except enumerated, documented ones; each block has SAFETY comment and Miri test where possible. | unsafe count | Rust practice | `#![forbid(unsafe_code)]` in >= 90 % crates; unsafe LOC <= 1 % of own code (excluding deps) | cargo-geiger, clippy undocumented_unsafe_blocks |
| SC-14 | Untrusted files | Opening any study/project file shall not execute code, shell out, or touch paths outside the project sandbox. | pass | zip-slip/zip-bomb defences | path traversal 0; decompression ratio cap <= 100:1 and absolute cap configurable (default 2 GB); entry count cap; nesting cap | Negative test corpus + fuzz |
| SC-15 | Adapters | External tool invocation shall use argv arrays (no shell), allow-listed binaries, working dir confinement, resource limits. | pass | OWASP | 0 shell invocation of user-controlled strings | Code lint + tests |
| SC-16 | Plugin trust | Any plugin/extension mechanism shall be sandboxed (WASM) or require explicit signed trust. | pass | VS Code workspace trust | Default untrusted; capability-based permissions | Security tests |
| SC-17 | Secrets | No secrets in repo/logs/diagnostic bundles. | findings | gitleaks | 0 | CI secret scan; bundle scanner |
| SC-18 | Network | FARIS shall make no network connection unless the user initiates it. | connections at idle | privacy | 0 outbound at idle, verified by packet capture; update check off or explicit | Network trace test |
| SC-19 | Auto-update | If auto-update exists it shall be opt-in, signature-verified, TUF-style rollback-protected, and downgrade-resistant. | pass | TUF/Sparkle | tampered update rejected in 100 % of tests | Update test with bad signature/old version |
| SC-20 | Vuln response | FARIS shall publish SECURITY.md with reporting channel and SLAs. | SLAs | Project Zero 90 d; CISA 14-21 d | acknowledge <= 3 business days; critical fix <= 7 days, high <= 30 d, medium <= 90 d | Process audit; tracked in advisory log |
| SC-21 | CRA reporting | FARIS shall be able to notify actively exploited vulnerabilities per CRA timelines if in scope. | clocks | CRA Art. 14 | early warning <= 24 h, notification <= 72 h, final report <= 14 d after fix | Runbook + annual drill |
| SC-22 | CRA support | FARIS shall provide security updates for >= 5 years per major release. | years | CRA | >= 5 years | Policy |
| SC-23 | Security advisories | Security fixes shall be published as GitHub/RustSec advisories with CVE where applicable. | % | n/a | 100 % of fixed vulns | Audit |
| SC-24 | Threat model | A documented threat model (STRIDE) covering files, adapters, update, data. | exists | n/a | reviewed each major release | Doc gate |
| SC-25 | Pen test | Independent security review before 1.0. | report | n/a | 1 external review, 0 open high | Report (provisional: cost) |
| SC-26 | Memory safety | Code in C/C++ (via adapters/bindings) shall be isolated in separate processes. | process isolation | Chrome/Rust practice | 100 % of non-Rust native code out-of-process or fuzzed | Architecture review |
| SC-27 | Telemetry integrity | Evidence receipts and project files shall be tamper-evident. | detection | hash-bound | 100 % single-bit changes detected | Mutation test |
| SC-28 | Release integrity | Updates and installers shall be served over TLS with hash lists. | pass | n/a | TLS 1.2+ only; checksums published | Scan |
| SC-29 | Export control | FARIS shall have a documented export-control review for code, bundled data, and generated files (EAR, 10 CFR 110/810) and flag controlled-data warnings. | review done | counsel | review at each major release; no controlled data bundled | Legal checklist (provisional: counsel input) |

### 2.7 Platforms and distribution (PD)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| PD-01 | Tiers | FARIS shall publish OS support tiers with CI evidence. | tier table | Rust platform-support | Tier 1: Linux x86_64 (glibc >= 2.28), Windows 10/11 x86_64, macOS 13+ arm64; Tier 2: Linux aarch64, macOS x86_64; Tier 3: others best-effort | Tier CI matrix |
| PD-02 | GPU backends | FARIS shall run on Vulkan 1.1, DX12 (FL 11_0), Metal. | pass on each | wgpu | pass on all 3 on reference GPUs from Intel UHD 620 class upward | Hardware CI |
| PD-03 | GPU fallback | FARIS shall run (slower, labelled) with software rasteriser when no GPU. | pass | lavapipe/WARP | opens and renders default scene at >= 5 fps | VM test |
| PD-04 | Min hardware | FARIS shall state min and recommended hardware. | table | n/a | min: 4 cores, 8 GB RAM, 4 GB disk; recommended: 8 cores, 16 GB (provisional) | Test on min VM |
| PD-05 | Linux packaging | Linux packages: Flatpak, AppImage, tarball (+ .deb). | present | Flathub | >= 2 formats per release | Install test |
| PD-06 | Windows packaging | Signed MSI/MSIX plus portable zip. | present | n/a | silent install `/qn` works; uninstall leaves 0 files except user data | Install test |
| PD-07 | macOS packaging | Notarized DMG, universal or per-arch. | present | n/a | gatekeeper passes offline after staple | Test |
| PD-08 | Offline | FARIS shall be fully functional offline (everything except optional downloads). | pass | air-gap use | 100 % of features pass with network disabled; data and docs bundled/offline-installable | Test in netns without network |
| PD-09 | Air-gapped install | FARIS shall install from a single offline bundle with checksum and signature verification. | steps | n/a | one archive; instructions; verified install on air-gapped VM | Test |
| PD-10 | Data packs | Nuclear data/assets shall be installable as separate versioned, hash-verified packs. | pass | n/a | install/verify offline; pack licence manifest | Test |
| PD-11 | Release cadence | FARIS shall follow a published cadence. | schedule | Blender ~ 4 releases/yr, Firefox 4 weeks | minor every 3 months (provisional), patch as needed | Release log |
| PD-12 | LTS | FARIS shall designate LTS releases. | years | Ubuntu 5 y; CRA 5 y | LTS every 12-24 months, supported >= 3 years with security fixes (>= 5 y if CRA applies) | Policy |
| PD-13 | SemVer | Public APIs (Rust crates, CLI, file schema) follow SemVer; breaking changes only in majors. | pass | semver.org; cargo-semver-checks | 0 breaking changes in minor; CI check | cargo-semver-checks |
| PD-14 | File back-compat read | FARIS shall open project files from any version within the previous 2 major versions (N-2) and all 1.x. | open success | Blender | 100 % of archived fixture files from every released version open (migrated on load) | Archive of fixtures per release |
| PD-15 | File forward-compat | FARIS shall open newer-minor files read-only with a warning listing unknown fields, preserving them on re-save. | pass | protobuf unknown-field rule | unknown fields preserved in 100 % of tests | Test |
| PD-16 | Migration | Migration shall be lossless, explained, and non-destructive (original kept). | pass | n/a | 100 % of fixtures round-trip with 0 numeric diff; original unchanged | Test |
| PD-17 | CLI parity | Every GUI operation shall have a CLI/API equivalent with stable JSON output. | % ops | house goal | >= 95 % at v1.0, 100 % later; JSON schema versioned | Parity matrix test |
| PD-18 | Uninstall | Clean uninstall and user-data preservation. | residual | n/a | 0 residual files outside user data | Test |
| PD-19 | Portable mode | Portable mode (config beside binary). | pass | n/a | works from read-only USB | Test |
| PD-20 | Installer size/time | Install time. | s | n/a | <= 60 s on SSD | Test |
| PD-21 | Accessibility of install | Installers operable by keyboard/screen reader. | pass | n/a | pass | Manual checklist |
| PD-22 | HiDPI/Locale | Correct operation at 100-300 % scaling, non-ASCII paths, non-English locales, decimal comma. | pass | n/a | tests with Unicode+space paths; locale `de_DE` and `ja_JP` pass | CI matrix |
| PD-23 | Docker/headless | Headless/CLI builds for HPC/CI (no GPU). | pass | n/a | container image <= 1 GB, runs without display | CI |
| PD-24 | Update channel | stable/beta/nightly channels with rollback. | pass | Firefox | rollback to previous release in <= 2 min, project files unaffected | Test |

### 2.8 Observability, privacy and diagnostics (OB)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| OB-01 | Telemetry default | FARIS shall send no telemetry by default. | outbound requests | GDPR Art. 25 | 0 by default; opt-in per category with plain-language preview | Network trace |
| OB-02 | Consent | Opt-in shall be revocable with one action and take effect immediately. | steps | GDPR Art. 7(3) | <= 2 clicks; stops within 1 s | UI test |
| OB-03 | Env override | Honour DO_NOT_TRACK and a FARIS_OFFLINE variable. | pass | consoledonottrack.com | respected 100 % | Test |
| OB-04 | Data minimisation | Telemetry (if ever enabled) shall contain no study content, paths, user names or IPs stored. | fields | GDPR Art. 5 | allow-list of fields documented; anything else dropped | Schema test + review |
| OB-05 | Local logs | Structured local logs with levels, rotation. | size | n/a | JSON-lines; rotate at 10 MB x 5; default level info; no study content at info | Test |
| OB-06 | Diagnostics bundle | One-click diagnostics bundle that is user-reviewable and redacts paths/secrets. | pass | VS Code "Report issue", Mozilla | zip <= 20 MB; manifest of contents; redaction scan 0 secrets; produced in <= 10 s | Test |
| OB-07 | Run provenance | Each calculation logs inputs hash, versions, duration, resource use. | fields | receipts | 100 % of runs | Test |
| OB-08 | Privacy notice | In-app privacy statement and data inventory. | present | GDPR Art. 13 | present; reviewed each release | Doc gate |
| OB-09 | Retention | Local logs/crash dumps auto-purge. | days | n/a | <= 30 days default; "clear now" button | Test |
| OB-10 | Performance traces | Optional trace export in Chrome trace / Perfetto format. | pass | n/a | opens in Perfetto | Test |

### 2.9 Licensing and compliance (LC)
| ID | Area | Requirement | Metric | Reference | Target | Verify |
|---|---|---|---|---|---|---|
| LC-01 | AGPL source | FARIS shall offer Corresponding Source for every distributed or network-served build (AGPL s.13). | pass | AGPL-3.0 | in-app "Source" link to exact commit/tag; any hosted FARIS service exposes same | Release checklist + hosted test |
| LC-02 | Licence audit | All dependencies shall be AGPL-compatible. | violations | cargo-deny licenses | 0 | CI |
| LC-03 | Notices | Third-party notices (licences + copyrights) shall ship in every package and in-app. | % deps listed | MIT/BSD/Apache notice clauses | 100 % | cargo-about diff vs lockfile |
| LC-04 | REUSE | Every file shall carry SPDX licence identifier. | % files | REUSE spec | 100 % (reuse lint) | CI |
| LC-05 | Data licences | A manifest shall record licence, source, version, and redistribution status for every nuclear data library and bundled dataset. | fields | ENDF/FENDL/JEFF/TENDL terms | 100 % entries; no data bundled without redistribution right | Manifest check |
| LC-06 | External tools | Adapter docs shall state licence and export/availability status of each external tool (OpenMC MIT-like, MCNP via RSICC, etc.) and shall not bundle restricted tools. | pass | RSICC | 0 bundled restricted binaries | Package scan |
| LC-07 | Contribution | DCO or CLA policy documented. | pass | n/a | DCO sign-off enforced by bot | CI |
| LC-08 | Generated output | Licence/terms for exported reports and figures stated. | pass | n/a | statement in export footer/metadata | Test |
| LC-09 | Trademark/claims | Names/claims lint. | pass | n/a | 0 forbidden claims | Doc lint |
| LC-10 | Citation | CITATION.cff and per-run citation list of data/code used. | pass | n/a | present; reports auto-list libraries | Test |

---------------------------------------------------------------------------
## 3. Traps

1. Coverage % is gameable: 100 % line coverage with no assertions. Pair with mutation score; SQLite's value is branch/MC/DC plus independent harnesses, not the number alone.
2. Mutation scores on numerical kernels are inflated by equivalent mutants and depressed by tolerance-insensitive tests; use as a trend, not an absolute gate (hence "provisional").
3. Code-to-code agreement is not validation: shared nuclear data (FENDL/ENDF) and shared geometry mistakes agree perfectly while both wrong. Require experimental C/E with measurement uncertainty.
4. C/E within 2 sigma is meaningless if the statistical error is huge; report the sigma, and require a minimum precision, else "not evaluated".
5. "Qualified/validated" language: no regulator qualifies a fusion code today; an overclaim is the fastest way to lose credibility. The NQA-1 posture is process evidence, not physics truth.
6. Crash-free % depends on denominator (sessions vs users) and on opt-in bias (users who crash opt out). Use soak tests as the primary evidence for a desktop tool, telemetry as secondary.
7. Startup time: cold vs warm, first window vs interactive, GPU init. Disclose the definition and reference hardware; a splash screen is not startup.
8. Frame-time averages hide stutter; use P99 and max, and test with real data. Continuous-repaint egui apps look fast but burn battery.
9. Perf CI on shared runners has 10-30 % noise on wall time; use instruction counts or dedicated hardware.
10. Reproducible builds on Windows/macOS are hard (signing embeds timestamps); claim Linux only until proven.
11. OpenSSF Scorecard measures repo hygiene, not security; a 10/10 repo can ship a vulnerable app. SBOM presence is not SBOM quality (NTIA elements, completeness).
12. cargo-audit only knows reported advisories; cargo-vet is only as good as the audits imported. `unsafe` count excludes dependencies and C libraries.
13. Floating-point determinism: FMA, libm transcendental differences, parallel reduction order, and compiler flags change last digits across OS/CPU. Bitwise-identical cross-platform results are a trap; specify tolerances.
14. MC seeds: same seed on different thread counts changes results unless the RNG stream is per-history. Declare which reproducibility class a test uses.
15. Autosave without atomic write can corrupt the autosave itself; fsync directories; test with fault injection, not by hope.
16. Zip/zstd project files are an attack surface (bombs, path traversal, duplicate entries, symlinks); content-addressing alone does not prevent it.
17. Telemetry "anonymous" is rarely anonymous (paths, geometry of an unreleased plant design are IP). For a fusion-design tool, study content is commercially sensitive; default off and no content ever.
18. Auto-update adds a remote-code-execution channel; a signed-but-rollback-able update is an attack. Weigh whether auto-update is needed at all for air-gapped-heavy users.
19. CRA scope: AGPL FOSS "non-commercial" exemption is narrow; offering paid support, a hosted service, or paid account tiers may bring FARIS into scope. Reporting obligations already apply since 2026-09-11 if in scope. Decide with counsel.
20. Export control: simulation outputs and model files for specific fusion designs (tritium handling, enrichment-adjacent) may be controlled even when the software is open source; and bundling data under non-redistributable licences breaks distribution. Keep a per-dataset manifest.
21. Backward-compat claims need an archived fixture from every release; "we think old files open" is untested.
22. Test-suite time creep: unit suites that grow past ~10 min get skipped by developers; enforce size budgets.
23. Validation suite rot: benchmarks that depend on moving external data (FENDL versions) silently change results; pin by hash.
24. Quality scorecards with too many metrics invite Goodhart behaviour; limit to the measures that gate releases.
