# Nuclear data

Transport needs nuclear data that FARIS does not ship: the files are large and their redistribution terms are not
settled. You assemble a library once, from public sources, and FARIS records which library every run used, by the
digests of its `cross_sections.xml` and its audit.

## The two libraries

| Library | Content | Size | Used by |
| --- | --- | --- | --- |
| First (`combined-fendl32-endfbvii1`) | 22 FENDL-3.2 neutron nuclides and eight ENDF/B-VII.1 photon elements (H, Li, Be, F, Ti, Fe, Cu, W) | small | the recorded study; never modified |
| v2 (`fendl32-endfbvii1-v2`) | every FENDL-3.2 neutron nuclide (193, one gap-filled from ENDF/B-VIII.1), photon data for all 61 elements in them, 20 ENDF/B-VIII.1 thermal-scattering tables | about 9.4 GB | your own designs; not yet the default |

v2 covers the natural isotopes of every element in the common fusion materials: steels (SS316L, EUROFER97), Inconel
718, CuCrZr, tungsten and WC, V-4Cr-4Ti, TZM, concrete, helium, air, water, FLiBe, PbLi, B4C, the hydrides, the lithium
ceramics, beryllium and Be12Ti, REBCO tape, Nb3Sn, NbTi, copper and epoxy insulation. Its thermal tables are H in H2O,
D and O in D2O, Be metal, Be and O in BeO, Be, F and Li in FLiBe, graphite, H and Li in 7LiH, D and Li in 7LiD, Ca and H
in CaH2, H and Zr in ZrH2, and H in polyethylene. ENDF/B-VIII.1 has no table for TiH2, natural-lithium hydride or oxygen
in liquid water.

Things to know about v2:

- Thermal tables apply only when a material declares them. They are ENDF/B-VIII.1 evaluations used with FENDL-3.2
  neutron data; combining libraries this way is standard practice, and it is a stated assumption.
- Most of the 9.4 GB is the thermal tables, which store up to 94 temperatures each; the H in H2O table alone is 2.6 GB.
  The neutron files are about 0.9 GB.

## Sources

- FENDL-3.2 neutron data in HDF5, from the [OpenMC data-library page](https://openmc.org/data/).
- ENDF/B-VIII.1 in HDF5 (thermal tables and gap fill), OpenMC's official build, from the same page.
- ENDF/B-VII.1 photoatomic and atomic-relaxation sublibraries, from the
  [NNDC download page](https://www.nndc.bnl.gov/endf-b7.1/download.html), which publishes their MD5 checksums. FARIS
  converts them to HDF5 with OpenMC 0.15.3 and checks the MD5 values before converting.

## Build v2

Download the sources above into a folder outside the FARIS source tree, then run, with the OpenMC 0.15.3 Python
environment:

```sh
OPENMC_PY=/path/to/openmc-0.15.3/bin/python
"$OPENMC_PY" scripts/build_nuclear_data_v2.py \
  --fendl-dir /path/to/fendl-3.2-hdf5 \
  --endfb-viii-dir /path/to/endfb-viii.1-hdf5 \
  --photoatomic-zip /path/to/ENDF-B-VII.1-photoat.zip \
  --atomic-relaxation-zip /path/to/ENDF-B-VII.1-atomic_relax.zip \
  --first-provenance references/photon-library-provenance.json \
  --output-dir data/raw/fendl32-endfbvii1-v2 \
  --provenance-out my-v2-provenance.json
"$OPENMC_PY" integrations/openmc/audit_library.py \
  --cross-sections data/raw/fendl32-endfbvii1-v2/cross_sections.xml \
  --nuclide-set fusion-materials-v1 \
  --output my-v2-audit.json
```

The build downloads nothing and is deterministic. It refuses to replace an existing library unless given `--force`, and
`--force` replaces only a folder it built itself. The audit confirms that OpenMC can read every file the material set
needs; give its output to a run together with the library's `cross_sections.xml`.

## What the audit does not establish

The audit records file identity and readability. It does not qualify the nuclear data for a particular calculation, and
it does not authenticate the FENDL-3.2 and ENDF/B-VIII.1 downloads; only the two ENDF/B-VII.1 archives are checked
against published checksums. Thermal tables larger than 1 GiB are read at 294 K only, because loading every
temperature needs several GB of memory; the audit record states the scope for each table.
