<!-- kind: reference -->
# Waste classification and clearance

Research screening only. These results are not a licensing, safety or design basis, and they are not a
regulatory determination of the class of any waste. A disposal site, a regulator and your own waste
acceptance criteria decide.

`faris waste` takes the activated nuclide inventory of one component and answers three questions at a
disposal time you state: which class it falls into under the regime you choose (10 CFR 61.55, the UK waste
thresholds, the Fetter limits, or the IAEA or UK clearance values), what its IAEA clearance index is and when
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

### Choosing the regime, the profile and the receipt

`--scheme` chooses the regime. Every `faris waste classify` result names it (`regime`, `regime_name`, and the
`scheme` block of the result) and the class it gave (`class`). It also names the **jurisdiction profile** the
scheme belongs to (`profile`: name, version, authored date, review date, status on the evaluation date) and
records the choice in a **receipt** (`receipt`: the scheme choice and how it was made, the scheme name, version
and date, the profile id and version, the SHA-256 of the inventory file, the time, the evaluation date and the
class, with a SHA-256 of the receipt text). `--receipt <file>` also writes the receipt to a new file, so the
choice is kept with the result.

| `--scheme` | Regime | `class` values |
| --- | --- | --- |
| `nrc-10cfr61.55` (default) | US NRC 10 CFR 61.55, profile `us-nrc` | `CLASS_A`, `CLASS_B`, `CLASS_C`, `GREATER_THAN_CLASS_C` |
| `iaea-gsr3-table-i2` | IAEA GSR Part 3 Table I.2 clearance, profile `iaea` | `CLEARANCE_INDEX_BELOW_1`, `CLEARANCE_INDEX_AT_OR_ABOVE_1` |
| `uk-llw-ilw` | UK LLW and VLLW definitions, profile `uk` | `VLLW_HIGH_VOLUME`, `LLW`, `ABOVE_LLW_ILW_OR_HLW` |
| `uk-epr2016-sch23-table2` | England and Wales Environmental Permitting Regulations 2016 Schedule 23 Table 2, profile `uk` | `OUT_OF_SCOPE_BY_TABLE_2`, `ABOVE_TABLE_2` |
| `fetter-specific-activity` | Fetter, Cheng and Mann (1990) Class C limits, profile `fetter-literature` | `SUM_AT_OR_BELOW_1`, `SUM_AT_OR_BELOW_1_AT_UPPER_RANGE_ONLY`, `SUM_ABOVE_1` |

Every class can also be `NOT_EVALUATED`, with the reason and the next step.

A profile carries an authored **review date**. A profile past that date blocks verdicts: the class is
`NOT_EVALUATED`, `blocked_by_profile` says which profile and when it had to be reviewed, and no scheme is run.
The fix is to re-check the profile's sources and re-date it in `crates/faris-model/data/waste/jurisdiction_profiles.json`.
`--date YYYY-MM-DD` sets the evaluation date (the default is today, UTC), which makes a run repeatable. The
profiles and their review dates are:

| Profile | Version | Review by | Schemes |
| --- | --- | --- | --- |
| `us-nrc` | 1 | 2027-01-16 | `nrc-10cfr61.55`. The NRC fusion-machine rule (Part 30 byproduct framework) is a proposed rule of 2026-02-26; no final rule was found on 2026-10-16. |
| `uk` | 1 | 2027-04-16 | `uk-llw-ilw`, `uk-epr2016-sch23-table2` |
| `iaea` | 1 | 2027-10-16 | `iaea-gsr3-table-i2` |
| `fetter-literature` | 1 | 2027-10-16 | `fetter-specific-activity` |

Each limit of a scheme carries its citation, the version of its source and the date it was checked (the date the
source was retrieved and read); the model builds that list for every profile and a test checks it.

### UK waste thresholds (`uk-llw-ilw`)

The UK defines low level waste (LLW) as "radioactive waste having a radioactive content not exceeding four
gigabecquerels per tonne (GBq/te) of alpha or 12 GBq/te of beta/gamma radioactivity" (UK Strategy for the Management
of Solid Low Level Waste from the Nuclear Industry, February 2016; the same wording is in the 2007 Defra/DTI policy).
Very low level waste in high volumes is waste with "maximum concentrations of four megabecquerels per tonne
(MBq/te) of total activity", and 40 MBq/te for tritium. FARIS reads the first as two limits that both hold, one for
alpha activity (the nuclides you flag `alpha_emitter: true`) and one for beta/gamma, and the second as a sum of
fractions (non-tritium over 4 plus tritium over 40) not above 1, since the text states each limit alone and gives no
rule for a mixture. The inventory needs `mass_g`, and every nuclide with activity needs an `alpha_emitter` flag
(a missing flag is not taken as beta/gamma). Decay products you list count at their activity, because the
definitions are of the radioactive content.

