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
