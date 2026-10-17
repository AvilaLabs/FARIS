<!-- kind: explanation -->
# Activation, decay heat and decay photons

FARIS hands the neutron flux in each component, and the irradiation history of the plant, to ACTINV, Avila Labs'
inventory solver, and reads back what is in the component afterwards: nuclides, activity, decay heat, the decay-photon
source and the main production paths. ACTINV does the physics. FARIS builds the problem, runs it under the same
bounded job runner as every other external tool, checks that nothing changed on the way in, converts units on the
way out, and records everything that identifies the run.

Every number is research screening. The result carries that statement and says nothing about whether any
regulation or limit is met.

## What you need

- **ACTINV** and its data (the v1.1.0 catalogue). See [actinv.avilalabs.org](https://actinv.avilalabs.org). ACTINV 1.5.0 or later is recommended: before it, the auto mode could hold a radioactive starting material constant. FARIS sends the coupled mode for any start from named nuclides and records that in the result, and it refuses an explicit trace mode there.
- **A group flux for each zone.** A [transport run](transport.md) with the activation spectra option tallies each
  component's neutron flux on ACTINV's 709-group structure. An empty spectrum means zero flux; a missing one is an
  error, never a zero.
- **An irradiation schedule** from the [operating history](operate.md), or steps you write yourself.

## Run it

```bash
faris activation run --zones zones.json --spectra spectra.json --schedule schedule.json \
  --cooling 1s,1h,1d,1y,10y,100y --actinv ~/.local/bin/actinv --data-dir ~/actinv-data \
  --output activation.json
```

`zones.json` is a list. Mass and volume are the zone's own; `component_id` names the transport component whose
spectrum drives the zone (the zone id when left out):

```json
[{"id": "blanket-1", "component_id": "blanket", "mass_g": 2.0e6, "volume_m3": 0.26,
  "basis": "wt_percent", "composition": {"Fe": 88.7, "Cr": 9.0, "W": 1.1, "Ta": 0.1}}]
```

`--request FILE` takes the whole request in one file instead (spectra in n per cm2 per second, steps, cooling
grid). `--photon-groups N` asks for N decay-photon groups instead of ACTINV's 24; `--photon-response FILE
--photon-response-sha256 HASH` adds an ACTINV photon response so the contact dose proxy is computed;
`--photon-source-dir DIR` writes the shutdown-dose source files; `--cache DIR` keeps zone results so an identical
request is not run again; `--top-nuclides N` keeps the N most active nuclides per cooling point (totals stay
complete and the number left out is recorded).

Cooling times run from 1 s to 1e9 s and results are given at the requested times only, with no interpolation.
The irradiation schedule can have up to 2,000 steps; condense a longer history with its lumping rule first
(the error bound of that rule is in [the operating history](operate.md)).

## What the result holds

For each zone and cooling time:

- **Inventory**: atoms of every nuclide in the whole zone, and its activity in Bq.
- **Decay heat**: W per m3 and kW for the zone, with the alpha, beta and gamma parts (they sum to the total).
- **Decay-photon source**: photons per second per group for the zone, the total over all lines, and the part the
  group structure leaves out. The shutdown-dose source file gives the rate per cm3 (unit `photons/s/cm3`), the
  group edges, and the hash of the activation result it came from.
- **Main paths**: for the ten most active nuclides, the reactions that make them (parent, first product, atoms).
- **Totals** over all zones, summed in zone order.

The result also records the ACTINV version, the activation library and decay data (identifier and SHA-256), the
irradiation schedule's hash, the hash of every zone's spectrum and composition, and the hash of the ACTINV problem.
The same inputs give the same result byte for byte. A changed library or decay file changes the hash and the cache key.

## What was checked on the way

After each run FARIS compares what ACTINV says it received with what was sent: the total flux, the fluence of the
schedule, the end time of every cooling step, the decay-file hashes, and that no nuclide has an activity without an
inventory row. A difference stops the run with a message; it is never absorbed.

## Missing data

ACTINV lists the nuclides, targets and branches it had no data for. FARIS keeps every one as a *data gap*, with the
count, the first identifiers, the amount ACTINV puts on them, what that does to the totals and what to do next. A
gap that can only lower the totals (a product with no decay data, atoms routed out of the chain) turns the zone's
data to `LOWER_BOUND` and the numbers are labelled `conditional` instead of `calculated`. A missing pathway is not
treated as zero anywhere.

Each number carries a kind: `calculated` (a solver result), `authored` (mass, composition, schedule) or
`conditional` (a lower bound). What FARIS cannot give is listed as NOT_EVALUATED, each with the reason and the
next step.

## Which impurities matter

```bash
faris activation impurities --request impurities.json --actinv ~/.local/bin/actinv --data-dir ~/actinv-data \
  --output impurities-result.json --cache cache/
```

Ranks the trace elements of a material by their effect on activity, decay heat and the 10 CFR 61.55 sums of
fractions, at each cooling time you ask for. The request holds the alloy without its impurities
(`matrix_wt_percent`), the impurities (`element`, `wt_ppm`, and the `kind` and `source` of each value:
`nominal`, `specified`, `measured` or `assumed`), the zone mass and volume, a 709-group neutron spectrum in n per
cm2 per second, the irradiation steps and the cooling times (1 s to 1e9 s).

All runs go to ACTINV in one request, one zone each: the matrix alone, the full specification, each element
alone added to the matrix, and each element at its content times (1 - h) and (1 + h) with the others present
(`relative_step` h, default 0.5). Twelve elements make 38 zones.

Why one run per element is enough: at a fixed flux the inventory is linear in the starting composition as long as
the starting nuclides are not burned up, and a part per million is not. So the response per weight ppm of an
element is the difference between its run and the matrix run, divided by its content, and the effects of the
elements add up to the full specification's excess over the matrix. FARIS does not take that on trust. For every
element it also computes a central finite difference around the full specification (all other impurities
present) and compares the two per element. The result records, for activity, decay heat and waste class, the
largest relative difference between the two estimates, any pair of elements the two order differently, and
whether the check passed: every compared element within 20 %, and a swapped pair only if the two effects are
within 20 % of each other. An element whose effect is under 1e-10 of the total is ranked but left out of the
check, and listed.

The ranking is by the absolute effect at the specified content (per ppm times ppm), for each of the three
quantities, and an overall rank from the mean of the three. The waste measure is the sum of the Table 1 and
Table 2 column 3 sums of fractions of 10 CFR 61.55 as activated metal, computed from the ACTINV inventory with
half-lives from the decay data ACTINV used; every element also carries all four column sums per ppm. The waste
part is `NOT_EVALUATED` with the reason when a nuclide has no half-life in the decay data or no free name in the
classifier; isomers beyond the first (`Hf179m2`) are listed to the classifier under the `m` name or the plain
mass name and the point says so in `waste_notes`. A zone with a data gap that can only lower totals makes the
result `conditional` and names the zones.

The spectrum is yours: the sensitivities are those of the composition and flux given, and a different spectrum
changes the ranking. Self-shielding and a spectrum that changes with composition are not modelled.

## What ACTINV does not provide yet

These are written down for ACTINV, not worked around in FARIS:

1. Decay heat per nuclide and radiation type (only the whole-inventory split is reported; photon power is per nuclide).
2. Pathway ranking by contribution to decay heat and dose (ACTINV ranks by atoms of the product).
3. A contact dose rate without a photon response table (ACTINV ships none).
4. Continuing from a saved inventory, so a changed cooling grid is a new run (seconds), not a decay-only update.
5. Returning only chosen steps, so a long irradiation history needs lumping before it is sent.
6. Machine-readable data identity (library evaluation year and decay-file versions) in the certificate.
