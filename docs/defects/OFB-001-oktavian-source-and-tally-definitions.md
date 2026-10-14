# OFB-001: the OpenMC-Fusion-Benchmarks OKTAVIAN source is not isotropic and its energy weights are read as densities; the CAD tally names a surface id that is not the leakage surface

Status: drafted, not reported. Verification and significance: see [README.md](README.md). Found 2026-10-14 while building
`oktavian-al-cad`. Draft for an issue against `eepeterson/openmc_fusion_benchmarks`. Nobody has been contacted.

## Versions

| | Result |
| --- | --- |
| `openmc_fusion_benchmarks` `develop` at 653d6aba04d993d2915a4e4e983e2a2119fa0e27 (2026-04-08), `benchmark.py` | findings 1 and 2: reproduced by running OFB's own `_build_source` on its own `specifications.yaml` (OpenMC 0.15.3; the package imports were stubbed because pydagmc and PyYAML are not installed in that environment, the function body was not changed) |
| the same commit with `openmc_fusion_benchmarks-lfs` `dace719d46c76c3b9f24a98ebf9f85ce1b8092a7` | finding 3: reproduced with OFB's own meshing call, not through OFB's post-processing (not installed); finding 4: read from the STEP file |

## 1. The source is emitted in a thin ring, not isotropically

`specifications.yaml` gives the source angle bins as `[0.0, 360.0]` degrees. `_build_source` computes
`abins = np.cos(angles['angle']['bins'])`, which takes the cosine of 0 and of 360 as radians (1.0 and -0.2837), and then
passes each value through `angular_conversion(..., 'degrees')`, which multiplies by pi/180. The polar cosine
`openmc.stats.Uniform(a, b)` therefore has `a = 0.017453` and `b = -0.004951`. Run on the specification:

```
mu limits 0.017453292519943295 -0.004951343605016356 reference [0. 1. 0.]
sampled mu min/max -0.004951125931813696 0.0174530601977088
```

Directions lie within about 1 degree of the plane perpendicular to the reference direction (0, 1, 0): a ring in the x-z
plane. The MCNP source of the same benchmark (`sdef pos=0 0 0 cel=1 erg=d1`) is isotropic. The beam channel of the
sphere is along +x, so a ring that contains +x sends about 19 % of the neutrons into the channel's half-angle of 34
degrees where an isotropic source sends 8 %. The function also returns the last `IndependentSource` it built, not the
list it collects, and sets `source.paricle` (a typo, so the particle type is never set; the default is neutron).

## 2. The energy weights are probabilities but are used as densities

The `weights` row has one entry per energy edge (52, the last 0.0). They equal the MCNP `sp1` entries of the IAEA input
one for one (`weights[i] == sp1[i+1]`, maximum difference 0): the probability of the interval `[edges[i], edges[i+1])`.
`_build_source` passes them as `p` to `openmc.stats.Tabular(..., interpolation='histogram')`, whose `p` is a density, so
OpenMC weights each interval by its width. The edges are not equal in width (0.036 MeV at the bottom, 1.4 to 2.0 MeV at
the top). Run on the specification:

| | mean source energy | share of the 13.574 to 15.002 MeV interval |
| --- | --- | --- |
| as OFB builds it | 14.637 MeV | 63.1 % |
| weights read as probabilities (as the MCNP input) | 14.031 MeV | 62.0 % |

## 3. The CAD tally names surface 7

The tallies in `specifications.yaml` use `surface` filter `values: [7]`, and `_openmc_to_ofb` divides the tally by
`msh.surfaces_by_id[7].area`. In the CSG model of the same benchmark (the IAEA OpenMC input) surface 7 is the r = 100 cm
sphere around everything. A DAGMC surface id is whatever the mesher assigns. Converting `oktavian_a.step` with OFB's own
call (`CadToDagmc.add_stp_file(scale_factor=.1)`, `export_dagmc_h5m_file(imprint=True, min_mesh_size=0.1, max_mesh_size=10,
set_size={1: 5, 2: 5, 3: 0.5, 4: 0.5, 5: 2})`, cad_to_dagmc 0.14.3) and reading the result gives 18 surfaces; surface 6 is
the r = 100 cm sphere (125,604 cm2), and surface 7 is a 3,543 cm2 patch of the r = 19.95 cm sphere, one of five patches
the beam channel splits that sphere into (7, 8, 9 border the outer steel shell; 4 and 5 are the openings of the channel).
If the numbering of OFB's own run is the same, the CAD tally sees about 70 % of the leakage surface and divides it by
the patch area. The stored experiment is the IAEA table divided by 4 pi (19.5 cm)^2 = 4,778.36 cm2 (every bin, to
1e-13), a radius that is neither the sample's outer radius (19.95 cm) nor the shell radius (19.75 cm).
Not confirmed: OFB's post-processing was not run (it needs pydagmc), and the numbering depends on the mesher version.

