<!-- kind: reference -->
# External tools and their licences

FARIS calls a few outside tools and reads outside data. This page lists each one with its licence and where you get it. Licences were read from each project's own licence file on 2026-10-14. Check the project's page for its current terms before you redistribute anything.

No restricted tool is bundled. Every program in a FARIS download is open source, and the download contains no nuclear data, no OpenMC and no code from an access-controlled library. The licence texts for what is bundled are in the package's `licenses/` folder and in the [third-party notices](https://github.com/AvilaLabs/FARIS/blob/main/docs/THIRD_PARTY_NOTICES.md).

| Tool or data | What FARIS uses it for | Licence | How you get it | In the FARIS download? |
| --- | --- | --- | --- | --- |
| FARIS itself | Everything on these pages | AGPL-3.0-only | [Releases](releases.md) and the source repository | Yes |
| [Avila Core](https://github.com/AvilaLabs/Avila-Core) | Compiles a study and runs its four stages under receipts | AGPL-3.0-only | Public source repository; the download holds the built `avila-core` program at a recorded commit | Yes, as a program (`bin/avila-core`) with its licence and dependency notices |
| [OpenMC](https://github.com/openmc-dev/openmc) 0.15.3 | Monte Carlo neutron and photon transport for fresh runs | MIT | Install with conda or from source; see [Run your own transport](transport.md) | No. Recorded results need no OpenMC. |
| [DAGMC](https://github.com/svalinn/DAGMC) | Transport on CAD geometry | Simplified BSD (BSD-2-Clause style) | Install with OpenMC (conda-forge) | No |
| [ACTINV](https://github.com/AvilaLabs/ACTINV) | Activation inventories and decay heat for computed maintenance durations | MIT OR Apache-2.0 | Public source repository, crates.io and PyPI | No. The recorded maintenance result needs no ACTINV. |
| [OpenCASCADE Technology](https://dev.opencascade.org/) | Reading STEP files in the design-file path | LGPL-2.1 with the OCCT exception | Built from source inside the `faris-occt` crate | Only if you build that crate yourself |
| FENDL-3.2 neutron data | Cross sections | IAEA terms; redistribution not settled | [OpenMC data page](https://openmc.org/data/) | No |
| ENDF/B-VII.1 and VIII.1 | Photon data, thermal scattering, gap fill | Public evaluated data of the US Nuclear Data Program | [NNDC](https://www.nndc.bnl.gov/) and the OpenMC data page | No |
| Rust crates (about 440) | Libraries linked into the programs | Mostly MIT or Apache-2.0; each is listed with its text | Downloaded by Cargo at build time | Yes, linked in |

Redistribution of nuclear data is not settled, which is why FARIS never ships it ([Nuclear data](nuclear-data.md)). If you keep a data library on a shared drive, check its terms first.

If a tool you need sits behind an access-controlled library or a licence you have to sign, FARIS does not bundle it and a study never depends on it for a recorded result. FARIS records the identity of every tool and library a run used, so a reader can tell which ones to obtain.
