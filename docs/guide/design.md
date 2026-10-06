# Design: the plant in 3D

Press 1 to open the Design step.

## The model

The viewport shows the plant as concentric shells from the first wall to the magnets. In FARIS the 3D view is the transport model: what you see is what the transport calculation used. The geometry is idealised. It is a full torus of concentric layers with one outboard port, not the published ARC design. [Scope and limits](scope.md) says what that leaves out.

The plant has a major radius of 3.3 m and a radial build of 1.20 m. Fusion power is 525 MW.

## Choose the arrangement

**Port** has two choices:

- **With outboard port.** One finite outboard service port, a 0.30 m × 0.30 m rectangular duct through the whole radial build. It has no shield plug. It is a bounding streaming case, and it is the 3D feature the magnets behind it feel most.
- **No port (matched control).** The same plant without the port.

**Allocation** lists the blanket/shield splits of the recorded study. The two arrangements you compare throughout the handbook are:

| Name | Blanket | Shield |
| --- | --- | --- |
| Reference | 0.45 m | 0.45 m |
| Breeder-heavy | 0.55 m | 0.35 m |

A radial-build bar shows the layers. Outlined layers are the ones that change between allocations. Look at it first.

## Inspect a component

Click a component in the viewport, or select it in the **Outliner**, to see its **Properties**. Tick or clear the boxes in the Outliner to show or hide components.

**Plant inputs** (major radius, radial build, fusion power) carry the badge *authored*. They are assumptions written for this scenario. **Sources and assumptions** lists the cited literature and the authored assumptions behind them.

Next: [Simulate: transport results](simulate.md).
