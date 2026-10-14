<!-- kind: reference -->
# Known limitations

This is the one list of what FARIS 0.2.0 does not do, does not validate, or does only in part. Each row names the requirement it falls under (the [requirements tables](https://github.com/AvilaLabs/FARIS/blob/main/docs/requirements/README.md) hold the target), the date the row was last checked, and what to do meanwhile. [Scope and limits](scope.md) explains the same gaps in prose. When a row here and a result disagree, the row is the later word.

None of these limits is hidden by a verdict: a calculation that depends on one reports NOT_EVALUATED or INCONCLUSIVE with the reason, and does not fill the gap with a number.

Last checked: 2026-10-14.

## Model gaps

| Gap | Requirement | Workaround |
| --- | --- | --- |
| The outboard port is an open duct with no shield plug, so port arrangements bound the streaming case. | NUC-055 | Read the port arrangements as an upper bound on magnet exposure. Compare with the port-free controls. |
| Magnet limits use regional averages, not a local peak fluence or a peak winding-pack power density. | MAG-003 | Treat a region as a screening average; a hot spot can reach a limit sooner. |
| No shutdown dose map and no dose-based maintenance timing. Outage durations come from fixed values, or from the computed decay-heat model for the four recorded arrangements. | OPS-080 | Use the computed maintenance durations ([Computed maintenance durations](maintenance.md)) and read contact dose from ACTINV directly. |
| Heating is an OpenMC deposition score, not a thermal balance. There is no coolant loop or thermal cycle. | THM-010 | Do not read the energy ledger as a thermal-cycle result. |
| No physical degradation model for components beyond the authored fluence limits. | OPS-030 | Treat replacement years as conditional on the authored limits. |
| The geometry is idealised concentric tori with one port, not a published design. | GEO-005 | Bring your own design with [the design file](design-file.md) for a geometry you can defend. |

## Missing or partial validation

| Gap | Requirement | Workaround |
| --- | --- | --- |
| No experimental neutron-transport benchmark is reproduced, and there is no design-level validation. | VAL-020 | Treat every result as a screening result; do not use it as a design basis. |
| The code-to-code comparison against an ITER 1D reference is unavailable without traceable reference responses. | VAL-018 | None yet. The comparison is listed as open work in the scientific baseline. |
| Thermal-scattering data are not applied to the crystalline blanket and shield materials in the recorded study, so low-energy spectra and breeding are not qualified. | NUC-063 | Build the v2 nuclear-data library ([Nuclear data](nuclear-data.md)) and declare thermal tables for your own materials. |
| Nuclear-data uncertainty is not propagated, so the true uncertainty is larger than the ranges shown. | UNC-020 | Read the ranges as transport sampling error only. Compare libraries by hand when the result matters. |
| Magnet fast-flux standard errors are 5 % to 24 % in the recorded runs (up to 30 % in one region). | UNC-001 | Rerun with more histories and variance reduction aimed at the magnet. |

## Unsupported features

| Gap | Requirement | Workaround |
| --- | --- | --- |
| Authored assumptions (service limits, outage durations, processing delays, efficiencies) are not sampled in the uncertainty ensembles. | UNC-010 | Use the what-if sliders to see their effect one at a time. |
| The programs are not code-signed on Windows or notarised on macOS. | PLAT-025 | Check the package hash ([Download and verify](install.md)) and allow the program once. |
| Windows 10 and the Linux desktop app below glibc 2.35 have no automated test. | PLAT-001 | Use the command line, which is held to glibc 2.31, or a supported system ([Supported platforms](platforms.md)). |
| Linux X11 and Wayland: both are compiled in, only the reference session is tested. | PLAT-006 | If one session misbehaves, start the app under the other. |
| Sign-in keeps the token in an owner-only file, not the operating system's credential store. | SEC-041 | Sign out on shared machines ([Avila Labs account](account.md)). |

Next: [Troubleshooting](troubleshooting.md).
