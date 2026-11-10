# JADE ITER 1D TBM candidate references

FARIS registers response-table candidates for `HCPB_TBM_1D` and `WCLL_TBM_1D` from JADE's pinned [JADE-RAW-RESULTS revision](https://github.com/JADE-V-V/JADE-RAW-RESULTS/tree/e3b442cf8e5eb3f41c500535931ddb8a6b0919e7). The source is licensed EUPL-1.2. The CSVs and JADE metadata are not included in Git. FARIS stores only their pinned SHA-256 identities, byte counts, response mapping, and attribution; fetch them into an external directory using `validation/cases/iter-tbm-jade-v1/build_manifest.py`.

Each case has five distinct output responses and 79 rows each: neutron flux, neutron heating, photon flux, photon heating, and tritium production. The JADE metadata identifies MCNP 6.2, FENDL 3.2b, JADE run 3.0.1, post-processing `3.1.1.dev181+g8162787`, and benchmark 1.0. Mesh tally IDs are HCPB 204/214/224/234/244 and WCLL 214/224/234/244/254 in that response order. The pinned [IAEA-NDS input revision](https://github.com/IAEA-NDS/open-benchmarks/tree/872543644e2353ab238242a27c2ce975eb935f32/jade_open_benchmarks/inputs) is verified by exact file hashes before it is recorded in the external candidate manifest.

JADE's CSV `Error` field is preserved as MCNP's estimated relative Monte Carlo standard error (one sigma); for nonzero values FARIS computes the candidate absolute error as `Value × Error`. A `(Value=0, Error=0)` row has unknown precision, not zero uncertainty. The raw values and exact published units remain unchanged. JADE TBM documentation labels mesh flux `#/cm^3/source`, while the MCNP manual defines mesh flux as volume-averaged fluence `#/cm^2/source`; the producer conversion receipt needed to resolve that dimensional discrepancy is unavailable. `Cor A` coordinates span 830.0 to 1084.2 cm with 79 reported rows and 78 intervals in the source deck. Row-to-bin edge/volume mapping is not established. Therefore no OpenMC cell correspondence, unit conversion, normalization confirmation, C/E score, or validation verdict is claimed.

The candidate's own producer metadata says “FENDL 3.2b,” but it does not provide the actual input deck hash, xsdir, ZAID-to-file mapping, processed library hash, executable identity, run seed, or history count. Both pinned MCNP decks contain `.31c`, `.21c`, and `.70c` ZAID suffixes. The `.70c` assignment includes nuclides absent from the audited local FENDL 3.2 general-purpose neutron catalog: As-75 for HCPB; As-75, Pd-102/104/105/106/108/110, and Te-120/122/123/124/125/126/128/130 for WCLL. The official [FENDL-3.2b source tree](https://github.com/IAEA-NDS/FENDL-ENDF/tree/FENDL-3.2b) likewise has no files for those targets. Suffixes alone cannot establish which fallback files the producer used. The local TENDL-2025 OpenMC candidate is unqualified and lacks H-1, H-2, He-3, and He-4; it must not be silently combined with FENDL or represented as producer data.

The committed manifests register five response classes × 79 row slots, with null references and explicit `NOT_EVALUATED` blocks. The external builder retains candidate CSV values and the source-reported precision but leaves them blocked. Eligibility requires recovering the exact producer run/data receipt, reconciling the flux units, resolving every ZAID to qualified nuclear data, and qualifying every row's spatial meaning and mapping. These are code-to-code computational references, not measured validation. Nothing here marks a FARIS requirement `Met`.

## Attribution and pinned artifacts

JADE-V-V, *JADE-RAW-RESULTS*, commit `e3b442cf8e5eb3f41c500535931ddb8a6b0919e7`, HCPB/WCLL MCNP FENDL 3.2b CSV response tables and metadata, EUPL-1.2. The builder verifies SHA-256 and byte length before parsing. Table identities:

| Case | Response | SHA-256 | Bytes |
| --- | --- | --- | ---: |
| HCPB | neutron flux | `1abb17caacb21ab8e3c6c208d744f58712b11946c85056ccf5cbd856021c6a90` | 2412 |
| HCPB | neutron heating | `d82d0e64b2679a43e35ce1b0311b90c92d0804468346daf23a8c0ebb2426a2d2` | 2323 |
| HCPB | photon flux | `3cdef2ec978d898e0e2f8663d95d8f162b64e03662de317d834f3b7b24c2c45f` | 2425 |
| HCPB | photon heating | `f3fad3e66fa6ae7f1219ca75ff4efaaf87459d660fa9cf649af14a3b4cec5ba2` | 2323 |
| HCPB | tritium production | `2fb11119b6ea8bbc588779a0f7d5e02c3f29b6a3691e15ce412f75016dea781f` | 2311 |
| WCLL | neutron flux | `638b87e694aa47090c029e57312c463435a216d3a9f995b2109b93a5ba6dd2b0` | 2392 |
| WCLL | neutron heating | `1f6c045fd976e20fc72ab4e3a19d697bcf83568816d69efdf6a5e9d688c2b5fa` | 2282 |
| WCLL | photon flux | `380db683db52f235a2df81fb3d00db621712637a5a02106eef0c0de71f3525d0` | 2398 |
| WCLL | photon heating | `b18f7e7c1d41ff1c8dc6a935a51a276804455c06339fdb1a3a567ede267d45d6` | 2302 |
| WCLL | tritium production | `9e1deafaa2cfe977944cb72b035bb1c0b64332d43b44a2cb00b4ee34cb4d15e5` | 2279 |

The HCPB and WCLL metadata SHA-256 values are respectively `985975a5aaf0a327f2ff1155542beeec5f8ae3242ec6e04bc81d54b3d62d4ad7` and `6f2560949983f54e1d783a4f6a21845ffc6450ae3ca6b23e32a702b42231b88c` (225 bytes each). IAEA-NDS, *open-benchmarks*, commit `872543644e2353ab238242a27c2ce975eb935f32`, input bundles under CC-BY-4.0; exact input hashes are recorded in the generated external manifests.
