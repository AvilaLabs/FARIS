<!-- kind: reference -->
# Waste classification and clearance

Research screening only. These results are not a licensing, safety or design basis, and they are not a
regulatory determination of the class of any waste. A disposal site, a regulator and your own waste
acceptance criteria decide.

`faris waste` takes the activated nuclide inventory of one component and answers three questions at a
disposal time you state: which class it falls into under 10 CFR 61.55, what its IAEA clearance index is and when
that index falls below 1, and what its recycling index is against a dose-rate criterion you write.

The inventory comes from an activation calculation (for example ACTINV). FARIS reads it as given; it does not
change a nuclide's activity, and it never fills in a missing nuclide.

## The inventory file

One JSON file per component. The fields are:

| Field | Meaning |
| --- | --- |
| `component_id` | Your name for the component. |
| `waste_form` | `activated_metal` or `other`. The activated-metal rows of the tables apply only to `activated_metal`. |
| `volume_m3` | The volume the activity is averaged over, for the curie per cubic metre limits. You say which volume this is, for example the displaced volume. |
| `mass_g` | The mass the activity is averaged over, for the nanocurie per gram limits and for clearance. |
| `inventory_time_years` | Years after your reference time (for example shutdown) at which the activities are given. Default 0. |
| `transuranics_evaluated` | `true` once the activation calculation covered the alpha-emitting transuranic nuclides, so that none listed means none produced. |
| `inventory_kind` | What produced the inventory, copied onto the result (for example `solver_result`). |
| `nuclides` | A list of `nuclide` (`Co-60`, `Ag-110m`), `activity_bq`, `half_life_s`, optional `alpha_emitter` (needed for a transuranic nuclide with a half-life over 5 years), optional `progeny_of` (a decay product counted with its parent, such as `Ba-137m` with `Cs-137`; it is left out of every sum). |

## 10 CFR 61.55 classification

```
faris waste classify --inventory <inventory.json> --time-years 10
```

The result says `CLASS_A`, `CLASS_B`, `CLASS_C`, `GREATER_THAN_CLASS_C` or `NOT_EVALUATED`. It also gives:

- the scheme name, version and date, and where the tables were read from;
- every nuclide's fraction in every column it counts in, and the sum of fractions of each column (paragraph
  (a)(7)), so the total can be checked by hand;
- the waste form and which rows it used. Activated metal uses the "in activated metal" rows of Tables 1 and 2
  (C-14, Ni-63, Ni-59, Nb-94); every other form uses the plain rows;
- the rule that gave the class, in words, and the decay treatment.

