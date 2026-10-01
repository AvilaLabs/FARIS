# FARIS demo acceptance status

**Evidence snapshot:** 2026-10-01. This is a status report, not a claim that the
functional demo is complete. The exit criteria below remain open until the
listed end-to-end evidence is produced from the current inputs and is replayed
through both CLI and native app. Transport completion, software fixture verdicts,
and numerical control agreement do not qualify the reactor model.

## Acceptance by roadmap area

| Roadmap IDs | Status | Evidence and remaining gap |
| --- | --- | --- |
| S01–S06 | Partial | Typed study selections/dependencies, explicit input provenance, and scoped result states exist in `crates/faris-model`, `crates/faris-engine`, and `docs/COLD_REFERENCE.md`. Need final reviewed study coverage and both fresh, identical-control runs. |
| G01–G02, G06–G07 | Implemented, review pending | Scenario-derived geometry feeds display and OpenMC; allocation variants share an envelope; display cutaway is separate; surrogate and unresolved structure are documented. Final native inspection and paired-input diff are still required. |
| G03–G05 | Partial | Finite outboard port and feature-free control inputs exist. `controls/check_port_geometry.py` independently uses SciPy adaptive quadrature and binds to run/audit identities, while OpenMC point classification and Rust midpoint integration are separate checks. No accepted port-run volume report is recorded yet; do not present a port effect as measured. |
| A01–A02, A04–A06 | Partial | Explicit cold-data recipes, source distribution/strength, and input hashes are recorded. The source-rate and unit arithmetic has independent controls. Physical material state is unspecified and remains a cold numerical idealization. |
| A03 | Partial | Current local combined data uses unchanged FENDL neutron files and NNDC ENDF/B-VII.1 photon/relaxation data converted by OpenMC 0.15.3; hashes and conversion details are in `references/photon-library-provenance.json`. FENDL publisher provenance is unauthenticated and NNDC redistribution rights are unresolved. These files are local and excluded from Git. |
| T01–T06 | Partial | Real OpenMC execution, Rust normalization, spectra, mesh responses, coupled-heating schemas, and an earlier hardened-worker smoke are implemented. Earlier 1M runs predate the valid photon-data worker and are not final demo records. A corrected paired 1M run and reinspection are pending. |
| T07, V01, V03 | Not demonstrated | No response-specific physical reference validation, converged magnet/local response, full sampling/refinement campaign, or qualified uncertainty interval exists. Monte Carlo standard error covers sampling only. |
| V02 | Partial | Source-rate, unit, volume, arithmetic, absorber, and history controls exist. Port-run volume identity and transport/reference applicability remain open. |
| L01–L10, V04 | Partial | Rust history and comparison operations, mass-balance/event/refinement controls, and a 27-point sensitivity artifact exist. `references/operating-history-verification.json` shows numerical controls, but its transport drivers came from superseded neutron-only runs; repeat against eligible coupled runs. Authored service triggers and operating assumptions are not plant limits. |
| C01–C06 | Partial | Paired deterministic history comparison and a 27-point authored sensitivity sweep are implemented and scoped. A supported ranking is not established; uncertainty covariance and physical-model uncertainty are not supplied. |
| K01–K12, V05 | Partial | Real Core compilation/evidence execution and input-binding checks are exercised. `runs/core-integration-controls-001.json` covers genuine software fixture PASS/FAIL/INCONCLUSIVE/NOT_EVALUATED states and rejection mutations. These are software-only controls; actual physical qualification remains NOT_EVALUATED. Rebuild final evidence against eligible coupled runs. |
| U01–U11, V07 | Partial | Native egui/wgpu viewport, scene-derived framing, axes/grid, component selection, scalar fields, study/history/transport panels, and worker controls are implemented. Final packaged-result walkthrough, supported-size/accessibility review, live field picking/uncertainty checks, and sustained 1440×900 performance measurement remain open. |
| J01–J08 | Partial | Worker execution supports cancellation, child cleanup, log bounds, process/address-space and artifact caps, identity-checked reuse, and exclusive result directories. Focused engine tests and workspace tests pass. Fresh solver resource measurements on the named workstation and full UI cancel/reopen tests are still needed. |
| P01–P06, V06 | Not complete | No final portable pair of corrected transport/history/Core records or release binary exists. `scripts/package_recorded_demo.py` builds hash-indexed Core case bundles only from revalidated run records; it intentionally does not bundle statepoints, nuclear data, or claim scientific qualification. Run it after eligible paired records exist. |

## Evidence boundaries

- `controls/check_transport_arithmetic.py` checks arithmetic and volume conversions independently with Decimal precision. It does not check Monte Carlo correctness or physical model validity.
- `controls/check_history.py` checks history equations and balance independently. It does not qualify the transport inputs or scenario assumptions.
- `controls/pure_absorber_sphere.py` and `controls/test_pure_absorber_sphere.py` exercise an independent OpenMC numerical-control problem. They do not establish accuracy for this toroidal design.
- `controls/check_port_geometry.py` is a geometry-volume cross-check. Adaptive quadrature, midpoint convergence, and point classification still require comparison at the exact port scenario/run identity; agreement cannot validate materials or nuclear data.
- Core fixture assessments in `runs/core-integration-controls-001.json` test Core semantics and FARIS declaration binding only. A fixture `PASS` is not a physical FARIS `PASS`.
- Any display of history inventory, service events, or net power must identify the assumptions and underlying transport run. Missing whole-model heating leaves recovered heat and electricity unavailable.

## Final release gate

Before calling M7 complete, rerun the sequence in [DEMO_WALKTHROUGH.md](DEMO_WALKTHROUGH.md) with the fresh corrected reference, breeder-emphasis, matched control, and port records. Save their run identities and acceptance outputs in the package index. Require exact record revalidation, nonempty provenance, responsive native interaction, Core evidence replay, external-data acquisition instructions, and a clear NOT_EVALUATED qualification statement. If a response misses its declared sampling goal or paired results overlap within their supported uncertainty, retain that response as unresolved instead of forcing a ranking.
