# PROCESS and bluemira LifeCycle comparison

This adapter runs the official PROCESS CLI and bluemira's public `LifeCycle` API as separate processes. The FARIS fixed-outage and physics-computed-outage records are included by SHA-256 as the authoritative comparison reference. External results are code-to-code references; they never set a FARIS validation status.

## What the adapter compares

The manifest requires a shared horizon, net electrical power, component lifetimes and fixed replacement durations, parallel remote-handling count, and explicit authored/literature/measurement/FARIS-output provenance. It also requires each tool's native downtime mapping. `unmapped` items stay visible and make the conditional comparison incomplete. The adapter does not treat an absent field as zero.

PROCESS supports a native fixed-duration availability model (`i_plant_availability = 1`) with blanket, divertor and combined replacement times and separate unplanned-unavailability fractions. Its Morris model instead derives planned/unplanned downtime and uses a fit for remote-handling replacement times with additional dose wait and pump-down assumptions. The chosen model and actual input values must be visible in the pinned `IN.DAT` and mapping record.

bluemira has no standalone LifeCycle command. Its public `bluemira.fuel_cycle.lifecycle.LifeCycle` API generates a damage-governed maintenance timeline and combines it with learning, pulsed operation and an operational-outage strategy. The adapter calls that native API and preserves its timeline. Its reproducible outage strategy and seed must be declared. It does not invent a replacement duration formula or a fake LifeCycle result. The LifeCycle can only enter a matched comparison when its pinned config can express the same plant horizon, component lifetimes, fixed maintenance durations and other downtime assumptions. Any unsupported correspondence must be labelled `unmapped` and the resulting availability/net-energy summary is marked incomplete.

The conditional energy normalization is `matched net MW(e) × native full-power years × 8760 h/year`. PROCESS contributes its native capacity factor and plant lifetime from the official MFILE reader. bluemira contributes its native `fpy` and `T` lifecycle attributes. Energy is omitted for either model when its native horizon does not equal the manifest horizon. These are conditional model comparisons, not measured plant performance.

## FARIS maintenance reference available in this checkout

`references/maintenance-0.2-parity.json` (SHA-256 `02822be5cc83b9c8b57d3b7fa3e198f6a3aecc06e7ef060e091caf7c44d92d71`) contains archived fixed and computed histories for `no-port__breeder` and three other designs over 946,728,000 seconds (30 Julian years). Its fixed/computed `lifetime_net_electricity_mwh` values come from the saved snapshots; this adapter does not rerun transport or activation physics. The adapter recomputes total downtime and availability from the archived replacement intervals using FARIS's `interval_union_duration_s` semantics and reports the archive's old summary alongside the recomputed summary. This preserves the archive unchanged and makes the method/source limitation explicit.

For `no-port__breeder`, the archive records 11 fixed blanket events at 60 days each, and 9 computed blanket events with recorded durations. The saved net-energy snapshots are 35,272,721.86735174 MWh fixed and 30,566,655.251259945 MWh computed (computed minus fixed: -4,706,066.616091795 MWh). Those are historical FARIS outputs, not peer results. They provide a reference scenario and a fixed blanket work duration, but do not provide the DPA/fluence conversion needed to map FARIS's component replacement trigger to bluemira's LifeCycle damage limits. A peer comparison manifest must leave that lifetime correspondence incomplete until a sourced mapping is available; it must not present the outputs as an established matched baseline.

## Sources, versions and licenses

- [PROCESS official installation guide](https://ukaea.github.io/PROCESS/installation/installation/) supports Python 3.11+, an isolated environment, an official release checkout (the guide shows tag `v3.4.2`) and `pip install .`.
- [PROCESS official run guide](https://ukaea.github.io/PROCESS/usage/running-process/) documents `process -i path/to/name_IN.DAT` and `_OUT.DAT`/`_MFILE.DAT` outputs; the [MFILE API](https://ukaea.github.io/PROCESS/source/reference/process/core/io/mfile/base/) provides the parser used by `process_mfile_metrics.py`.
- [PROCESS availability documentation](https://ukaea.github.io/PROCESS/eng-models/plant-availability/) documents user-input, Taylor–Ward, Morris and ST modes and their lifecycle inputs. PROCESS is MIT-licensed in [the official source repository](https://github.com/ukaea/PROCESS/blob/main/LICENSE.txt).
- [bluemira installation guide](https://bluemira.readthedocs.io/en/latest/installation.html) documents the Miniforge environment install. Its [LifeCycle API](https://bluemira.readthedocs.io/en/latest/autoapi/bluemira/fuel_cycle/lifecycle/index.html), [fuel-cycle example](https://bluemira.readthedocs.io/en/latest/examples/fuel_cycle/EUDEMO_fuelcycle.html) and [fuel-cycle model notes](https://bluemira.readthedocs.io/en/latest/fuel_cycle/fuel_cycle.html) document the API and operating assumptions. The upstream [latest tagged release](https://github.com/Fusion-Power-Plant-Framework/bluemira/releases/tag/v2.14.0) is LGPL-2.1-or-later.

Pin the installations separately: PROCESS `v3.4.2`, bluemira `v2.14.0`, Python 3.11. Do not install either as part of FARIS startup or link/vendored packaging. These peer tools are separate processes and keep their notices/licenses.

## Run

This task does not install or execute either peer framework. After the coordinator has created the two isolated environments and filled a provenance-complete manifest:

```sh
python3 integrations/lifecycle_peer/compare.py path/to/peer-comparison.json \
  --out runs/peer-lifecycle/<comparison-id>
```

The PROCESS input is a pinned, user-reviewed `*_IN.DAT` prepared for the fixed-duration reference. The bluemira config is a pinned JSON object with `lifecycle_config`, `learning_strategy`, `availability_strategy`, `inputs`, and a `provenance` record for every numeric config parameter; its adapter emits the native LifeCycle timeline. `manifest.schema.json` is the JSON Schema contract. The archived FARIS parity fixture alone is insufficient to create a fully matched PROCESS/bluemira case: PROCESS can accept FPY directly but its normal plant model calculates lifetimes from design physics; bluemira LifeCycle derives replacements from DPA/fluence and planned maintenance, and no source in this fixture maps FARIS neutron-fluence triggers to its DPA thresholds. Until that correspondence is sourced, any run report must show it as incomplete and must not attribute output differences to FARIS computed outages.

The runner uses argument arrays with no shell, a per-process timeout, an output-byte cap, SHA-256 input/output records, and separate working directories. It refuses to overwrite a non-empty output directory. The report keeps peer-native metrics distinct from its conditional MWh normalization.

Run only the stdlib contract tests before any framework setup:

```sh
python3 -m unittest integrations.lifecycle_peer.test_compare
```