What is implemented, by paragraph of 10 CFR 61.55: (a)(3) and (a)(4) the table columns and the 0.1 and 1 times
Table 1 steps; (a)(5) both tables together; (a)(6) Class A when no listed nuclide is present, but only for a
complete inventory; (a)(7) the sum of fractions; (a)(8) the concentration averaged over the one volume or mass
you state. A sum of exactly 1 stays inside its class (the rule's "does not exceed"), within 1e-9 relative.

Not implemented: the NRC guidance on mixed packages and hot spots, intruder assessments, site waste acceptance
criteria and state rules.

### When FARIS refuses

`NOT_EVALUATED` is returned, with the missing items and the next step, when:

- a nuclide a table needs is absent from the inventory. Give 0 where the calculation covered it and none was
  produced. A missing nuclide is never taken as absent and the class is never defaulted to Class A;
- the volume or the mass is missing for a limit that needs it, or `transuranics_evaluated` is false with no alpha
  emitters listed, or a transuranic nuclide with a half-life over 5 years has no `alpha_emitter` flag;
- Nb-94 or Ni-59 appears in a waste form other than activated metal: Table 1 lists them for activated metal only,
  so the regulation states no limit for another form;
- the disposal time is earlier than the inventory time. FARIS decays forward only.

### Disposal time and decay

The class is taken at the disposal time you give. Each nuclide decays from the inventory time by its own
half-life. There is no ingrowth from parent nuclides: where ingrowth matters, give an inventory calculated at the
disposal time. Changing the time changes the result.

## IAEA clearance index

```
faris waste classify --inventory <inventory.json> --time-years 10 --scheme iaea-gsr3-table-i2
faris waste clearance-time --inventory <inventory.json>
```

The index is the sum over nuclides of the activity concentration (Bq per gram, averaged over `mass_g`) divided by
the nuclide's level in IAEA GSR Part 3 Table I.2 (the summation rule of paragraph I.14). Below 1 means the table
would allow clearance; whether to clear is a regulator's decision. A nuclide with activity and no level makes the
index `NOT_EVALUATED` and is named; mark a decay product that the table counts with its parent using `progeny_of`.

`clearance-time` searches the window (default 1 to 100 years after the inventory reference) for the first time
the index is below 1, by bisection, and says if it is already below 1 at the start or still at or above 1 at the
end of the window.

## Recycling index

```
faris waste recycling --criterion <criterion.json> --contributions <contributions.json> --time-years 50
```

The index is a dose rate divided by a dose-rate criterion. FARIS carries no criterion value: you write the
criterion (`name`, `dose_rate_limit_sv_per_h`, `source`) and the result labels it authored and prints it beside
every number. The contributions are per-nuclide dose rates at your geometry, with a half-life and a sigma each;
the index sigma adds their sigmas linearly (fully correlated, the cautious choice).

## Schemes and tables

```
faris waste schemes
faris waste tables
```

`schemes` lists every scheme with name, version and date. The 10 CFR 61.55 scheme and the IAEA clearance scheme
are implemented. The UK waste thresholds and the Fetter specific-activity limits are listed as `NOT_EVALUATED`: the
values have not been transcribed and checked against their primary sources, so FARIS gives no result under them.

`tables` prints the encoded rows with their source, retrieval date and licence. The 10 CFR 61.55 rows were read
from the eCFR text retrieved on 2026-10-15 (title 10 current through its 2026-10-13 issue); that text is stored in
the repository with its SHA-256, and a test compares every row with it. The IAEA levels were read from Table I.2,
pages 124 to 127, of GSR Part 3 (STI/PUB/1578).

## Waste over plant life

```
faris waste lifetime --plan plan.json --maintenance-result maintenance.json --design ref
```

Reports waste mass and volume by class, by component and for the whole plant, over the life of the plant
including every replacement. The replacements come from a [`maintenance run`](maintenance.md) result (`--design`
names the design, `--history computed|fixed` the history inside it) or from a JSON list of replacements with
`--replacements`.

The plan gives, per component, what one installation is as waste:

```json
{"components": [
  {"component_id": "magnets", "pieces": 18, "mass_kg_each": 5000.5, "volume_m3_each": 0.7},
  {"component_id": "blanket", "mass_kg_each": 123456.789, "volume_m3_each": 21.3}
]}
```

`replacement_inventory` and `end_of_life_inventory` are optional, each `{"inventory": <an inventory as above>,
"disposal_time_years": 5.0}`. The class of a batch comes from classifying that inventory under 10 CFR 61.55. A
batch with no inventory, or whose inventory the classifier refuses, is counted under `NOT_EVALUATED` with the reason in its
`class_basis`; a class is never guessed. A replaced component that the plan gives no mass for refuses the whole
report and names the component.

Every replacement removes one installation (batch `component#k`) and the installation in place when the history
ends is the end-of-life batch (`component#end`). Each batch is counted once; a repeated batch id is refused. The
result lists every batch, then totals by class, by component, and by component and class, and a `sum_check`: the
plant total is summed from the batches and compared with the sum over components and the sum over classes, which
must agree to 1e-9 relative (for mass and for volume). A replacement outage still open at the end of the history
counts: the part has been taken out.
