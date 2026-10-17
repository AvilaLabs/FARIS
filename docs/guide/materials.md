<!-- kind: reference -->
# Materials and limits database

Research screening only. These results are not a licensing, safety or design basis.

`faris materials` reads a versioned, plain-text database (one JSON file in the repository) of material
compositions, impurity limits, properties and damage and gas limits. Every stored value carries a label and a
citation with the source, the page or table, and the quoted text. The seed holds only values that were read from
a source opened for the purpose; a value that could not be cited is `NOT_EVALUATED` with the reason and what to
add.

## Labels

Each value is one of: `authored` (you wrote it, with author and reason), `literature` (a source states it),
`calculated` (FARIS derived it and says how), `conditional` (a source states it under conditions that are listed),
or `NOT_EVALUATED` (no cited value; the reason and next step are stored). A literature value cites its source.

## What is in the database

```
faris materials list
faris materials check
faris materials limits
faris materials watchlist
```

- **Materials**: base composition (mass percent, with the balance element calculated so the fractions sum to 1)
  and impurity specifications in weight ppm, each labelled `nominal`, `specified`, `measured` or `assumed`, with
  its source. The seed has EUROFER97 (composition and the specification limits for Nb, Mo, Ni, Cu, Al, Ti, Si
  and Co, and the range achieved in production). Tungsten, CuCrZr, Inconel 718, SiC/SiC, REBCO, the magnet insulator
  and 316 stainless steel are listed with their composition `NOT_EVALUATED`.
- **Limits**: quantity (dpa, helium appm, hydrogen appm, fluence, dose), value and unit, the dpa convention, the
  irradiation temperature window, the spectrum, the material form, the conditions, the status (design aspiration,
  code rule or measurement), the label, and the citation. A field the source does not state says so and why.
- **Watch-list**: the elements whose trace content decides a waste class (Nb, Mo, Ag, Co, Gd, Tb, Ho), the nuclide
  each makes, the reason, and the class threshold it endangers. Where 10 CFR 61.55 has a row the threshold is read
  from the encoded table; where it has none (Ag-108m, Tb-158, Ho-166m) the literature limit is given and the
  absence of a 61.55 row is stated.
- **Sources**: each source's citation, URL, retrieval date, licence and redistribution status (`public_domain`,
  `cc_by`, or `quote_only`: values are quoted with a citation and the document itself is not redistributed).

`faris materials check` validates the database (schema, duplicates, every citation present, every written value
inside its quote and equal to the stored number); `--file` checks another database file. The same checks run in CI
with a second reader, `scripts/materials_db_lint.py`; a monthly job asks whether every source link still resolves.

## Comparing a value with a limit

```
faris materials compare --limit <limit-id> --value <value> --conditions <conditions.json>
```

A comparison happens only when the case says what the limit needs:

- a dpa value is compared only with a limit in the same dpa convention. If the convention differs, or the stored
  limit does not state one, the result is `NOT_EVALUATED` and says why. The Federici DEMO limits do not name their
  convention, so they cannot be compared until you enter an authored override that names it;
- where a limit states an irradiation temperature window, a spectrum or a material form, the case states its own.
  A missing statement gives `NOT_EVALUATED`; a statement outside the limit's conditions gives `WITHIN` or
  `EXCEEDED` labelled `conditional`, with the unmet conditions listed;
- the result carries the database id, version and content hash. A change to any stored value changes the hash, so
  a result made under an older database is recognised as out of date.

A limit you replace is an authored override: it needs an author, a time and a reason, and the result is labelled
`authored_override`, never `literature`, with the stored value shown beside it. FARIS reads no clock; you supply
the time.

## Which limit is reached first

```
faris materials time-to-limit --material eurofer97 --criteria <criteria.json>
```

Each criterion is a limit id with its accumulated value and a constant rate per year. The result gives years to
each limit and names the one reached first. A dpa-only check is refused for a material that has a helium or
hydrogen limit: the gas limit must be checked with it.

## Impurities and waste class

```
faris materials classify --material eurofer97 --inventory <inventory.json> --inventory-spec <spec-id> --spec <spec.json> --time-years 10
faris materials max-impurity --inventory <inventory.json> --element-yield <yield.json> --target CLASS_A --time-years 10
```

`classify` states a waste class only with an impurity specification that covers every watch-list element and that
the inventory was computed with. Without one it returns `NOT_EVALUATED` with the elements needed and the next step.
You can add an assumed limit (element, wt ppm, author, reason) for an element the stored specification does not cover.

`max-impurity` gives the largest content of one element that keeps the component at or below a target class at
the disposal time, and names the table column that binds. It relies on two linearities: the sums of fractions in
10 CFR 61.55 are linear in each nuclide's concentration, and activation at a fixed flux is linear in the starting
composition. The element's yield (becquerel per weight ppm, per nuclide) comes from the activation calculation.
It does not model self-shielding or a spectrum that changes with composition.

To see the effect of a whole impurity specification on activity, decay heat and waste class, element by element,
and which elements matter most, use `faris activation impurities`
([Which impurities matter](activation.md#which-impurities-matter)). Each impurity in its request carries the
kind (`nominal`, `specified`, `measured`, `assumed`) and source of its value, and the result shows the effect per
element; two specifications are two requests.

## Properties

A property with a table is evaluated by linear interpolation inside its cited valid range. Outside the range the
result is `NOT_EVALUATED` with the range; it is never extrapolated. The seed holds no cited temperature table yet.

## Export

```
faris materials export --format csv-limits --output <limits.csv>
```

`json` is the database file itself; `csv-limits` and `csv-properties` have one row per limit or property with the
citation columns, and read back to the same rows.
