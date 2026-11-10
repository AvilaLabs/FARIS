# PROCESS and bluemira LifeCycle comparison

This adapter runs the official PROCESS CLI and bluemira's public `LifeCycle` API as separate processes. The FARIS fixed-outage and physics-computed-outage records are included by SHA-256 as the authoritative comparison reference. External results are code-to-code references; they never set a FARIS validation status.

## What the adapter compares

The manifest requires a shared horizon, net electrical power, component lifetimes and fixed replacement durations, parallel remote-handling count, and explicit authored/literature/measurement/FARIS-output provenance. It also requires each tool's native downtime mapping. `unmapped` items stay visible and make the conditional comparison incomplete. The adapter does not treat an absent field as zero.

PROCESS supports a native fixed-duration availability model (`i_plant_availability = 1`) with blanket, divertor and combined replacement times and separate unplanned-unavailability fractions. Its Morris model instead derives planned/unplanned downtime and uses a fit for remote-handling replacement times with additional dose wait and pump-down assumptions. The chosen model and actual input values must be visible in the pinned `IN.DAT` and mapping record.

bluemira has no standalone LifeCycle command. Its public `bluemira.fuel_cycle.lifecycle.LifeCycle` API generates a damage-governed maintenance timeline and combines it with learning, pulsed operation and an operational-outage strategy. The adapter calls that native API and preserves its timeline. Its reproducible outage strategy and seed must be declared. It does not invent a replacement duration formula or a fake LifeCycle result. The LifeCycle can only enter a matched comparison when its pinned config can express the same plant horizon, component lifetimes, fixed maintenance durations and other downtime assumptions. Any unsupported correspondence must be labelled `unmapped` and the resulting availability/net-energy summary is marked incomplete.

The conditional energy normalization is `matched net MW(e) × native full-power years × 8760 h/year`. The full PROCESS CLI path contributes its native capacity factor and plant lifetime from the official MFILE reader. The controlled availability-submodel path contributes PROCESS's native Ward--Taylor metrics directly. bluemira contributes its native `fpy` and `T` lifecycle attributes. Energy is omitted for either model when its native horizon does not equal the case horizon. These are conditional model comparisons, not measured plant performance.

## FARIS maintenance reference available in this checkout

`references/maintenance-0.2-parity.json` (SHA-256 `02822be5cc83b9c8b57d3b7fa3e198f6a3aecc06e7ef060e091caf7c44d92d71`) contains archived fixed and computed histories for `no-port__breeder` and three other designs over 946,728,000 seconds (30 Julian years). Its fixed/computed `lifetime_net_electricity_mwh` values come from the saved snapshots; this adapter does not rerun transport or activation physics. The adapter recomputes total downtime and availability from the archived replacement intervals using FARIS's `interval_union_duration_s` semantics and reports the archive's old summary alongside the recomputed summary. This preserves the archive unchanged and makes the method/source limitation explicit.

For `no-port__breeder`, the archive records 11 fixed blanket events at 60 days each, and 9 computed blanket events with recorded durations. The saved net-energy snapshots are 35,272,721.86735174 MWh fixed and 30,566,655.251259945 MWh computed (computed minus fixed: -4,706,066.616091795 MWh). Those are historical FARIS outputs, not peer results. They remain separate reference context.

## Authored controlled verification case

`cases/controlled_fixed/manifest.json` defines two authored blanket operating campaigns and one completed replacement between them. The campaign interval is the archived FARIS fixed-history value (79,859,518.2721506 s); maintenance duration is 60 days. The case horizon is two campaign intervals plus the one replacement, minus a 600 s terminal margin (one configured FARIS history step) so a replacement exactly at the horizon is not counted. This yields 5.225442889963153 authored years. PROCESS receives the explicit blanket life, pulse boundary, replacement duration, and zero-unplanned assumption. Its longer divertor-life input is authored at exactly two blanket intervals (5.061190855587916 FPY), with zero divertor replacement time, solely to define one blanket-only event per Ward–Taylor combined cycle.

The pinned PROCESS 3.4.2 Ward–Taylor implementation calculates `n = ceil(lb / ld) - 1`, where `ld` is the shorter and `lb` the longer component life. Setting the explicit divertor schedule boundary to `2 × blanket_lifetime` makes `n = 1`; its native planned availability is therefore `2L / (2L + D)` for two blanket operating intervals `L` and one fixed duration `D`. This sentinel is a control input, not a divertor service-life claim. PROCESS reports its periodic submodel fraction while bluemira reports its pulse-resolved finite timeline fraction; the comparison retains the strict 1e-6 absolute agreement criterion, with the authored 600 s terminal margin reflected in bluemira's actual timeline. No output is adjusted to force agreement.

