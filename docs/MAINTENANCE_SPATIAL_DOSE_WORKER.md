# Spatial shutdown-dose worker inputs

The native dose-maintenance path can ask the bounded OpenMC worker for a curve when an exact
history/event/location key is missing. A design may name one `dose_physics` file in its existing
`faris-maintenance-designs/v0.1` entry. This file is a physical adapter manifest, not an estimate.
If it is absent, the event remains `NOT_EVALUATED`.

The manifest schema is `faris-maintenance-dose-physics/v0.1` and has these fields. Existing
prebuilt-model manifests are an advanced path; the supported CAD preparation path starts from a
complete passing Design ImportReport and checked H5M, then emits this adapter manifest.

```json
{
  "schema_version": "faris-maintenance-dose-physics/v0.1",
  "model_xml": {"path": "openmc/model.xml", "sha256": "..."},
  "geometry_h5m": {"path": "openmc/geometry.h5m", "sha256": "..."},
  "component_material_map": {"path": "openmc/component-material-map.json", "sha256": "..."},
  "cross_sections_xml": {"path": "nuclear-data/cross_sections.xml", "sha256": "..."},
  "chain_xml": {"path": "nuclear-data/chain.xml", "sha256": "..."},
  "activation_mesh": {
    "lower_left_cm": [-500, -500, -500],
    "upper_right_cm": [500, 500, 500],
    "dimension": [20, 20, 20]
  },
  "response_locations": {
    "task-location-id": {"path": "locations/task-location.json", "sha256": "..."}
  },
  "sampling": {
    "neutron_particles": 1000,
    "neutron_batches": 20,
    "photon_particles": 1000,
    "photon_batches": 20,
    "seed": 271828,
    "threads": 2,
    "mat_vol_samples": 50000,
    "activation_groups": "VITAMIN-J-42",
    "run_budget_s": 3600
  }
}
```

Paths are resolved relative to the manifest. Every referenced file is hashed before use. The model
XML must reference the exact H5M file and define its neutron source with data-only distributions;
compiled or dynamically imported source plugins are refused. The source kernel has total strength
1 neutron/source. The physical neutron/s rate is supplied by the verified FARIS transport run and
each component's exact Rust-generated irradiation schedule.

The component map has schema `faris-cad-component-material-map/v0.1`:

```json
{
  "schema": "faris-cad-component-material-map/v0.1",
  "geometry_sha256": "...",
  "materials": {"component-id": [101, 102]}
}
```

Each physical CAD solid must have a distinct OpenMC material ID, even when two solids have identical
composition. The cloned materials must retain their authored recipes, densities, temperatures and
physical locations. A map that shares material IDs across solids, omits an active component, or does
not hash-bind to the selected H5M is rejected. The output records the component material-volume
regions and their sampled volumes. Mesh-source resolution convergence remains a separate required
physical check; a single worker run does not establish it.

The separate CAD-preparation worker request is `faris-maintenance-dose-cad-prepare/v0.1`. FARIS
constructs its numeric material/plasma payload only after the Design SHA-256 matches the passing
ImportReport and the report's conversion H5M matches its recorded digest. The request carries an
authored `solid_history_components` mapping covering each nonvoid CAD solid exactly. The worker
clones the audited composition, density, and temperature once per solid, retags a copied H5M using
the trusted `design_transport.py` helper, builds the model with the checked tokamak source normalized
to unit strength, and writes a component-material map. It records hashes of the original/derived
H5M, exported model, and map. It neither mutates the checked geometry nor loads study-provided Python
or compiled source plugins. The irradiated geometry/material state is reused across component
history schedules; that fixed-geometry assumption must hold over the modeled history.

The activation preflight rejects chain nuclides with listed depletion reactions that are absent
from the selected neutron cross-section index. OpenMC's MicroXS path otherwise silently leaves
missing nuclide rows at zero. Presence in the index does not establish that every channel is
complete or that the chain is adequate; those remain nuclear-data/model limitations.

Each response-location file must contain `schema: faris-task-location/v0.1`, the same `id` used by
the dose threshold, `lower_left_cm`, `upper_right_cm`, and `dimension: [1, 1, 1]`. The worker reports
ICRP-116 AP photon effective dose averaged over that authored volume. It uses the complete CAD
geometry for neutron and photon transport.

For each replacement event, Rust writes the exact history serialization to a separate artifact and
passes its SHA-256 unchanged. Rust builds an irradiation schedule for every currently installed
component. R2S activates each component's mesh/material regions under its own schedule, creates
separate decay-source meshes, and combines all active sources in one full-geometry photon run per
cooling time. Material-volume and activation-mesh approximation, source conservation, batch loss
counts, run limits and loaded OpenMC C-library identity are included in the receipt. The reported
standard error covers aggregate photon batches conditional on the activated source; it does not
include neutron-response, activation-source, geometry, composition or nuclear-data uncertainty.

Stock D1S is run only when the currently installed components share the same irradiation schedule.
With different installation histories, the R2S dose curve can still be selected, while D1S is
stored as `NOT_EVALUATED` with the reason that OpenMC D1S time factors are global per radionuclide.
The method comparison remains `NOT_EVALUATED`; an R2S-only result does not satisfy a D1S/R2S
validation gate.
