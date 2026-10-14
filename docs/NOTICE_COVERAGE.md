<!-- kind: reference -->
# Dependency notice coverage on other platforms

`docs/THIRD_PARTY_NOTICES.md` and `docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md` cover the locked Linux x86_64 graph
(`x86_64-unknown-linux-gnu`). Each platform's download carries its own notices, collected from that platform's graph
by `.github/workflows/desktop.yml` with `scripts/collect_dependency_notices.py` and
`cargo metadata --filter-platform TARGET`. The Linux files in this folder are therefore not the notices a Windows or
macOS user receives.

This page lists the registry packages that the shipped Windows and macOS (Apple silicon) graphs need and the Linux
file does not name. It was computed on 2026-10-14 from `cargo metadata --locked --offline --filter-platform` for
`x86_64-pc-windows-msvc` and `aarch64-apple-darwin` against the 305 entries of the Linux file. Together the two targets
need 70 packages that the committed file lacks (38 for Windows, 45 for macOS; some are shared). Their texts are
produced at packaging time by the workflow; they are listed here so a reviewer can see what the other platforms add and
check each declared licence.

This is a point-in-time list, not a generated check. Regenerate it with the commands above whenever `Cargo.lock`
changes. The requirement that the committed notices cover every shipped platform (LEG-013) is met only when a script
compares each platform's notices with its graph; that script does not exist yet.

| Package | Declared licence |
| --- | --- |
| aho-corasick 1.1.5 | Unlicense OR MIT |
| anstyle-wincon 3.0.11 | MIT OR Apache-2.0 |
| avila-account 0.1.2 | MIT OR Apache-2.0 |
| bitflags 1.3.2 | MIT/Apache-2.0 |
| block2 0.5.1 | MIT |
| block2 0.6.2 | MIT |
| cgl 0.3.2 | MIT / Apache-2.0 |
| clipboard-win 5.4.1 | BSL-1.0 |
| core-foundation 0.9.4 | MIT OR Apache-2.0 |
| core-foundation-sys 0.8.7 | MIT OR Apache-2.0 |
| core-graphics 0.23.2 | MIT OR Apache-2.0 |
| core-graphics-types 0.1.3 | MIT OR Apache-2.0 |
| cxx 1.0.205 | MIT OR Apache-2.0 |
| cxx-build 1.0.205 | MIT OR Apache-2.0 |
| cxxbridge-flags 1.0.205 | MIT OR Apache-2.0 |
| cxxbridge-macro 1.0.205 | MIT OR Apache-2.0 |
| dispatch 0.2.0 | MIT |
| dispatch2 0.3.1 | Zlib OR Apache-2.0 OR MIT |
| error-code 3.4.0 | BSL-1.0 |
| foreign-types 0.5.0 | MIT/Apache-2.0 |
| foreign-types-macros 0.2.4 | MIT/Apache-2.0 |
| foreign-types-shared 0.3.1 | MIT/Apache-2.0 |
| glutin_wgl_sys 0.6.1 | Apache-2.0 |
| link-cplusplus 1.0.12 | MIT OR Apache-2.0 |
| num-bigint 0.4.8 | MIT OR Apache-2.0 |
| num-integer 0.1.47 | MIT OR Apache-2.0 |
| objc-sys 0.3.5 | MIT |
| objc2 0.5.2 | MIT |
| objc2 0.6.4 | MIT |
| objc2-app-kit 0.2.2 | MIT |
| objc2-app-kit 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-data 0.2.2 | MIT |
| objc2-core-foundation 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-graphics 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-image 0.2.2 | MIT |
| objc2-encode 4.1.0 | MIT |
| objc2-foundation 0.2.2 | MIT |
| objc2-foundation 0.3.2 | MIT |
| objc2-io-surface 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-metal 0.2.2 | MIT |
| objc2-metal 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-quartz-core 0.2.2 | MIT |
| objc2-quartz-core 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| once_cell_polyfill 1.70.2 | MIT OR Apache-2.0 |
| range-alloc 0.1.5 | MIT OR Apache-2.0 |
| raw-window-metal 1.1.0 | MIT OR Apache-2.0 |
| regex 1.13.1 | MIT OR Apache-2.0 |
| regex-automata 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax 0.8.11 | MIT OR Apache-2.0 |
| scratch 1.0.9 | MIT OR Apache-2.0 |
| wgpu-core-deps-apple 30.0.1 | MIT OR Apache-2.0 |
| winapi-util 0.1.11 | Unlicense OR MIT |
| windows 0.62.2 | MIT OR Apache-2.0 |
| windows-collections 0.3.2 | MIT OR Apache-2.0 |
| windows-core 0.62.2 | MIT OR Apache-2.0 |
| windows-future 0.3.2 | MIT OR Apache-2.0 |
| windows-implement 0.60.2 | MIT OR Apache-2.0 |
| windows-interface 0.59.3 | MIT OR Apache-2.0 |
| windows-link 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics 0.3.1 | MIT OR Apache-2.0 |
| windows-result 0.4.1 | MIT OR Apache-2.0 |
| windows-strings 0.5.1 | MIT OR Apache-2.0 |
| windows-sys 0.52.0 | MIT OR Apache-2.0 |
| windows-sys 0.60.2 | MIT OR Apache-2.0 |
| windows-sys 0.61.2 | MIT OR Apache-2.0 |
| windows-targets 0.52.6 | MIT OR Apache-2.0 |
| windows-targets 0.53.5 | MIT OR Apache-2.0 |
| windows-threading 0.2.1 | MIT OR Apache-2.0 |
| windows_x86_64_msvc 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc 0.53.1 | MIT OR Apache-2.0 |

## Where each licence text comes from

Every entry in the notices names the source of each text and gives its SHA-256, and the generator stops with an error for
a package that ends up with no text (`scripts/collect_dependency_notices.py`, tested in
`scripts/test_collect_dependency_notices.py`). There are three sources:

1. The licence files inside the packaged crate, read from the Cargo cache. These are the exact packaged source and are
   the default.
2. Texts pinned from the crate's upstream repository at the commit recorded inside the crate, for crates that package
   no licence file (and for the egui and krilla families). They are kept in `licenses/upstream-rust/`, the notices show
   the raw URL they came from, and the generator refuses a file whose hash differs from `licenses/upstream-rust/manifest.json`.
   The build does not fetch them again. In the Linux file this covers accesskit, the egui family, gl_generator,
   khronos_api, krilla, krilla-svg, profiling and spirv.
3. The standard SPDX text, for crates whose upstream publishes no full licence text. The label says "standard ... text",
   names the SPDX list version and gives the reason; the hash is checked against `licenses/spdx-standard/manifest.json`.
