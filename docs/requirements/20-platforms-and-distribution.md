# Platforms and distribution

Where FARIS runs, how it is installed, updated and removed, how nuclear data reaches the machine, and what hardware it needs. Today FARIS builds and runs on Linux only. The targets below set the support matrix and the packaging that make it usable by people who are not the author. Interactive performance targets stay in the performance file; this file only adds what must hold on each platform. See the [index](README.md#reference-hardware-and-models) for RL (Ubuntu 26.04 LTS), RW, RC and the reference models RM-S, RM-M and RM-L.

## Operating-system support

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-001 | FARIS shall publish a support table with tiers and automated evidence for each row. | Tier 1: Linux x86_64 (Ubuntu 24.04 LTS and 26.04 LTS, glibc 2.31 or later), Windows 11 and Windows 10 22H2 x86_64, macOS 14 or later on Apple silicon. Tier 2: Linux aarch64, macOS on Intel. Tier 3: other distributions, best effort (provisional: versions are choices; confirm before the first public release). | CI matrix on real or virtual machines for every Tier 1 and 2 row; table checked against the matrix. | R5 PD-01 [U]; glibc floor idea manylinux_2_28 [V](https://github.com/pypa/manylinux) | F1 | Partial: Linux x86_64 on Ubuntu 26.04 only; CI runs ubuntu-latest |
| PLAT-002 | Every release shall pass the end-to-end scenario (new study, run recorded transport, compare, export, reopen) on each Tier 1 platform. | 100 % pass per release. | CI matrix running the scripted scenario. | R5 TP-14 | F1 | Partial: scripted checks on Linux only |
| PLAT-003 | The Linux build shall run on distributions older than the build machine. | Binary runs on Ubuntu 24.04 and on a distribution with glibc 2.31 (provisional: floor trades against dependencies; confirm before first release). | Run the release binary in containers of each distribution. | R5 PD-01 | F1 | No: built and run on Ubuntu 26.04 only |
| PLAT-004 | FARIS shall build and run natively on Windows. | Tier 1 scenario passes; no Unix-only code on the default path. | Windows CI job. | FARIS choice | F2 | No: faris-engine job runner uses the nix crate and prlimit under cfg(unix) |
| PLAT-005 | FARIS shall build and run natively on macOS. | Tier 1 scenario passes on Apple silicon. | macOS CI job. | FARIS choice | F2 | No |
| PLAT-006 | FARIS shall run on both X11 and Wayland on Linux. | Tier 1 scenario passes under each; no crash on startup under either. | CI with headless X11 and Wayland compositors. | eframe features x11 and wayland enabled (Cargo.toml) | F1 | Partial: both features compiled in; only the reference session tested |
| PLAT-007 | Behaviour that differs by platform (paths, line endings, file locking, fonts, scaling) shall be covered by tests. | Unicode and space paths, 100 % to 300 % display scaling, non-English locales and decimal comma pass on every Tier 1 platform. | CI matrix with locale and scaling variables. | R5 PD-22 | F1 | Partial: UI scale checks recorded in references/native-ui-scale-checks; Linux only |
| PLAT-008 | The support window for each release shall be published. | Security fixes for at least 3 years after release; at least 5 years for releases designated long-term support if the Cyber Resilience Act applies (SEC-075). | Policy document and release log. | R5 PD-12; CRA 5 years [U] | F4 | No |
| PLAT-009 | Public interfaces shall follow semantic versioning: crates, command line, JSON output and file format. | 0 breaking changes in minor releases, checked by tooling; file format version stated separately from the program version. | cargo-semver-checks; schema diff in CI. | semver.org [V as a spec]; R5 PD-13; R4 trap 2 | F1 | No: workspace version 0.0.1 |

## Graphics

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-010 | FARIS shall render on Vulkan, Metal and DirectX 12 through wgpu. | Viewport tests pass on each backend on reference GPUs; Vulkan 1.1, DirectX 12 feature level 11_0, current Metal. | Hardware CI per backend. | wgpu backends [V](https://wgpu.rs/) | F1 | Partial: Vulkan on Intel UHD (RL) only; wgpu is required by the 3D viewport |
| PLAT-011 | The minimum GPU shall be an integrated Intel UHD 620-class device. | PERF-011 frame-time targets met on RL's Intel UHD; viewport opens on 100 % of Tier 1 minimum devices (provisional: UHD 620 class is a recall figure [U]; confirm before the first release). | Frame-time harness on the minimum device. | RL is the reference device | F1 | Partial: PERF-011 profile on RL, P95 12.1 ms |
| PLAT-012 | FARIS shall fall back to a software renderer when no usable GPU is found, and say so. | Opens and renders RM-S at 5 frames per second or better with Mesa lavapipe on Linux and WARP on Windows; banner names the fallback; receipts mark rendering as software (REL-033). | Virtual-machine test with no GPU. | R5 PD-03; lavapipe and WARP [V as names] | F1 | No: the app refuses to start without wgpu |
| PLAT-013 | A missing or failing GPU driver shall produce a plain message with a next step, not a panic. | 100 % of injected adapter-request failures show a message and exit cleanly or fall back. | Fault injection. | REL-032 | F1 | Partial: startup returns the error "wgpu is required for the 3D viewport"; no next-step text |
| PLAT-014 | Display scaling and multi-monitor moves shall work without a restart. | 100 %, 150 %, 200 %, 300 % scale and moving between monitors with different scales produce no artefacts, verified by screenshots. | Screenshot-diff harness. | Existing scaling checks | F2 | Partial: scale checks recorded for Linux |

## Packaging, install and removal

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-020 | FARIS shall ship at least two Linux package formats per release: a .deb and an AppImage or Flatpak, plus a tarball. | 3 artefacts per release, each installed and started in a clean container or virtual machine. | Install test per artefact. | R5 PD-05 | F1 | No: release binary 31 MB and a user-level desktop integration script only |
| PLAT-021 | Windows shall ship as a signed MSI or MSIX and a portable zip. | Silent install with `/qn` works; uninstall leaves 0 files except user data. | Install and uninstall test with a file-system diff. | R5 PD-06 | F2 | No |
| PLAT-022 | macOS shall ship as a signed, hardened, notarised and stapled disk image. | `spctl -a -vvv` and `stapler validate` pass offline. | macOS verification step in the release pipeline. | R5 SC-07 / PD-07 | F2 | No |
| PLAT-023 | Installation shall be clean and reversible: the installer lists every file it writes and the uninstaller removes exactly those, leaving user data. | 0 residual files outside user data after uninstall, by file-system diff on each platform. | Install, uninstall and diff test. | R5 PD-18 | F1 | Partial: Linux desktop integration script writes three named files under XDG paths and its --uninstall removes exactly those three |
| PLAT-024 | FARIS shall register the .faris type, icon and "Open with" on each platform. | Double-click opens the study on each Tier 1 platform; MIME type application/vnd.avila-labs.faris-study. | Install test plus open-by-association test. | docs/STUDY_FILE.md container type | F1 | Partial: Linux per-user MIME and desktop entry; icon 256 px |
| PLAT-025 | Windows binaries shall be Authenticode-signed with a timestamp, and macOS binaries Developer ID-signed. | 100 % of binaries and installers signed; signature verified in the release pipeline. | signtool verify; codesign --verify. | R5 SC-06/SC-07; certificate validity 460 days or less [V](https://knowledge.digicert.com/alerts/code-signing-certificates-459-day-validity) | F2 | No |
| PLAT-026 | Installed size and install time shall be bounded. | App at most 150 MB excluding nuclear data and sample results (PERF-045); install at most 60 s on an SSD. | Package check; timed install. | R5 PD-20; PERF-045 | F1 | Met: 31 MB release binary for the size bound; install time unmeasured |
| PLAT-027 | Installers shall be usable by keyboard and screen reader. | Checklist passes on each platform (the accessibility audit itself is owned by the accessibility file). | Checklist plus audit. | R5 PD-21 | F2 | No |
| PLAT-028 | A portable mode shall keep configuration and workspace beside the program. | Runs from a read-only folder and a USB drive; writes nothing outside the chosen folder. | File-system diff test. | R5 PD-19 | F2 | No |
| PLAT-029 | FARIS shall never write into the system temporary folder for large data. | Workspaces and caches under the runs directory or a configured folder; 0 files over 1 MB in the system temporary folder in an audit. | File audit during a full session. | /tmp may be memory-backed [internal]; docs/STUDY_FILE.md "Opening in the app" | F1 | Met: studies unpack to a workspace in the runs directory, not /tmp |

## Updates

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-030 | Update checking shall be opt-in, and never on by default. | 0 update traffic with the setting off (SEC-050). | Network trace. | R5 SC-18/SC-19 | F4 | Met: no update mechanism exists |
| PLAT-031 | Updates shall be signature-verified with rollback and downgrade protection. | Tampered, truncated, replayed or older update rejected in 100 % of tests. | Update fault test. | TUF and Sparkle practice [V as tools] | F4 | No |
| PLAT-032 | A user shall be able to roll back to the previous release without affecting studies. | Rollback at most 2 minutes; 0 study files modified (REL-025). | Rollback test with a hash check on a fixture library. | R5 PD-24 | F4 | No |
| PLAT-033 | Releases shall follow a published channel and cadence. | Stable channel plus a beta channel; release log lists date, version and changed numbers; cadence stated (provisional: every 3 months; confirm before 1.0). | Release log audit. | R5 PD-11 | F4 | No |
| PLAT-034 | A release shall list every change that can alter a published number. | 100 % of numeric-changing changes in the notes, found by a golden-study diff (PRV-027). | Golden-study diff in the release gate. | R4 trap 6 | F1 | No |

## Offline installation and data delivery

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-040 | FARIS shall install from a single offline bundle with checksum and signature, containing the program, documentation and, as an option, the nuclear data. | Install on an air-gapped virtual machine succeeds; the bundle verifies before install; 0 network calls. | Install test in a network-less namespace. | R5 PD-08/PD-09 | F1 | Partial: a local recorded-demo package exists (dist/FARIS-demo-2026-10-01, 446 MB) with a launcher and a binary manifest check; not a signed installer |
| PLAT-041 | Documentation, tutorials and licences shall be installable offline and match the installed version. | 100 % of pages available offline; version stamp matches. | Offline test and version check. | PLAT-040 | F1 | Partial: docs are in the repository and package |
| PLAT-050 | Nuclear data shall be delivered as separate, versioned, hash-verified packs with a licence manifest, never inside the program. | Each pack names library, version, size, SHA-256 and licence; install verifies the hash before use; a mismatch leaves the previous pack in place. | Install, corrupt and verify test. | R5 PD-10; LEG-010 | F1 | Partial: local library assembled by scripts with provenance JSON (combined FENDL-3.2 and ENDF/B-VII.1 photon root, 164 MB); no pack format |
| PLAT-051 | Data download shall be resumable and verify piece by piece. | An interrupted download resumes in 100 % of tests; the final hash is checked; a partial file is never used (REL-038). | Interrupt test. | FARIS choice | F2 | No |
| PLAT-052 | Pack sizes shall be published before download, and the default pack shall be the smallest that runs the reference studies. | Size shown before download; default pack for RM-S at most 250 MB (provisional: the local combined library is 164 MB, larger libraries will exceed this; confirm before F2 gate). | Package audit. | Local library measured 164 MB (du, 2026-10-05) | F1 | Partial: 164 MB measured; no packs |
| PLAT-053 | A data pack from a source with no explicit redistribution right shall be fetched by the user from the source, not redistributed by FARIS. | 0 such packs in release artefacts (LEG-011). | Package scan. | docs/PHOTON_LIBRARY_ACQUISITION.md: NNDC download page gives no explicit licence | F1 | Met: acquisition is by documented scripts; data/ is ignored by git |
| PLAT-054 | FARIS shall check on opening that the installed data pack matches the identity recorded in the study, and say what differs. | 100 % of mismatches reported; the run blocked or labelled (PRV-002). | Swapped-pack test. | PRV-002 | F2 | No |

## Headless and server use

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-060 | FARIS shall run all non-viewport operations headless, with no display or GPU. | The command line passes the end-to-end scenario with no display server and no GPU in a container. | Container test. | R5 PD-23; AGENTS.md CLI parity | F1 | Partial: faris-cli is the default workspace member and has no graphics dependency; CLI export missing (COL-007) |
| PLAT-061 | FARIS shall publish a container image for headless runs. | Image at most 1 GB without data, runs without a display, signed, with SBOM (SEC-030). | Container build and run test. | R5 PD-23 | F2 | No |
| PLAT-062 | Cluster use shall be documented and tested on a containerised scheduler. | Slurm example runs the sweep on a containerised cluster in CI. | CI job. | R4 trap 17: untested cluster support rots | F6 | No |

## Hardware and builds

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-070 | Release binaries shall be reproducible on Linux from a pinned toolchain and locked dependencies. | Two independent builds yield identical SHA-256; documented for Windows and macOS as not guaranteed. | Independent rebuild job. | R5 SC-08; reproducible-builds.org [V]; OpenSSF gold requires it [V](https://www.bestpractices.dev/en/criteria/2) | F2 | Partial: toolchain pinned (rust-toolchain.toml, 1.98.1), Cargo.lock committed, CI uses --locked; no rebuild check |
| PLAT-071 | FARIS shall publish minimum and recommended hardware, tested on a machine of that size. | Minimum: 4 cores, 8 GB memory, 4 GB disk plus data. Recommended: 8 cores, 16 GB (provisional: untested; confirm before the first release). Reference laptop: 8 cores, 30 GB. | Run the reference scenario on a minimum-size virtual machine. | R5 PD-04 | F1 | Partial: runs on RL; smaller machines untested |
| PLAT-072 | The program shall state what each mode needs: a recorded-transport study versus a new transport run. | Published table of memory and time for RM-S, RM-M and RM-L, from measurement. | Measured table per release (PERF-030). | PERF-041, PERF-043 | F2 | Partial: recorded-study open is light; a 1 M history OpenMC case takes about 20 min (1178–1365 s) on RL |
| PLAT-073 | A release checklist shall be executable: one command runs every gate in this file and the other requirement files that apply to releases. | Command exits non-zero on any failed gate; log kept with the release. | Dry-run on a release candidate (see QA-002 for the trace matrix). | FARIS choice | F1 | No |

## Traps

- "Works on Windows" usually means it compiled. Require the end-to-end scenario on real or virtual machines, not a build.
- Reproducible builds on Windows and macOS are blocked by signing timestamps. Claim Linux only until proven.
- A package that runs on the build machine can still need a newer libc than the user has. Test in older containers.
- GPU support tested only on one integrated GPU says little about drivers on other vendors. Name the devices tested.
- Auto-update adds an attack channel and a way to change published numbers. Opt-in, signed, with rollback and a numeric-change list.
- Data size grows faster than the program. A 164 MB library is small; full evaluated libraries are not. State sizes before download.
- A recorded demo package is not an installer. Keep "demo runs from a folder" separate from "installs and uninstalls cleanly".
- Certificate lifetimes are short (460 days or less), so signing is a recurring process, not a one-off.
