# Photon data acquisition and local assembly

FARIS uses photoatomic interaction and atomic-relaxation records from the
NNDC-hosted ENDF/B-VII.1 photoatomic and atomic-relaxation sublibraries. OpenMC
0.15.3 converted the evaluated ENDF records into HDF5 with
`openmc.data.IncidentPhoton.from_endf(photoatomic, relaxation)` and
`export_to_hdf5`. The combined `cross_sections.xml` combines those eight
photon-element files (H, Li, Be, F, Ti, Fe, Cu, W) with the unchanged, audited
FENDL-3.2 neutron files needed by the authored reference cases.

The complete local, Git-ignored data root is
`data/raw/combined-fendl32-endfbvii1/`. Its `cross_sections.xml` uses paths
relative to that root. `references/photon-library-provenance.json` records
archive URLs, official NNDC MD5 checks, observed SHA-256 values, per-HDF hashes
and sizes, conversion details, file coverage, and the combined XML digest. The
HDF, ENDF, and ZIP files are not part of the source repository.

Primary references:

- [OpenMC official data libraries](https://openmc.org/data/)
- [OpenMC photon cross-section conversion guide](https://docs.openmc.org/en/latest/usersguide/data.html)
- [NNDC ENDF/B-VII.1 download page and sublibrary checksums](https://www.nndc.bnl.gov/endf-b7.1/download.html)
- [NNDC photoatomic sublibrary archive](https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-photoat.zip)
- [NNDC atomic-relaxation sublibrary archive](https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-atomic_relax.zip)

The NNDC download page and archives provide no explicit license statement.
The local data are retained for this workspace only; redistribution rights
remain unresolved. Preserve ENDF/B-VII.1 and NNDC attribution and obtain rights
clarification before redistribution.

## Rebuild the local overlay

From the repository root, use the pinned OpenMC 0.15.3 Python environment.
The NNDC MD5 values are checked by the converter before any data is converted.
The FENDL archive URL below is the OpenMC-published archive, but the OpenMC
data page does not publish a checksum for that archive. The locally audited
FENDL HDF5 digests pin exact bytes; they do not authenticate the archive
acquisition. This procedure requires the archive and FENDL HDF files to stay
outside version control.

```sh
mkdir -p data/raw/acquisition data/raw/fendl32-hdf
curl -fL https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-photoat.zip \
  -o data/raw/acquisition/ENDF-B-VII.1-photoat.zip
curl -fL https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-atomic_relax.zip \
  -o data/raw/acquisition/ENDF-B-VII.1-atomic_relax.zip
printf '%s  %s\n' \
  5192f94e61f0b385cf536f448ffab4a4 data/raw/acquisition/ENDF-B-VII.1-photoat.zip \
  fddb6035e7f2b6931e51a58fc754bd10 data/raw/acquisition/ENDF-B-VII.1-atomic_relax.zip \
  | md5sum --check
```

Obtain FENDL-3.2 neutron HDF5 from the [OpenMC data-library page](https://openmc.org/data/)
(the linked FENDL archive is `https://anl.box.com/shared/static/3cb7jetw7tmxaw6nvn77x6c578jnm2ey.xz`). Extract it under `data/raw/fendl32-hdf/` while preserving its `cross_sections.xml` and relative paths. Then convert the two photon archives and assemble a confined, combined data root:

```sh
OPENMC_PY=/path/to/openmc-0.15.3/bin/python
"$OPENMC_PY" integrations/openmc/convert_endfbvii1_photon.py \
  --photoatomic-zip data/raw/acquisition/ENDF-B-VII.1-photoat.zip \
  --atomic-relaxation-zip data/raw/acquisition/ENDF-B-VII.1-atomic_relax.zip \
  --output-dir data/raw/endfbvii1-photon-converted
"$OPENMC_PY" integrations/openmc/assemble_fendl_photon_overlay.py \
  --neutron-cross-sections data/raw/fendl32-hdf/cross_sections.xml \
  --audit references/openmc-library-audit.json \
  --photon-conversion data/raw/endfbvii1-photon-converted \
  --output-dir data/raw/combined-fendl32-endfbvii1
```

The assembler accepts only FENDL neutron files whose bytes match the current
audited inventory, copies them into a new root with the converted photon HDF5,
and writes `provenance.json`. It never writes into an existing output directory.
For a fresh all-material audit, provide the nuclide and photon-element lists
from the relevant case set explicitly; the repository audit is intentionally
scoped to its current selected inventory. For example, replace the abbreviated
lists below with the exact authored case inventory before using the audit for a
run:

```sh
"$OPENMC_PY" integrations/openmc/audit_library.py \
  --cross-sections data/raw/combined-fendl32-endfbvii1/cross_sections.xml \
  --nuclides Li6 Li7 H1 H2 Be9 F19 Ti46 Ti47 Ti48 Ti49 Ti50 \
             Fe54 Fe56 Fe57 Fe58 Cu63 Cu65 W180 W182 W183 W184 W186 \
  --photon-elements H Li Be F Ti Fe Cu W \
  --output data/raw/combined-fendl32-endfbvii1/openmc-library-audit.json
```

Do not replace the checked-in reference metadata with a locally regenerated
audit without reviewing the file identities and scientific scope. A successful
conversion or audit establishes file readability and declared capabilities;
it does not establish evaluation suitability or qualify transport results.
