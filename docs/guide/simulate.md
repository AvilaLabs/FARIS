# Simulate: transport results

Press 2 to open the Simulate step.

## Read the transport card

For the selected arrangement, the transport card shows:

- Tritium breeding: H3 atoms per source neutron.
- Magnet-region mean flux.
- Total nuclear heating.
- The number of histories.

Each value carries its Monte Carlo standard error. The recorded runs use 10 million histories for the port cases and the sweep, and 30 million for the port-free controls. [Reading the numbers](results.md) explains the standard errors and the badges.

The transport result is labelled "cold-data surrogate · NOT_EVALUATED". Scientific qualification is not evaluated, even for completed transport, because the materials are surrogates and the data are cold. Hover the label for the reason, and read [Scope and limits](scope.md) for what would settle it.

## Colour the 3D model

The field-view menu in the viewport controls colours the model:

| View | What it shows |
| --- | --- |
| Materials | The material of each component. |
| Mean component flux | The flux averaged over each component. |
| Spatial neutron flux | The calculated flux on a plane around the port. Move the **Y slice** slider to change the plane. |
| Nuclear heat deposition | Where the transport deposits heat. |
| Accumulated component fluence | Fluence built up over operation. It follows the timeline year. |

Flux colours use a fixed logarithmic scale, so arrangements are directly comparable. Each displayed value is a volume average over a mesh bin, including void. The mesh does not resolve point peaks. Gray bins have no samples. Desaturated bins have more than 30 % relative standard error. Neither treatment gives a zero-flux bound or a total uncertainty.

Transport fields use the recorded source strength and stay fixed during outages. Component colouring shows region averages, not a field inside a component.

Open **Reference-source energy-group spectra** to see the recorded neutron and photon flux spectra. They are flux spectra, not deposited-energy spectra.

## Run new transport

**Run and review transport** can run new OpenMC transport. It needs your own OpenMC and data, set under **Transport configuration**. The recorded study does not need it. See [Run your own transport](transport.md).

Next: [Operate: the 30-year history](operate.md).
