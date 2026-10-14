<!-- kind: reference -->
# Supported platforms

Each row names the CI job that is its evidence. `scripts/platform_table.py --check` fails the build when a row names a job that does not exist in `.github/workflows`, or a runner label that job does not use. A row marked NONE has no hosted runner; the reason is stated and nothing is claimed for it.

Tier 1 is built, tested and supported. Tier 2 is built and tested, with fixes as time allows. Tier 3 (other distributions and versions) is best effort and has no evidence. The versions are choices made before the first public release and may change.

What the jobs do: `ci.yml:check` builds the whole workspace and runs every Rust and Python test. `platforms.yml:native` builds the command line and the desktop app and runs the Rust tests of the command-line crates, then runs `faris --version`, `faris validate` and `faris doctor`. The desktop app is built there but not started, because hosted runners have no display; the packaged app is started by the release smoke test, which runs on a release, not on every push. `platforms.yml:linux-old-glibc` builds the command line on Ubuntu 20.04 (glibc 2.31) and runs it in four distributions.

<!-- platform-table:start -->
| Platform | Tier | Runner | Evidence | What it shows |
| --- | --- | --- | --- | --- |
| Ubuntu 24.04 LTS, x86_64 | 1 | `ubuntu-latest` | `ci.yml:check` | Whole workspace builds and passes its tests. The label `ubuntu-latest` is Ubuntu 24.04 today. |
| Ubuntu 26.04 LTS, x86_64 | 1 | `ubuntu:26.04` | `platforms.yml:linux-old-glibc` | The command-line binary runs in an Ubuntu 26.04 container. No hosted 26.04 runner exists, so it is not built or tested there. |
| Linux, glibc 2.31 floor | 1 | `ubuntu:20.04` | `platforms.yml:linux-old-glibc` | The command line is built on Ubuntu 20.04 (glibc 2.31) and runs on Debian 11, Ubuntu 20.04, 24.04 and 26.04 (Debian 11 is a run-only check). The desktop app is built on Ubuntu 22.04 (`desktop.yml`) and needs glibc 2.35; it is not yet held to the 2.31 floor. |
| Windows 11, x86_64 | 1 | `windows-2025` | `platforms.yml:native` | Command line and app build, Rust tests run. The runner is Windows Server 2025, which shares Windows 11's code base but is not Windows 11. |
| Windows 10 22H2, x86_64 | 1 | none | NONE: GitHub offers no Windows 10 runner. Windows Server 2022 (`windows-2022`, `platforms.yml:native`) is the nearest. | Nothing is claimed for Windows 10. |
| macOS 14, Apple silicon | 1 | `macos-14` | `platforms.yml:native` | Command line and app build, Rust tests run. |
| macOS 15, Apple silicon | 1 | `macos-15` | `platforms.yml:native` | Same, on the newer release. |
| Linux aarch64 (Ubuntu 24.04) | 2 | `ubuntu-24.04-arm` | `platforms.yml:native` | Command line and app build, Rust tests run. |
| macOS 15, Intel | 2 | `macos-15-intel` | `platforms.yml:native` | Command line and app build, Rust tests run. |
<!-- platform-table:end -->

## Feature matrix

What each platform has been shown to do. A dash means nothing is claimed. "Compiled" means the code is built there; "run" means a person or a test started it and checked the result.

| Feature | Linux x86_64 | Linux aarch64 | Windows 11 x86_64 | macOS (Apple silicon) | macOS (Intel) | Browser |
| --- | --- | --- | --- | --- | --- | --- |
| Command line (`faris`) | run in CI | run in CI | run in CI | run in CI | run in CI | not applicable |
| Desktop app (`faris-app`) | run by hand on the reference laptop; packaged check on a release | compiled | compiled; packaged check on a release | compiled; packaged check on a release | compiled; packaged check on a release | not applicable |
| X11 session | compiled in; run by hand on the reference laptop | compiled in | not applicable | not applicable | not applicable | not applicable |
| Wayland session | compiled in; run by hand on the reference laptop | compiled in | not applicable | not applicable | not applicable | not applicable |
| Recorded study, 30-year history | digests checked in CI | Rust tests in CI; digests not checked | digests checked in CI | digests checked in CI | digests checked in CI | opened in the browser smoke test |
| New transport run (OpenMC) | run on the reference laptop | - | - | - | - | not available |
| Computed maintenance durations (ACTINV) | run on the reference laptop | - | - | - | - | not available |

## Linux display servers

The desktop app builds with both the X11 and the Wayland back ends of its window library, and it picks the one your session offers. The native walkthrough of the demo (recorded in `references/native-demo-verification.json`) ran on one workstation under both: on X11 at 1440 by 900 the app drew 89.33 frames per second with a 95th-percentile frame time of 11.88 ms, and on Wayland at 1260 by 840 with a 1.667 interface scale it drew 62.07 frames per second with 22.60 ms. These figures describe that workstation and are not a promise for yours.

What is not tested: a headless X11 or Wayland session in CI, and any other compositor or driver. Hosted runners have no graphics adapter, so the packaged app is started there only by the release smoke test, which tries one window capture and reports a failure without failing the release. If the window does not open under one session type, start the app under the other (for example with `WAYLAND_DISPLAY` unset to force X11) and report the compositor and driver on the issue tracker.

## Graphics device

The reference device is the laptop's integrated Intel UHD Graphics (Alder Lake-N, 32 execution units). It is not a UHD 620. The 2026-09-30 frame-time profile of the 3D viewport on that device gave a 95th-percentile frame time of 12.1 ms and a mean of 90 frames per second on the demo model. The 99th percentile and the longest frame were not measured, so the 60 Hz target is not yet shown to hold.

The stated minimum is an integrated Intel UHD 620-class device. That is a working figure from memory, not a tested one: nothing here has been run on a UHD 620. If the viewport is slow, lower the window size or the interface scale; the rest of the program does not depend on the graphics device.

## What each mode needs

| Mode | Memory and time, as measured | Source |
| --- | --- | --- |
| Open a recorded study and recalculate its 30-year histories | Calculated histories recalculate on opening in about a second. No peak memory figure has been taken. | [Study files](study-files.md) |
| Computed maintenance durations, recorded result | Opens from the recorded result with no calculation. A new calculation needs ACTINV and has no timing table yet. | [Computed maintenance durations](maintenance.md) |
| New transport run on the demo model (RM-S) | The four primary runs of 1 million histories each took 1,178 to 1,365 seconds on the reference laptop, 11 MB retained in 16 files each. A 30 million history run took about 68 minutes on 7 threads. A magnet-targeted run with weight windows took 3.4 to 6.4 minutes on 3 threads. Peak memory was not captured. FARIS caps every external job at 4 GiB by default. | [Run your own transport](transport.md) |
| New transport run on a CAD sector (RM-M) | Not measured: the medium reference model does not exist yet. | none |
| New transport run on a full plant (RM-L) | Not measured: the large reference model does not exist yet. | none |

The table fills in as each row is measured at a release. A cell that says "not measured" is a gap, not an estimate.
