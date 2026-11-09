# TIARA 68 MeV BC501A spectra

This adapter translates two pinned IAEA/JADE MCNP inputs directly to OpenMC:

- 40 cm iron shield, no additional collimator: `tiara-bc-fe-68-40`
- 100 cm concrete shield, no additional collimator: `tiara-bc-cc-68-100`

The manifest builder reads an external, clean checkout of
`IAEA-NDS/open-benchmarks` at `1d9855b00e05c5d5e25a11026ac56940dc75b516`.
It records the exact MCNP deck, measured CSV, and upstream licence hashes. The
source and measured tables are not copied into Git. IAEA's source repository
states CC-BY-4.0; cite IAEA-NDS and the TIARA experiment when reusing them.

The selected MCNP decks define the point source spectrum with SI1 H/SP1, the
physical cosine distribution with SI2/SP2, the MCNP sampling bias with SB2, an
F4 neutron flux tally in cell 109, its FM14 source-charge factor, and the E14
energy boundaries. The adapter normalizes the interval weights from SP1,
constructs OpenMC's histogram energy distribution, and checks that SP2 is the
isotropic cosine distribution. It omits SB2: the MCNP manual defines SB as a
sampling distribution distinct from SP, with particle weights compensating for
the bias. OpenMC therefore samples the physical distribution analogously; this
is expected to be much less efficient than the MCNP ADVANTG biasing.

The geometry translator supports only the card subset used by these two decks:
plane surfaces, axis aligned RCC cylinders, BOX macrobodies, region
intersections/unions/complements, and the listed cell cards. Unknown surface
types, missing cells, changed source/tally cards, and changed material IDs fail
closed. The source and detector point checks exercise OpenMC's actual geometry
locator. This is a checked card translation; it has not yet been compared with
MCNP transport and is not independent experimental validation.

The source strength uses the exact FM14 value from each deck (`1.31126e11`),
which is the rounded product `2.61 x 4.00e9 n sr-1 microC-1 x 12.56 sr`. The
OpenMC `flux` score is divided by `ln(E_hi/E_lo)` to match the published
per-unit-lethargy spectrum. The first 0–6 MeV bin, rows with no measured value,
and the table's 70 MeV 0/0 placeholder remain NOT_EVALUATED. The CSV `abs err`
is retained as the per-bin reference uncertainty; the source does not specify
its confidence meaning or energy covariance, so verdicts remain INCONCLUSIVE.

Before any transport run, every neutron HDF5 table in the selected library must
extend through the source's 72.5 MeV upper boundary. The runner refuses a short
table and does not extrapolate or truncate the source spectrum. A failure of
this preflight leaves the case NOT_EVALUATED.

## Provenance

- Kos and Kodeli, *MCNP modelling of the TIARA SINBAD shielding benchmark*,
  IAEA INDC(NDS)-0785 (2019), DOI [10.61092/iaea.ny5x-0pq9](https://doi.org/10.61092/iaea.ny5x-0pq9).
- OECD-NEA, [TIARA 43/68 MeV Proton Benchmark](https://www.oecd-nea.org/science/wprs/shielding/sinbad/tiara/tia-abs.htm),
  including source peak-flux and beam-charge normalization notes.
- MCNP6.2 manual, [LA-UR-24-24602 Rev. 1](https://mcnp.lanl.gov/pdf_files/TechReport_2024_LANL_LA-UR-24-24602Rev.1_KuleszaAdamsEtAl.pdf),
  section 5.8.4: SB describes biased sampling, while SP defines the physical
  distribution and particle weights compensate the sampling bias.

## Use

```sh
python3 validation/cases/tiara-bc-68mev/build_manifest.py --upstream /path/to/open-benchmarks
python3 validation/cases/tiara-bc-68mev/run_case.py plan --case tiara-bc-fe-68-40
python3 validation/cases/tiara-bc-68mev/run_case.py build --case tiara-bc-fe-68-40 --work-dir /tmp/tiara-fe-build
```

Run `build` with the project's pinned OpenMC 0.15.3 Python environment. The
`run` subcommand also requires a cross-section XML path and
`--i-have-the-cpu`; it checks source-energy coverage before transport. Build
and run outputs belong in a local work directory and are not committed.