Encoded only what has a number. Intermediate level waste is defined as waste above the upper LLW boundaries that
needs no heat allowance, and high level waste as waste whose temperature may rise significantly: neither has a
number for the heat criterion, so a waste above an LLW limit is `ABOVE_LLW_ILW_OR_HLW` and FARIS does not say which.
The low-volume VLLW limits (400 kBq per 0.1 m3, 40 kBq per item) depend on how the waste is packaged and itemised,
which an inventory of one component does not say; they are listed in `faris waste tables` and not evaluated.

### UK out-of-scope concentrations (`uk-epr2016-sch23-table2`)

The England and Wales Environmental Permitting Regulations 2016, Schedule 23 Part 3 Table 2, give a concentration
in Bq/g per radionuclide above which a solid or relevant liquid containing radionuclides not of natural terrestrial
or cosmic origin is radioactive material or waste (Part 2 paragraph 6). The sum of concentration over value is
"exceeded" only when it is greater than 1, so a sum of exactly 1 is not above. A nuclide marked `+` has its
daughters of Table 3 included in the value: list a daughter with `progeny_of` and it is not counted again. A
nuclide with no Table 2 row falls under the schedule's 10 microsievert per year dose criterion, which FARIS does not
compute, so the result is `NOT_EVALUATED` naming it. FARIS does not apply the dilution rule (paragraph 6A) or the
rule for half-lives of at most 100 seconds (paragraph 7). The schedule applies in England and Wales only.

### Fetter limits (`fetter-specific-activity`)

Table 2 of S. Fetter, E.T. Cheng and F.M. Mann, "Long-term radioactive waste from fusion reactors: Part II",
Fusion Engineering and Design 13 (1990) 239-246, gives specific-activity limits in Ci/m3 for Class C disposal of
activated metal, for 101 radionuclides with half-lives from 5 years to 10^12 years. The paper tabulates the limits
and does not define a named rating from them, so the result is the sum of fractions (concentration over limit).
The Nb-94 limit is 0.2 Ci/m3, the same as the activated-metal row of 10 CFR 61.55 Table 1. A limit given as a range
runs from the waste-form indices for non-fuel reactor components (lower, used as the limit) to those for
high-activity industrial waste (upper); the result gives the sum at the lower limits and at the upper values, and
`SUM_AT_OR_BELOW_1_AT_UPPER_RANGE_ONLY` means the verdict depends on the waste form. A limit of TMSA (the theoretical
maximum specific activity of the pure nuclide) cannot be reached and adds nothing. The waste form must be activated
metal and `volume_m3` is needed. Nuclides the inventory does not list count as absent: a sum at or below 1 holds for
the listed nuclides, and `inventory_lists_every_limited_nuclide` says whether the inventory covers the whole table.

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

`schemes` lists every scheme with name, version, date and citation; all five are implemented. `faris waste
tables --scheme <scheme>` prints the encoded rows with their source, retrieval date and licence. Every value was
read from a retrieved source and is stored with the source's hash, and a test compares each encoded row with the
stored copy:

| Scheme | Source | Kind | Retrieved | Stored copy |
| --- | --- | --- | --- | --- |
| `nrc-10cfr61.55` | eCFR text of 10 CFR 61.55 (title 10 current through 2026-10-13) | primary | 2026-10-15 | `ecfr_10cfr61.55.xml`, SHA-256 recorded |
| `iaea-gsr3-table-i2` | Table I.2, pages 124 to 127, of GSR Part 3 (STI/PUB/1578) | primary | 2026-10-15 | page numbers and the hash of the PDF |
| `uk-llw-ilw` | UK LLW strategy, February 2016, and the 2007 Defra/DTI policy (gov.uk) | primary | 2026-10-16 | verbatim extracts `uk_llw_policy_extracts.txt` and the hash of each PDF |
| `uk-epr2016-sch23-table2` | SI 2016/1154 Schedule 23 Part 3, legislation.gov.uk | primary | 2026-10-16 | `uk_epr2016_sch23_part3.xml` and the hash of the downloaded document |
| `fetter-specific-activity` | Fetter, Cheng and Mann (1990) Table 2, copy hosted by S. Fetter at the University of Maryland | primary | 2026-10-16 | text of Table 2 `fetter_1990_part2_table2.txt` and the hash of the PDF |

The files are in `crates/faris-model/data/waste/`. The Fetter paper is a journal article: FARIS stores only the text of
its Table 2 for checking, not the article.

Component totals by class over a plant life (waste mass and volume per class) are not computed here.
