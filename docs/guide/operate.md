# Operate: the 30-year history

Press 3 to open the Operate step. It edits the operating assumptions and recalculates the 30-year history of every arrangement and, when present, of the allocation sweep. The history follows magnet and blanket exposure, replacement outages, tritium inventory and net electricity.

Start with the preset menu, then move the sliders and watch the timeline.

## Choose a preset

Hover a preset to see what it is and its key numbers.

| Preset | What it is |
| --- | --- |
| Demountable magnets | The default. The magnet envelope is replaceable at a literature REBCO screening value of 3 × 10²² n/m² of fast fluence (neutron energy above 0.1 MeV). |
| Loaded assumptions | The operating assumptions loaded with the study. |
| Authored baseline | The authored baseline operating scenario. |
| Permanent-trip test | A numerical control, not a plant scenario. It trips the magnets permanently to exercise the replacement and shutdown logic, and it ends with negative net electricity. It is marked as a control. |

## Ask what if

Under **What if…**, each slider edits one assumption. Each edit cancels any running calculation and recalculates after a short pause. A row shows an *edited* badge and a revert button once you move it away from the preset.

- **Magnet service limit**: 10²¹ to 10²³ n/m², logarithmic, with the literature value marked and a reset button.
- **Magnet replacement duration**: 14 to 365 days.
- **Blanket service limit**: 10²⁵ to 10²⁷ n/m².
- **Tritium recovery fraction**: 0 to 1.
- **Processing delay**: 0 to 7 days.
- **Thermal-to-electric efficiency**: 0 to 1.
- **Opening usable fuel / kg**.

A row appears only when the preset declares it. **Revert all to preset** undoes every edit. **Recalculate history** is a manual fallback.

All of these values are authored or literature screening values. They are not material allowables or measured plant data. Service limits, outage durations, recovery fractions and efficiencies stay conditional on what you set.

## Magnet limits per region

The magnet limit applies to fast fluence averaged over each of three regions:

- **Inboard**: the inboard half of the magnet.
- **Outboard**: the outboard half, outside the port sector.
- **Port sector**: the 20° sector of the outboard half behind the port.

Fluence is each region's fast flux times operating time. The first region to reach the limit triggers the replacement, and the event names it. Replacing the magnets resets the exposure of every region. One slider moves all the region limits together.

FARIS reports regional averages, not the local peak. A hot spot inside a region can reach the limit sooner than the average does. The port-sector figure is the one most likely to understate it.

## Read the timeline

The bottom panel plots one quantity for each arrangement. Choose it from:

- **Magnet fluence**, toward its service limit.
- **Electricity**, cumulative signed net electricity.
- **Tritium**, usable inventory.
- **Full-power years**, cumulative.

Click an arrangement chip to show or hide it. The one drawn in 3D is drawn thicker. Click or drag on the plot, or use the **calendar years** slider, to scrub through time. Scrubbing selects the computed snapshot at or before the requested time. Events are never interpolated across. In the *Accumulated component fluence* view, the 3D colours follow the year you scrub to.

**Jump to a calculated event…** lists every event: tritium imports, processed tritium released, operation started and stopped, fuel-limited stops, planned outages, service limits reached, and replacements starting and completing.

The headline row shows:

- **Year**.
- **State**: Operating, Magnet replacement, Component replacement, Planned outage, Fuel-limited or Stopped.
- **Usable tritium**, with the amount still in processing on hover.
- **Net electricity so far**.
- **Magnet swaps so far**.
- **Next magnet swap**.

Replacement outages and planned outages are authored in duration and timing. The planned outages are illustrative. They are not an availability estimate. Select a component in the Outliner to see its accumulated fluence, replacements completed, trigger and operating events.

## Tritium and net electricity

Usable tritium follows breeder production, D-T burn, imports, processing delay, process loss and decay. It is conditional on the authored recovery fraction and delay.

Net electricity is gross output minus auxiliary load under authored energy assumptions. It is unavailable when no total nuclear-heat response is bound to the transport run. The ledger is not a coolant or thermal-cycle calculation.

## Conditional sensitivity

The collapsed section **Conditional sensitivity** runs 27 full-history reruns. Recovery is 0.90, 0.95 and 0.99. Processing delay and the service limits are each ×0.5, ×1 and ×2. These are authored probes around the preset. They are not uncertainty bounds.

Next: [Uncertainty ensembles](uncertainty.md).