bluemira 2.14.0 receives the same 60-day replacement as 5,184,000 seconds: its `LifeCycleParams.bmd` field is seconds internally, though its field documentation describes days. The blanket damage rate and limits are authored from the documented EU-DEMO example's 10.2 dpa/FPY, scaled to the shared 525 MW fusion boundary; this does not establish FARIS fluence-to-DPA equivalence or represent a measurement. The pinned 2.14.0 phase builder leaves `count2` undefined when the second blanket interval has no divertor change. To run its native LifeCycle path without modifying upstream source, this case sets an explicit 1 dpa/FPY divertor compatibility rate, a 0.4-campaign DPA limit, and zero-second divertor maintenance. That creates four native zero-duration divertor phases; they are disclosed compatibility scaffolding, not physical inputs and do not contribute downtime. Vessel and TF limits remain high sentinels. The worker records actual timeline horizon and full-power years from `Timeline.plant_life` and `Timeline.ft[-1]`, rather than the continuous damage-budget value on `LifeCycle.fpy`.

The comparison keeps its strict 1e-6 horizon and native availability checks. A source-inspected difference in bluemira's pulse-array endpoint convention can leave the actual availability outside that tolerance; the runner reports that native result as incomplete and does not adjust outputs or tolerances. One remote-handling system and 500 MW(e) are authored shared boundaries; bluemira has no matching native remote-handling count or electric-power calculation, so those values do not imply peer throughput or native MWh output.

This case invokes the actual PROCESS `Availability.run(WARD_TAYLOR)` API with physics lifetime calculation bypassed and explicit lives. It invokes the actual bluemira `LifeCycle.make_timeline()` loop. For the deterministic control, a small subclass of bluemira's documented `LogNormalAvailabilityStrategy` overrides only its documented `generate_distribution(n, integral)` extension point to return zero unplanned downtime; it raises if the API asks it to represent positive unplanned downtime. Blanket damage limits still govern the native maintenance timeline. This is a scoped verification harness, not a whole PROCESS system-code run or physical validation.

The matched FARIS control inputs and native runner are in `cases/controlled_fixed/faris/` and `run_faris_native_case.py`. The FARIS runner first executes the native 709-group transport and `history from-run`, then derives an authored integrated-flux threshold from the actual native blanket flux rate multiplied by the authored 79,859,518.2721506-second interval. That threshold is explicitly a software control trigger, not a measured or qualified lifetime. The native maintenance command evaluates fixed and ACTINV-coupled histories with identical scenario, transport, horizon, threshold, and energy assumptions. Only the maintenance-duration coupling changes. The 30-day work duration and calibrated 30-day cooldown target produce a 60-day first computed duration under the authored control. The native energy record remains separate from peer-normalized energy.

After the coordinator supplies the compiled FARIS CLI, matching OpenMC Python/executable, compatible audit and cross sections, and pinned ACTINV executable/data, run:

```sh
python3 integrations/lifecycle_peer/run_faris_native_case.py \
  --faris /path/to/faris \
  --openmc-python /path/to/openmc-env/bin/python \
  --openmc /path/to/openmc \
  --cross-sections /path/to/cross_sections.xml \
  --audit /path/to/read-only-audit.json \
  --actinv /path/to/actinv \
  --data-dir /path/to/actinv-data \
  --out runs/peer-lifecycle/no-port-breeder-one-boundary-outage-control/faris
```

This native case is authored cold-data code verification, not experimental validation. It must complete successfully before the peer control can be described as a FARIS-matched comparison.

After the coordinator creates the separately pinned PROCESS 3.4.2 and bluemira 2.14.0 environments, run the concrete case with:

```sh
python3 integrations/lifecycle_peer/run_controlled_case.py \
  --process-python /path/to/process-env/bin/python \
  --bluemira-python /path/to/bluemira-env/bin/python \
  --out runs/peer-lifecycle/no-port-breeder-one-boundary-outage-control
```

The runner checks input hashes, launches each native API in a bounded separate process, and keeps the peer timelines/metrics under the selected output directory. It reports a matched controlled baseline only when native outputs confirm the horizon, one completed blanket replacement, zero unplanned downtime, zero planned time from the bluemira compatibility phases, and availability agreement within 1e-6. Any failed check leaves the comparison incomplete. FARIS's 30-year fixed/computed outputs are reported separately and are never treated as matched peer outputs. No external framework was installed or executed when this case was authored.

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
