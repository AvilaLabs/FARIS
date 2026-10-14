<!-- kind: reference -->
# Model pages

One row for each model FARIS runs today: where it is described, what that description holds, and where its
validation status is. This is the index of the theory manual (DOC-003) and of the per-model pages (DOC-024). Neither
requirement is met: no page below has numbered equations, a stated validity range or a link from the code to the
equation, and the models of the requirement files that FARIS does not implement yet have no page at all.

Last checked: 2026-10-14.

Validation status for every model is in [Validation](VALIDATION.md) and the [scientific baseline](SCIENTIFIC_BASELINE.md).
The recorded campaign does not reproduce an experimental benchmark; [Known limitations](guide/limitations.md) lists the gaps row by row.

| Model | Pages | Holds | Missing |
| --- | --- | --- | --- |
| Fixed-source neutron and photon transport (OpenMC adapter) | [Transport boundary](TRANSPORT.md), [Cold reference](COLD_REFERENCE.md), [Method](guide/method.md), [Run your own transport](guide/transport.md) | Inputs, outputs, normalisation, units, assumptions, limits | Equations; validity range; benchmark C/E |
| Independent numerical controls | [Numerical controls](NUMERICAL_CONTROLS.md), [Lithium capture control](LITHIUM_CAPTURE_CONTROL.md) | What each control checks and its tolerance | Validity range |
| Reference comparison, ITER 1D | [ITER 1D reference](ITER_1D_REFERENCE.md) | Why the comparison is unavailable | The comparison itself |
| Nuclear data libraries | [Nuclear data](guide/nuclear-data.md), [Library v2](NUCLEAR_DATA_LIBRARY_V2.md), [Photon library](PHOTON_LIBRARY_ACQUISITION.md) | Sources, versions, checks, assembly steps | Uncertainty data |
| Material baselines | [Material baseline](MATERIAL_BASELINE.md) | Composition, density, source of each value | Validity range; values at other temperatures |
| Operating history (fuel, decay, processing, exposure, replacement, energy) | [Operating history](OPERATING_HISTORY.md), [Operate](guide/operate.md) | State, update order, inputs, conservation checks | Equations as numbered formulae |
| Computed maintenance durations | [Maintenance](guide/maintenance.md), [design notes](notes/MAINTENANCE_0.2_DESIGN.md), [coupling validation](notes/MAINTENANCE_COUPLING_VALIDATION.md) | Method, thresholds, iteration, validation runs | Equations; independent benchmark |
| Uncertainty ensembles | [Uncertainty](guide/uncertainty.md), [Method](guide/method.md) | Sampling from the batch covariance, outputs | Nuclear-data and model uncertainty (not modelled) |
| Variance reduction (weight windows) | [Run your own transport](guide/transport.md), [CAD transport notes](notes/CAD_TRANSPORT_S0F.md) | Method, cache, measured gains | Validity range |
| Geometry checks | [Design file](guide/design-file.md), [CAD integrity checks](notes/CAD_INTEGRITY_CHECKS.md) | What each check finds and its limit | Equations behind the tolerances |

## Models with no page

The requirement tables name models that FARIS does not implement and so cannot document: thermal-hydraulic loops
(THM), magnet nuclear heating and quench margins (MAG), the itemised plant power balance (PWR), tritium permeation
(FUEL), shutdown dose and worker dose (OPS-080, OPS-081), reliability and availability by event simulation (OPS-040),
and nuclear-data uncertainty (UNC-020). Each gets a page when the model exists; a page for a model that does not
run would only restate a requirement.
