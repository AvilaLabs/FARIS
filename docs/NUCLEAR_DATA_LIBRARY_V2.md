<!-- kind: reference -->
# Nuclear-data library v2

FARIS has two local, Git-ignored nuclear-data libraries. The first,
`data/raw/combined-fendl32-endfbvii1/`, holds the 22 FENDL-3.2 neutron files and
eight ENDF/B-VII.1 photon elements that the authored reference cases need. Recorded
results depend on its identity and it is never modified. The second,
`data/raw/fendl32-endfbvii1-v2/`, covers the fusion materials a design is likely to
use. No default path points at v2 yet; a run uses it only when given its
`cross_sections.xml` and a matching audit. FARIS records which library a run used,
through the audit digest and the `cross_sections.xml` digest in the run record.

## What v2 contains

| Part | Content | Source |
| --- | --- | --- |
| Neutron | every neutron nuclide of the full FENDL-3.2 HDF5 directory, plus the gap-filled entries below | FENDL-3.2; gap fill from ENDF/B-VIII.1 |
| Photon | one file for every element that appears in a neutron nuclide | ENDF/B-VII.1 photoatomic and atomic relaxation, converted with OpenMC 0.15.3 |
| Thermal scattering | 20 S(alpha,beta) tables: H in H2O, D and O in D2O, Be metal, Be and O in BeO, Be, F and Li in FLiBe, graphite, H and Li in 7LiH, D and Li in 7LiD, Ca and H in CaH2, H and Zr in ZrH2, H in polyethylene | ENDF/B-VIII.1 |

The whole root is about 9.4 GB: the thermal tables are large (ENDF/B-VIII.1 stores up to 94
temperatures per table; `c_H_in_H2O.h5` alone is 2.6 GB), against about 0.9 GB for the neutron files.
Neutron files are byte-for-byte copies (not links), checked by SHA-256 at build time.
Thermal tables apply only when a material declares them. They are ENDF/B-VIII.1
evaluations used with FENDL-3.2 neutron data. Combining a thermal library with a
different neutron evaluation is standard practice, but it is a stated assumption.
ENDF/B-VIII.1 has no table for TiH2, for natural-lithium hydride (only 7LiH and 7LiD)
or for oxygen in liquid water; `references/nuclear-data-library-v2-provenance.json`
lists these.

Gap fill: a nuclide of the audit set `fusion-materials-v1` that FENDL-3.2 lacks comes
from ENDF/B-VIII.1 and is listed by nuclide with its source library. The provenance
record holds the list and any remaining gap with its reason.

## Differences from the first library

- All FENDL-3.2 nuclides instead of 22; photon data for every element of those nuclides
  instead of eight.
- Thermal-scattering tables are present (the first library has none).
- Files are copied, not hard-linked.
- The audit set `fusion-materials-v1` (`integrations/openmc/audit_library.py
  --nuclide-set fusion-materials-v1`) covers the natural isotopes of the elements in
  SS316L, Inconel 718, EUROFER97, CuCrZr, tungsten and WC, V-4Cr-4Ti, TZM, ordinary
  Portland concrete, helium, air, light and heavy water, FLiBe, PbLi, B4C, TiH2, ZrH2,
  Li4SiO4, Li2TiO3, beryllium, Be12Ti, REBCO tape, Nb3Sn, NbTi, copper and
  glass-fibre/epoxy insulation. The audit output carries the element lists and their
  sources. They are presence lists, not certified compositions.

## Rebuild

Use the OpenMC 0.15.3 environment, and keep every source outside version control. The
build reads local copies only and downloads nothing. The two ENDF/B-VII.1 archives are
checked against the NNDC MD5 values before conversion (see
[the first library's acquisition](PHOTON_LIBRARY_ACQUISITION.md)).

```sh
OPENMC_PY=/path/to/openmc-0.15.3/bin/python
"$OPENMC_PY" scripts/build_nuclear_data_v2.py \
  --fendl-dir /path/to/fendl-3.2-hdf5 \
  --endfb-viii-dir /path/to/endfb-viii.1-hdf5 \
  --photoatomic-zip data/raw/acquisition/ENDF-B-VII.1-photoat.zip \
  --atomic-relaxation-zip data/raw/acquisition/ENDF-B-VII.1-atomic_relax.zip \
  --first-provenance references/photon-library-provenance.json \
  --output-dir data/raw/fendl32-endfbvii1-v2 \
  --provenance-out references/nuclear-data-library-v2-provenance.json
"$OPENMC_PY" integrations/openmc/audit_library.py \
  --cross-sections data/raw/fendl32-endfbvii1-v2/cross_sections.xml \
  --nuclide-set fusion-materials-v1 \
  --output references/openmc-library-audit-v2.json
```

The build is deterministic and refuses to replace an existing root without `--force`;
`--force` replaces only a directory that carries the v2 marker file. The photon
conversion is repeated rather than reused; the provenance record states, for each
element the first library also has, whether the converted bytes match the first
library's file.

## What this does and does not establish

Established: the identity of every file (SHA-256 and size), the NNDC MD5 of the two
photon archives, OpenMC readability of every listed file (the audit), and the source
library of every entry. Not established: redistribution rights (FENDL-3.2 falls under
IAEA NDS terms, ENDF/B under NNDC; the data stay local), authentication of the FENDL
and ENDF/B-VIII.1 acquisitions, and the suitability of any evaluation for a particular
calculation. The audit reads thermal tables above 1 GiB at a single temperature (294 K) through the OpenMC reader,
because loading all temperatures needs several GB of memory; the audit record states the scope per table.
The audit does not qualify the nuclear data or any transport result.
