# Scientific tools and integration roles

FARIS can use the wider open-source ecosystem. This table is a shortlist, not
a requirement to install every tool. None of these adapters is implemented yet.

| Role | Candidate tools | Initial use |
| --- | --- | --- |
| Geometry / CAD | [Paramak](https://fusion-energy.github.io/paramak/stable/), [bluemira](https://github.com/Fusion-Power-Plant-Framework/bluemira) | Parametric geometry and eventual CAD export; simple native OpenMC geometry may suffice for the first circular-torus reference |
| Neutron/photon transport | [OpenMC](https://docs.openmc.org/en/stable/) and [DAGMC](https://svalinn.github.io/DAGMC/) | Spatial fields and component spectra; use CAD transport when required by the selected geometry |
| Integrated plant/plasma scenarios | [PROCESS](https://github.com/ukaea/PROCESS), [FUSE](https://fuse.help/dev/) | Self-consistent design inputs when needed; the first demo can start with a documented prescribed scenario |
| Activation / transmutation | [ACTINV](https://github.com/AvilaLabs/ACTINV) | Component inventories, activity, decay heat, and photon sources from the same irradiation history |
| Magnets | [Converra](https://github.com/AvilaLabs/Converra) | Design and operating-margin inputs; an irradiation lifetime model still needs to be supplied |
| Tritium transport | [FESTIM](https://festim.readthedocs.io/), [TMAP8](https://mooseframework.inl.gov/tmap8/) | Higher-fidelity retention/extraction when it changes the result |
| Study compilation / evidence / execution | [Avila Core](https://github.com/AvilaLabs/Avila-Core) | Generated contracts and coarse-stage records for the demo study; shared simulation remains independently usable |

Source data, solver interfaces, numerical models, and their licenses must be
recorded when a particular adapter is selected. Open solver code does not imply
that every design geometry, measured material dataset, or processed nuclear
library is accessible or redistributable.

The neighboring Fusion Energy Ledger is a possible starting reference for fuel
and power balance. Reuse requires a deliberate Rust implementation/adapter and
its original scientific boundaries; there is no import of that Python core here.

The native interface uses [egui/eframe](https://github.com/emilk/egui) with a
[wgpu callback](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/trait.CallbackTrait.html)
for depth-tested 3D rendering. It requires no web server, browser, Node, or npm.
