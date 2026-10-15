<!-- kind: explanation -->
# Export-control statement

> **DRAFT for owner and counsel review.** No lawyer or export-control specialist has reviewed this text. It is a
> starting point for that review (requirements LEG-050 and LEG-052), not legal advice and not a determination of
> any kind. Do not publish it on the website or in the app as final until the review has happened and this box
> has been replaced with the date, the reviewer and the outcome.

## What FARIS does

FARIS is open-source software (AGPL-3.0-only) for early screening of fusion plant designs. It runs calculations
that the user starts, on the user's own computer, from inputs the user supplies or that ship with a demo study.
It does not connect to a service to compute anything, and it sends no study content anywhere
(`docs/guide/account.md`).

## What FARIS does not do

- **It does not classify anything.** FARIS does not decide whether a design, a geometry, a result, a data set or
  a file is controlled under any export-control regime (for example the U.S. Export Administration Regulations,
  10 CFR Part 110 or Part 810, or the EU dual-use regulation). It contains no classification list and makes no
  determination about any item.
- **It does not mark or hide controlled material by itself.** The author of a study can be asked to mark it as
  restricted and to be warned before sharing it (requirements SEC-070 and SEC-071); until those are built, FARIS
  has no such mark, and nothing in a file says whether the file is controlled.
- **It does not make a study, a result or a design safe to export.** A result being produced by FARIS says nothing
  about whether the inputs, the geometry or the output may be sent to another person or country.

## Who is responsible

Users are responsible for their own studies. Before you share a study, a design file, a geometry, a result or an
evidence archive across a border, or with a person who is not a citizen or resident of your country, you decide
whether it is controlled and whether you may send it. FARIS gives you no answer to that question. The demo study
that ships with FARIS is an idealised, authored example and is not a real machine's design
(`docs/guide/scope.md`).

## What ships with FARIS

| Item | Control status |
| --- | --- |
| FARIS source code and documentation (AGPL-3.0-only) | Not reviewed |
| Demo study and its recorded transport and history results | Not reviewed |
| Bundled data files (`scripts/nuclear_data_in_git.json`) and fonts | Not reviewed; provenance and redistribution terms are recorded per file |
| Files FARIS generates for your inputs | The inputs' status applies; not reviewed |

FARIS does not bundle nuclear data, tools or geometry that are access-controlled or that may not be
redistributed (requirement SEC-072); larger nuclear-data libraries are fetched by hash from their providers,
under the providers' own terms.

## Release-notes line (LEG-052)

Every release's notes carry this line, with the status filled in from the review:

> Export-control review status of the code, the bundled data and the generated files: NOT REVIEWED (draft
> statement in docs/EXPORT_CONTROL.md awaits owner and counsel review).

`scripts/release_notes.py` writes it and refuses to produce notes without it.

## Questions for counsel

1. Does publishing the source and the demo study under an open-source licence fall under the published-information
   or public-domain exclusions in the jurisdictions where Avila Labs operates, and does that change for any
   functionality that is added later (for example neutron-transport models of specific designs)?
2. Does the software, or any input class it reads, need a classification statement, or is "no determination
   made" an acceptable public position?
3. What wording should the website and the app use, and does it need to differ by jurisdiction?
4. Does running FARIS for a user in another country, through the optional sign-in, create any obligation?
