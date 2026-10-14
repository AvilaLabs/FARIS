<!-- kind: reference -->
# Reference geometries

Three reference sets ship with, or are checked by, FARIS for validation and
teaching. Each entry gives its citation, what it is for and, under **Omits**,
what it leaves out. None is a design basis. A test in `faris-model` reads this
page and fails if an entry loses its citation or its omission line (GEO-009).

## ARC-inspired concentric torus

- **Files:** `scenarios/arc-inspired/` (the scenario, cold-reference, ported and
  allocation-sweep files).
- **Citations:** Sorbom et al., ARC conceptual design,
  [arXiv:1409.3540](https://arxiv.org/abs/1409.3540) (concept inspiration only);
  Conceptual design study for heat exhaust management in ARC,
  [arXiv:1809.10555](https://arxiv.org/abs/1809.10555) (3.3 m major radius,
  525 MW fusion power). Every other dimension is an authored demo assumption.
- **Use:** comparing blanket and shield allocations in a fixed radial envelope;
  the cold-data OpenMC reference in [COLD_REFERENCE.md](COLD_REFERENCE.md).
- **Omits:** ARC ribs and support columns, coolant channels, the divertor, coil
  packs and windings, plasma-facing heat transfer, and every port except the one
  authored rectangular bore in the ported sibling. The torus is circular and
  concentric; the plasma source is uniform and isotropic, not ARC source phase
  space. FLiBe is treated as a homogeneous solid at the declared cold state.
  The ARC tritium breeding ratio of 1.1 or more does not transfer to this
  geometry or library. See [MATERIAL_BASELINE.md](MATERIAL_BASELINE.md).

## ITER-class one-dimensional radial build (IAEA ITER_1D)

- **Files:** external, not vendored. The IAEA-NDS ITER_1D OpenMC input, from
  [IAEA-NDS/open-benchmarks](https://github.com/IAEA-NDS/open-benchmarks)
  (CC BY 4.0). Identity: `references/iter-1d-reference.json`.
- **Citation:** IAEA Nuclear Data Section,
  [open benchmarks repository](https://github.com/IAEA-NDS/open-benchmarks),
  ITER_1D.
- **Use:** a code-to-code test and adapter exercise
  ([ITER_1D_REFERENCE.md](ITER_1D_REFERENCE.md)). Not an experimental comparison.
- **Omits:** all toroidal curvature and poloidal variation (the model is
  concentric z-cylinder annuli between reflective planes at plus and minus
  1000 cm), ports, penetrations and any three-dimensional feature. The
  benchmark bundle has no OpenMC settings or tally definitions; FARIS adds the
  tallies and the 293.6 K nearest-temperature setting. Its custom source library
  is never loaded; the source law is mirrored through the OpenMC API with a
  different random stream. The numerical comparison stays `NOT_EVALUATED`
  until a traceable reference response exists.

## Synthetic analytic shape (one-group pure-absorber sphere)

- **Files:** `controls/pure_absorber_sphere.py`; described in
  [NUMERICAL_CONTROLS.md](NUMERICAL_CONTROLS.md).
- **Citation:** no outside source; the closed-form result (escape probability
  exp(-Sigma_a R) for a point source at the centre) is derived in
  [NUMERICAL_CONTROLS.md](NUMERICAL_CONTROLS.md).
- **Use:** checking solver, source and normalization arithmetic against an
  exact answer.
- **Omits:** scattering, energy dependence, any real material or nuclear data,
  and any reactor geometry. Its cross section and radius are chosen for the
  mathematics, not for a material, and must not be used as a benchmark of a
  reactor.