## 4. `oktavian_al.step` holds overlapping solids

`lfs/benchmarks/oktavian_al/oktavian_al.step` (object sha256 ce30dad1...) reads as five solids, three of which have volumes
close to those of full spheres, not shells: 0.0045, 0.0312 and 0.0332 m3, against 0.0044, 0.0323 and 0.0333 m3 for full
spheres of r = 10.2, 19.75 and 19.95 cm (the beam channel changes each a little). The specification uses `oktavian/oktavian_a.step`, whose five solids are the shells
(0.00031, 0.00097 and 0.0268 m3 for the two steel shells and the aluminium) and do not overlap. The unused file is a
trap for anyone who builds from the folder name.

## Significance

Silent and large where it applies. To measure it, the same DAGMC model (FARIS conversion, FENDL-3.2 neutron data with
ENDF/B-VII.1 photon data, OpenMC 0.15.3, 3,000,000 histories each, tally on the whole outer boundary) was run once with
the source OFB's own `_build_source` returns for its specification (findings 1 and 2 together) and once with the
isotropic, probability-weighted source. Leakage per source neutron, OFB-as-built over the correct source:

| Energy range | Ratio |
| --- | --- |
| 0.097 to 1 MeV | 0.741 |
| 1 to 10 MeV | 0.810 |
| 10 to 13.3 MeV | 0.923 |
| 13.3 to 15.0 MeV | 1.049 |
| 15.0 to 20.7 MeV | 1.166 |
| 0.097 to 20.7 MeV in total | 1.003 |

The total hardly changes and the spectrum shape does, so a C/E taken per bin is wrong by 20 to 25 % at low energy and 17 %
above 15 MeV while an integral check would look fine. Findings 1 and 2 were not separated. This measures the source
definition on FARIS's model, not OFB's pipeline end to end; it applies only to a user who builds the OKTAVIAN model with
OFB's package. The stored results database predates the CAD route (its runs are CSG), so published OFB C/E values are not
affected.

## What FARIS did

`oktavian-al-cad` builds the source itself (isotropic, interval probabilities divided by the interval width), tallies the
whole outer boundary, divides by no area, and records the deviations in its protocol.

## Reproducer for 1 and 2

Run in an OpenMC 0.15.3 Python with OFB's `src` on the path and the package imports stubbed; `spec.json` is the
specification converted to JSON.

```python
o = object.__new__(OpenmcBenchmark)
o._benchmark_spec = json.load(open("spec.json"))
s = o._build_source()
print(s.angle.mu.a, s.angle.mu.b)
x, p = np.array(s.energy.x), np.array(s.energy.p)
eff = p[:-1] * np.diff(x); eff /= eff.sum()
print((eff * (x[:-1] + x[1:]) / 2).sum() / 1e6)
```

## Verification, 2026-10-14

- Findings 1 and 2 re-checked by reading `_build_source` at 653d6ab and on every branch of SteSeg's fork
  (including `develop_2.0`): the code is unchanged everywhere. Running the function on the `oktavian_al` spec gives
  mu from -0.00495 to 0.01745 and a mean source energy of 14.637 MeV (14.031 MeV with the weights read as bin
  probabilities).
- Independent spectrum run, seed 7, 3M histories each, `oktavian_a` CAD model, OpenMC 0.15.3; OFB source divided
  by the protocol source: 0.1-1 MeV 0.723 +/- 0.017, 1-10 MeV 0.807 +/- 0.012, 10-15 MeV 1.043 +/- 0.008,
  > 15 MeV 1.164 +/- 0.015, total 1.003 +/- 0.006. Agrees with the first run.
- Two further code points in the same function: `return source` instead of `return sources`, and the typo
  `source.paricle`.
- Finding 3 (tally surface number) stays unconfirmed and is left out of the report.
- Report target: eepeterson/openmc_fusion_benchmarks (issues are disabled on the fork). Draft opened for Connor
  to read and send; not posted by FARIS.
