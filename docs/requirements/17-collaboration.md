# Collaboration

How people share, review, compare, merge and approve FARIS studies. FARIS is offline-first and single-user today; this file sets what a team needs, in the order it can be built: read-only bundles and diff first, then history and merge, then review workflows, then concurrent editing. Verdicts in any review are derived by checkers, never typed in by a person. See the [index](README.md#reference-hardware-and-models) for RL, RW and the reference models RM-S, RM-M and RM-L. Account and sign-in behaviour is owned by the security file (SEC-040 onward); this file only uses it.

## Sharing

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-001 | FARIS shall share a study as one self-contained file that opens on a clean machine with only FARIS and the declared adapters and data. | 100 % of golden studies open on a clean virtual machine; a missing adapter or data library is named precisely. | Clean-virtual-machine test. | R4 COL-7; RO-Crate idea [U] | F1 | Partial: .faris is one file; referenced evidence needs archives supplied; no clean-machine test |
| COL-002 | FARIS shall create a read-only bundle that can be opened and inspected but not edited or re-saved as a new authoritative study. | Bundle opens read-only in 100 % of tests; every edit control is disabled with a stated reason; saving a copy requires an explicit "make editable copy" step that records the origin hash. | UI automation over every edit control. | R6 collaboration row (COMSOL Model Manager is the closest analogue [U]) | F1 | No |
| COL-003 | A read-only bundle shall carry its own integrity: opening it shall verify every blob and the bundle manifest, and refuse a damaged bundle with a message naming the failing item. | 100 % of single-byte corruptions refused (see REL-020). | Corruption test. | docs/STUDY_FILE.md reading rules | F1 | Partial: blobs hashed on read; no separate bundle type |
| COL-004 | FARIS shall let the sender strip named blobs (for example licensed data or proprietary geometry) and leave their hash references. | 100 % of named blobs removed; hash references kept; recipient sees which items are missing and why. | Redaction test with a pattern list. | R4 COL-11 | F1 | No: referenced evidence mode exists for archives only |
| COL-005 | Sharing shall not require an account or a network connection. | 100 % of sharing features work with the network disabled and no sign-in. | Test in a network-less namespace. | Offline-first (SEC-050) | F1 | Met: .faris and export folder are plain files; sign-in is optional (crates/faris-app/src/main.rs) |
| COL-006 | A bundle for reviewers without FARIS shall be a PDF brief and CSV folder that name the study hash. | 100 % of bundles pass the reader-without-FARIS checklist (PRV-056). | Checklist script. | docs/STUDY_EXPORT.md | F1 | Met: 2-page PDF brief, CSV, SVG/PNG and export-manifest.json, stamped with the study sha256 (0.53 s); no CLI export yet |
| COL-007 | The export shall be available from the CLI as well as the desktop. | `faris export` produces output byte-identical (modulo timestamp) to the desktop export. | Parity test. | AGENTS.md CLI parity; PRV-031 | F1 | No: CLI export is missing |

## Comments and annotations

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-010 | FARIS shall store comments anchored to a parameter, a result number, a component or a chart, and keep them in the study file. | 100 % of anchor types supported; a comment survives save, reopen and a change to unrelated parameters. | Round-trip test per anchor type. | R4 COL-8 | F1 | No |
| COL-011 | A comment shall record author, time, the study hash it was written against and the value it referred to. | 100 % of comments carry all four fields. | Schema test. | FARIS choice | F1 | No |
| COL-012 | When the anchored value changes, the comment shall show as "refers to an earlier value" with old and new value, not vanish or attach silently to the new one. | 100 % of changed anchors flagged in a mutation test. | Edit every anchored parameter in turn. | FARIS choice | F1 | No |
| COL-013 | Comments shall not change any result, receipt or hash of the calculation. | Result and receipt hashes identical with and without comments over all golden studies. | Hash comparison test. | AUTO-033 principle: only inputs enter the cache key | F1 | No |
| COL-014 | Comments shall be diffable, filterable (open, resolved, by author) and exportable into the PDF brief. | Filters return exact counts in test; export lists 100 % of open comments. | UI and export test. | R3 review practice [U] | F2 | No |
| COL-015 | Resolving a comment shall be a recorded action with who and when, and shall not delete the thread. | 100 % of resolutions logged; thread retrievable. | Test. | PRV-007 | F2 | No |

## Semantic diff

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-020 | FARIS shall compare two .faris files semantically and list every changed parameter with old value, new value and unit. | 100 % of changed parameters listed in a seeded-change test of at least 200 edits; 0 false "changed" on a re-save of an unchanged study. | Property test: apply random edits, expect exactly those edits in the diff. | R4 COL-1; R6 collaboration row; nbdime idea [U] | F1 | No |
| COL-021 | The diff shall also list every changed result with old value, new value, change in standard errors and the kind label of each side. | 100 % of changed results listed; a change below 2 sigma is shown as "within noise" not as a difference. | Seeded test with Monte Carlo reruns on different seeds. | Existing 2-sigma compare flags | F1 | Partial: compare view flags 2-sigma contrasts between arrangements; no file-to-file diff |
| COL-022 | The diff shall separate causes from effects: inputs changed, evidence changed, results changed, labels changed. | 4 sections present; each result change links to at least one input change or is marked "no input change found". | Test over fixture pairs. | FARIS choice | F1 | No |
| COL-023 | The diff shall be available from the CLI as `faris diff a b` with stable, versioned JSON, and from the desktop. | Same content in both; JSON schema versioned and validated. | Parity test. | AGENTS.md CLI parity; R4 COL-1 | F1 | No |
| COL-024 | The diff shall take at most 2 s for two RM-S studies and 10 s for two 1 GB studies on RL (provisional: no reference; confirm before F2 gate). | P95 within limits over 20 runs. | Benchmark. | FARIS choice | F2 | No |
| COL-025 | The diff shall treat float formatting, key order and locale as non-changes. | 0 false changes across 1,000 reorderings and reformattings. | Property test. | R4 trap 5 | F1 | No |

## History, restore and merge

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-030 | FARIS shall keep a project version history with named milestones. | Every save creates a version; a milestone name can be attached to any version; list view shows author, time, hash. | Test. | R4 COL-5; Onshape version graph [U] | F2 | No |
| COL-031 | Restoring a version shall reproduce the original study file hash exactly. | 100 % of restored versions have a SHA-256 equal to the recorded one over 1,000 random restores. | Restore and compare test. | Content-addressed container (docs/STUDY_FILE.md) | F2 | No |
| COL-032 | Restore shall take at most 2 s for RM-S and 10 s for a 1 GB study on RL (provisional: no reference; confirm before F2 gate). | P95 within limits over 20 runs. | Benchmark. | R4 COL-5 [U] | F2 | No |
| COL-033 | Branching a scenario variant shall be cheap: it shares blobs with its parent and costs time and disk independent of data size. | Branch creation under 100 ms and under 1 MB extra disk for a 1 GB study. | Benchmark. | R4 COL-4 | F2 | No |
| COL-034 | The history shall be tamper-evident (see PRV-006): deleting or editing a past version without a trace shall be impossible. | 100 % of injected edits detected. | Tamper test. | R4 COL-6 | F4 | No |
| COL-035 | FARIS shall merge two variants of a study three-way: independent parameter edits merge automatically and conflicting edits are reported with both values and the common ancestor value. | 100 % of a corpus of at least 30 merge cases give the expected result; 0 silent overwrites. | Merge corpus test. | R4 COL-2 | F2 | No |
| COL-036 | A merged study whose inputs differ from either parent shall have no inherited results: affected results are invalidated and shown as not-evaluated with a re-run action. | 100 % of results depending on a merged input are invalidated. | Test over the merge corpus. | House rule: fail closed; AUTO-033 | F2 | No |
| COL-037 | Single-parameter edits shall give small, reviewable changes in the text part of the file. | A one-parameter edit changes at most 1 line of canonical text and at most 1 blob, and no timestamp appears in canonical text. | Test. | R4 COL-3 | F2 | Partial: manifest is pretty-printed JSON with content-addressed blobs; ordering and timestamp rules untested |
| COL-038 | Writing a study from two processes at once shall never silently lose an update. | 0 lost updates in a 2-process write-race test of 1,000 rounds. | Race test. | R4 COL-9 | F1 | Partial: saves are atomic (temp file, fsync, rename) so the file is never torn; the later writer wins silently |

## Review and approval

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-040 | FARIS shall support a review workflow with states: draft, in review, changes requested, approved. | States and transitions exist; an invalid transition is refused with a reason. | State-machine test. | FARIS choice | F6 | No |
| COL-041 | Approval shall be permitted only when the checkers return no blocking finding: receipts verified, no missing or stale evidence, no open defect notice (PRV-051). | 0 approvals recorded while a blocking finding exists; approval record cites the checker output hash. | Negative tests for every blocking class. | House rule: verdicts are derived by checkers | F6 | No |
| COL-042 | An approval shall be bound to a study hash and become void when the study changes. | 100 % of edits void a prior approval in test. | Mutation test. | PRV-003 | F6 | No |
| COL-043 | Reviewer actions (comment, request changes, approve) shall be logged with identity and time in the tamper-evident log. | 100 % of actions logged. | Test. | PRV-007 | F6 | No |
| COL-044 | The approval stamp shall say "reviewed within research screening" and never state a licensing or safety conclusion. | 0 occurrences of licensing or safety wording in approval text (see LEG-041). | Wording lint. | AGENTS.md; R5 VV-18 | F6 | No |
| COL-045 | A reviewer shall be able to see exactly what changed since the last version they reviewed (the diff of COL-020). | 100 % of reviews open on the diff since their last review. | UI test. | FARIS choice | F6 | No |

## Roles and concurrent editing

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| COL-050 | FARIS shall define roles: owner, editor, reviewer, viewer, each with a published permission table. | 100 % of actions mapped to a minimum role; a lower role is refused with a reason. | Permission matrix test. | FARIS choice | F6 | No |
| COL-051 | Permissions shall be enforced by the data layer, not only hidden in the UI. | 0 actions succeed through the CLI or API when refused in the UI. | Parity test across clients. | AGENTS.md: UI must not own semantics | F6 | No |
| COL-052 | FARIS shall work offline-first: every collaboration feature that does not need a second person works with no network, and queued changes sync later without loss. | 100 % of offline edits present after sync in a 100-round partition test; 0 silent overwrites. | Partition test with two clients. | Offline-first stance; R4 section 1.9 | F6 | No |
| COL-053 | Multi-user concurrent editing shall converge: all clients reach the same state and the same study hash after the same set of edits, in any order. | 0 divergences in 10,000 random interleavings (provisional: structure of merge or CRDT undecided; confirm before F6 gate). | Property test. | CRDT practice (Automerge, Yjs) [U] | F7 | No |
| COL-054 | Concurrent editing shall never apply to results: only inputs are shared state, and each user's results are recomputed or fetched by hash. | 0 shared mutable result objects. | Architecture test. | AUTO-033; PRV-003 | F7 | No |
| COL-055 | Presence and live cursors shall be optional and off by default. | Default off; no network traffic from presence when off. | Network trace. | Privacy by default [U] | F7 | No |

## Traps

- Diff completeness is measured by seeded edits, not by counting fields. A diff that hides a unit change or a default change passes a field count.
- A merge that reuses results across merged inputs looks fast and is wrong. Invalidate dependent results, then count re-runs.
- Real-time co-editing is a large cost for a tool whose work is long runs. Do not let "multi-user" displace single-user review quality.
- Comments anchored by position drift. Anchor to an object identity and the value hash.
- An approval that survives an edit is worse than none. Bind it to the hash.
- File locks protect nothing across a sync folder. Test with the sync tool actually in use.
- Roles hidden only in the UI are bypassed by the CLI.
