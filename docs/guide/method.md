# Method

This chapter summarises what FARIS computes. The repository documents each part in full. It links to them at the end.

## Transport

Each arrangement is a full-torus model of concentric layers, solved with OpenMC fixed-source Monte Carlo transport, with neutrons and photons coupled. The recorded runs use OpenMC 0.15.3, FENDL-3.2 neutron data and ENDF/B-VII.1 photon data.

The source is uniform in the plasma volume, isotropic and monoenergetic at 14.1 MeV. One source neutron stands for one D-T reaction of 17.6 MeV. At 525 MW that is about 1.86 × 10²⁰ source neutrons per second. OpenMC reports each score per source neutron with its standard error. FARIS then multiplies by the source rate and divides by volumes to get physical rates and densities.

The materials are authored cold-data surrogates: tungsten for the first wall, solid Li₂BeF₄ with 90 atom per cent lithium-6 for the blanket, titanium hydride for the shield, iron as a structural stand-in and copper as the magnet stand-in. [Scope and limits](scope.md) says what that means for the results.

The responses are:

- Tritium production, in H3 atoms per source neutron.
- Heating, from coupled neutron and photon transport.
- Flux by component, with neutron and photon spectra.
- A 3D flux map on a mesh around the outboard side.
- Fast-neutron flux (neutron energy above 0.1 MeV) in three regions of the magnet: the inboard half, the outboard half outside the port sector, and the port sector, the 20° sector behind the port.

## Covariance from batches

Each run records the per-batch value of every scalar response. FARIS takes the sample covariance of those batch values, divided by the number of batches, as the covariance between the response means. It first checks that the batch values reproduce OpenMC's own means and standard errors. Correlated quantities, such as tritium production and heating, can then move together when sampled.

## The operating history

The history is a deterministic ledger driven by one transport result. It is conditional on the authored operating assumptions and is not a prediction.

- **Tritium.** Opening stock, breeder production, D-T burn, imports, processing delay, process loss and radioactive decay go into one site balance. The balance is checked at every recorded state.
- **Exposure.** A component accumulates exposure only while the source operates: its flux times operating time. A service limit names a component, a response, a threshold and its source. When a replaceable component reaches a limit, a replacement outage starts and that component's exposure resets. A permanent limit stops operation.
- **Fuel.** The source stops when usable tritium falls to a reserve and restarts at a higher threshold. This is a control rule, not a plant control system.
- **Energy.** Alpha heating and recovered transport heat go through an authored thermal-to-electric efficiency. Auxiliary load is subtracted, and net electricity is signed, so it can be negative.

[Operate](operate.md) lists the assumptions you can change.

## Ensembles

An ensemble draws the driving rates from a multivariate normal distribution with the recorded means and covariance, and runs the history once for each draw. Each sample has its own random stream derived from a seed, so results do not depend on the thread count. Draws with impossible rates are rejected, and if more than 1 per cent of accepted samples are rejected, the ensemble is not evaluated.

Each output is summarised by its median, its 5 to 95 per cent range, and distribution-free confidence intervals on those quantiles. Discrete outcomes get shares with Wilson intervals. A paired comparison subtracts sample *i* of one arrangement from sample *i* of another, which is valid only for independent transport runs. [Uncertainty ensembles](uncertainty.md) describes how to read them.

## Controls

Independent Decimal-arithmetic checks cover the half-life decay, the source-rate conversion, the mass balance and the heat conversion, and numerical refinement checks cover the time step. They show the ledger arithmetic is repeatable. They do not bound transport statistics, nuclear data or model error.

## Read the details

- [Operating history](https://github.com/AvilaLabs/FARIS/blob/main/docs/OPERATING_HISTORY.md): the ledger, its controls and the ensemble method.
- [Transport boundary](https://github.com/AvilaLabs/FARIS/blob/main/docs/TRANSPORT.md): normalization, regions and the covariance method.
- [Cold-data reference](https://github.com/AvilaLabs/FARIS/blob/main/docs/COLD_REFERENCE.md): materials, source and recorded tallies.

Next: [Scope and limits](scope.md).
