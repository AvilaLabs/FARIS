# Documentation and learning

What FARIS teaches and how it proves the teaching is right. Documentation counts only if it
is complete against the product, runs in CI, matches the release and is read at the level
promised. Page counts are never evidence. Usability studies that test learning are in
[09-usability.md](09-usability.md); accessibility of the documents themselves is in
[10-accessibility-and-localisation.md](10-accessibility-and-localisation.md). Reference
hardware is in [the index](README.md#reference-hardware-and-models).

Repository check (2026-10-05): `docs/` holds about 20 Markdown files (architecture, demo
walkthrough and roadmap, study file and export, transport, operating history, scientific
and material baselines, numerical controls, third-party notices) written for developers
and reviewers. There is no changelog, no citation file, no user manual, no theory manual,
no generated API reference, no `missing_docs` lint and no documentation job in CI. CI runs
format, Clippy, workspace tests and the Python control tests. The first-run tour exists in
the app.

## Document set

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-001 | FARIS shall ship four kinds of documentation, kept apart: tutorials, how-to guides, reference and explanation. | Every page declares its kind in its front matter; each of the four kinds exists for every feature; no page mixes kinds (checked by a page template lint). | Docs build with the kind lint; coverage script (DOC-020). | Diataxis four forms [V]([Diataxis](https://diataxis.fr/)) | F1 | Partial: developer and reviewer documents exist, unclassified |
| DOC-002 | FARIS shall ship a user manual. | Covers every UI panel, every command and every file the user touches; written to the task, not to the code; ≥ 1 how-to per core task (UX-010). | Doc coverage script; review against the command registry. | SCALE ships a manual per module [V]([SCALE 6.3 manual](https://scale-manual.ornl.gov/6.3.2/Introduction.html)) | F1 | No |
| DOC-003 | FARIS shall ship a theory manual. | One chapter per physics and plant model, each with equations, assumptions, validity range, units, kind labels and references; every equation in the code carries a manual link and every manual equation a code link. | Link check between model registry and manual; review. | SCALE manual per module has theory, input, V&V and examples [V]([SCALE 6.3 manual](https://scale-manual.ornl.gov/6.3.2/Introduction.html)) | F2 | Partial: scientific baseline, transport, operating history and material baselines documents exist |
| DOC-004 | FARIS shall ship a verification and validation report with every release. | Per model: benchmarks run, result, tolerance, pass or fail, hash of the evidence; generated from the same data the checker uses (VAL, QA); no hand-typed verdicts. | Generator run in release CI; diff against checker output. | Verdicts come from checkers [internal]; SCALE per-module V&V [V]([SCALE 6.3 manual](https://scale-manual.ornl.gov/6.3.2/Introduction.html)) | F2 | Partial: numerical controls and cold-reference documents exist; not generated |
| DOC-005 | FARIS shall ship a reference for every CLI command. | 100 % of subcommands, flags, exit codes and output files documented; each has an executed example; help text and manual generated from one source. | CLI-to-doc diff in CI. | Rust `clap` help as source [internal] | F1 | Partial: `--help` text exists; no manual |
| DOC-006 | FARIS shall ship an API reference. | 100 % of public Rust items and public Python items documented; build fails on a missing doc comment (`missing_docs` deny); doctests run. | `cargo doc` with deny and `cargo test --doc` in CI; Python docstring lint. | Rust doc practice, docs.rs [U] | F1 | No: no `missing_docs` lint |
| DOC-007 | FARIS shall document every file format. | Study file, scenario, CSV exports, receipt and plugin manifests have a schema, a prose description and a versioned example; the schema is the one the code reads. | Schema-to-doc check; example parses in CI. | FARIS choice | F1 | Partial: study file and export documents exist |
| DOC-008 | FARIS shall ship tutorials by study type. | ≥ 1 worked example per study type (design comparison, sensitivity, sweep, operating history, activation, shielding, optimisation as each phase lands); ≥ 5 tutorials at F2, ≥ 10 at F6, each with expected results and their kind labels. | Tutorial runs in CI (DOC-020); count from the front matter. | Diataxis [V]([Diataxis](https://diataxis.fr/)); FARIS choice | F2 (5), F6 (10) | Partial: one demo walkthrough |
| DOC-009 | FARIS shall publish a known-limitations page. | One page listing every known model gap, missing validation and unsupported feature, each with a requirement ID, a date and the workaround; linked from the Help menu and every report footer; 0 undocumented limitation found in review at release. | Page-to-registry check; release review. | Honest-limits rule [internal] | F1 | Partial: limitations appear in demo caveats and baselines, not on one page |

## Examples, coverage and rot

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-020 | Every example shall execute in CI. | 100 % of code blocks, CLI snippets and tutorial steps run against the current build, output compared with the stated result; a failing example fails the build. | Doc-test runner in CI over the manual sources. | Rust doctests; tutorials rot silently unless run [R3] | F1 | No |
| DOC-021 | Every CLI command shall be documented. | 100 % of subcommands and flags appear in the reference with an example. | Coverage script comparing `--help` output to the manual. | FARIS choice | F1 | No |
| DOC-022 | Every public API symbol shall be documented. | 100 % of public Rust and Python symbols have a description, units where relevant and an example for ≥ 50 %. | `deny(missing_docs)`; example-count script. | Rust practice [U] | F1 | No |
| DOC-023 | Every UI panel shall be documented. | 100 % of panels, tabs and dialogs listed in the UI inventory have a manual section and a current screenshot. | UI inventory from the registry compared with the manual. | FARIS choice | F1 | Partial: demo walkthrough covers the main panels |
| DOC-024 | Every requirement-relevant model shall have a page. | 100 % of models named in files 01–05 have a theory page with sources and validation status linked to receipts. | Model registry compared with the manual. | Domain credibility [R3] | F2 | Partial |
| DOC-025 | Screenshots shall never be stale. | Manual screenshots regenerated automatically each release from the app's screenshot mode; a pixel diff above 2 % on any page fails the release check until reviewed. | Screenshot job in release CI. | FARIS choice; app has a capture mode [internal] | F2 | Partial: docs images were captured from the app by script; not automated per release |
| DOC-026 | Links shall not break. | 0 broken internal or external links at release; external links checked weekly. | Link checker in CI. | FARIS choice | F1 | No |
| DOC-027 | Examples shall use real, labelled numbers. | 100 % of numbers in examples carry kind labels and come from a recorded run or an authored input marked as such; 0 invented values. | Example-source lint. | House rule: no invented values [internal] | F1 | Unmeasured |
| DOC-028 | Every error code in the error catalogue shall have a page. | 100 % of catalogue errors (UX-044) link to a page with cause, fix and an example. | Catalogue-to-doc check. | NN/g error guidance [U]([NN/g](https://www.nngroup.com/articles/error-message-guidelines/)) | F2 | No |
| DOC-029 | Documentation coverage shall be reported by content, not by count. | Coverage report lists each requirement ID with its documents and the executed tests that touch it (QA-002); a page that exists but lacks an executed example counts as uncovered. | Trace matrix. | Trap: page counts can be gamed [R3] | F2 | No |

## Delivery, versions and search

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-030 | Docs shall be versioned with releases. | Every release has its own frozen documentation; a version selector; the in-app Help opens the manual for the running version; older versions stay available for ≥ 5 years. | Release job; link test from a built app. | docs.rs per-version practice [U] | F1 | No |
| DOC-031 | Docs shall work offline. | Full manual bundled with the app, ≤ 30 MB, searchable and opening ≤ 1 s with the network off. | Offline test; package size check. | FARIS choice | F2 | No |
| DOC-032 | Documentation shall be searchable. | Full-text search with the top-3 hit correct for ≥ 90 % of 50 benchmark queries; searches glossary and error codes; works offline. | Query set run in CI. | FARIS choice | F2 | No |
| DOC-033 | In-app help shall link to the docs. | 100 % of parameters, results, panels and error codes open their manual section from the UI (F1 key and a link), same version as the app; see UX-052. | Registry lint; link test. | Blender F1 manual [U] | F2 | No |
| DOC-034 | FARIS shall have a glossary. | Every term in the terminology table (UX-040) defined once, ≤ 40 words, with a link to the theory page; hover on a term in the manual shows it; glossary is the source for the UI's help text. | Glossary lint against the terminology table. | FARIS choice | F2 | No |
| DOC-035 | Documentation shall be accessible. | Manual passes the automated and manual checks in A11Y-050 to A11Y-054 for web content; images have alt text; PDF editions are tagged (A11Y-047). | Accessibility audit of the built site and PDF. | WCAG [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F4 | No |
| DOC-036 | Documentation shall be built from source in the repository. | One command rebuilds the whole manual; builds are reproducible (same input, same output hash); no content lives outside version control. | Reproducible-build check. | FARIS choice | F1 | No |
| DOC-037 | The manual shall be published under a clear licence. | Documentation licence stated in every page footer, matching the repository licence (AGPL-3.0-only); third-party text and figures credited. | Licence lint. | Repository licence [internal] | F1 | Partial: licence stated for the repository |

## Releases, changes and migration

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-040 | Every release shall have a user-facing changelog. | Each version lists added, changed, fixed and removed items; breaking changes and any change that alters numerical results are flagged at the top, with the size of the change in the benchmark set. | Release checklist; script comparing benchmark results between versions. | Keep a Changelog convention [U] | F1 | No: no changelog |
| DOC-041 | Every breaking change shall have a migration guide. | 100 % of breaking changes (file format, CLI, API, result values) link to a guide with before and after examples that run in CI. | Link check; example run. | FARIS choice | F1 | No |
| DOC-042 | Old study files shall open, or fail with a way forward. | Files from the last 5 releases open or fail with a named reason and the migration tool or version to use; 100 % of fixture files covered. | Fixture test (see INT and REL for format guarantees). | Fail-closed rule [internal] | F2 | Partial: strict reading with versioned schemas; no migration tooling |
| DOC-043 | Deprecations shall be announced. | A deprecated item stays for ≥ 2 minor releases with a warning that names the replacement; the removal release is stated when announced. | Deprecation lint. | FARIS choice | F2 | No |
| DOC-044 | Result changes between versions shall be explained. | A "results changed" table per release lists each reference case whose number moved beyond its tolerance, with the cause. | Generated from benchmark comparison. | Reproducibility rule [internal] | F2 | No |

## Learning outcomes and readability

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-050 | A new user shall complete the first tutorial in a stated time. | Median ≤ 20 min and P90 ≤ 35 min from opening the tutorial to the final check (provisional: no engineering-software reference; confirm before F2 gate); success ≥ 90 % (n ≥ 10). | Timed study per UX-070. | Time to first hello world convention [U] | F2 | No |
| DOC-051 | The first tutorial shall teach reading uncertainty. | After it, ≥ 80 % of n ≥ 10 new users answer 5 comprehension questions correctly and complete a modified run unaided. | Study per UX-070. | UX learning outcome [R3] | F2 | No |
| DOC-052 | Users shall find the answer in the docs quickly. | ≥ 80 % of n ≥ 10 participants find the right page for 10 scripted questions in ≤ 2 min each using search and navigation. | Moderated study. | FARIS choice | F2 | No |
| DOC-053 | Prose shall be readable. | Measure: Flesch-Kincaid grade level computed with a named tool on prose only (code, tables and equations excluded); user manual ≤ grade 12, tutorials ≤ grade 10, mean sentence ≤ 20 words, passive voice ≤ 15 % of sentences; theory manual ≤ grade 16 with every symbol defined at first use. | Readability script in CI; tool and version recorded. | FARIS choice; readability scores are rough guides [U] | F2 | No |
| DOC-054 | Writing shall follow one style. | Plain English, British spelling, short sentences, one name per thing; style lint (spelling and terminology) passes with 0 errors. | Prose linter in CI. | Project style rule [internal] | F1 | No |
| DOC-055 | Video tutorials shall be accessible and short. | ≥ 3 videos of ≤ 5 min at F4; each with captions, transcript and a text version of every step; no step exists only in video. | Asset check; caption check. | FARIS choice | F4 | No |
| DOC-056 | The guided tour shall link to the manual. | Each tour stop links to its manual section; tour text and manual text come from the same source (no drift). | Source-identity check. | FARIS choice | F2 | Partial: tour exists with replay; text is separate |
| DOC-057 | Documentation quality shall be measured over time. | Per release: broken links, stale screenshots, uncovered symbols, failed examples, readability grade and search success reported on one page; none worse than the previous release without a note. | Release dashboard. | FARIS choice | F2 | No |

## Citation and support

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DOC-060 | FARIS shall say how to cite it. | A machine-readable citation file in the repository (Citation File Format) and a "How to cite" page giving the software version, DOI at release, authors and licence; the same text in the app's About box and in every report footer. | File validation; text identity check. | Citation File Format [U]; FARIS choice | F1 | No |
| DOC-061 | FARIS shall say how to cite its data and models. | Each model and data library (nuclear data, FENDL, ENDF/B, benchmark sets) listed with its own reference, version and licence on a page and in exports; exports include the list of references actually used. | Export contains the reference list; page-to-registry check. | Provenance rule [internal]; nuclear-data licences recorded in third-party notices [internal] | F2 | Partial: third-party notices exist; no per-export reference list |
| DOC-062 | Reports shall state what they are. | Every report and export carries the statement that FARIS outputs are research screening, not a licensing or safety claim, and that FARIS is a design model, not a digital twin. | Export text check. | House rule [internal] | F1 | Partial: caveats in the brief; exact statement not checked |
| DOC-063 | Support routes shall be documented. | Public issue tracker, security contact, and a first-response target of ≤ 5 business days (provisional: solo maintainer; confirm before F6 gate); the page states what is and is not supported. | Process audit. | FARIS choice | F6 | No |
| DOC-064 | A citation shall be reproducible. | The cited version, its docs, its checked-in test data hashes and its example outputs remain downloadable for ≥ 10 years or the page says when they will not. | Archive check. | FARIS choice | F6 | No |

## Traps

- Page counts and coverage by kind look good while pages stay thin. Count executed examples and covered requirements instead.
- Tutorials rot without CI. A tutorial that passed last year and fails today is worse than none.
- Reference generated from code is complete and unreadable. Keep explanation and how-to written by hand, and test them with users.
- Readability grade rewards short words, not correct physics. Never trade a defined technical term for a vaguer one to lower the score.
- Search that works on titles passes small tests and fails on real questions. Test with the 50 questions people actually ask.
- "Docs versioned" means little if the in-app link goes to the latest page. Test the link from an old build.
- A V&V report that is hand-written drifts from the checker. Generate it.
- Screenshots refreshed only when someone remembers will be stale at the next UI change.

