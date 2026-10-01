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
