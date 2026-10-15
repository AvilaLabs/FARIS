<!-- kind: reference -->
# Archived study-file fixtures

Small `.faris` files, each with a `*.summary.json` beside it, that every later
version of FARIS must keep opening (PRV-053, DOC-042) and that the independent
reader `scripts/read_study_file.py` must agree on (INT-001).

They were written by the writer in the commit that added them (format
`faris-study/1`, on top of release 0.2.0) with `cargo test -p faris-study regenerate_archived_fixtures
-- --ignored`, except the first three, which are hand-built containers.
Between the releases v0.1.0 and v0.2.0 the manifest module did not change
(`git diff v0.1.0 v0.2.0 -- crates/faris-study/src/manifest.rs` is empty), so a
file from any released version has the manifest shapes the set covers. The
evidence-store layer and the preview thumbnail were added after v0.2.0; the
fixtures `03`, `16`, `20`, `21` and `24` cover them.

Frozen means frozen: do not regenerate a fixture to make a test pass. When a
release changes the format or adds a layer, run the generator once for the new
files and commit them next to the old ones; a new major version gets its own
fixtures here and keeps these.

| Fixture | Covers |
| --- | --- |
| 01 hand-built minimal | stored container, one blob, nothing else |
| 02 newer minor, unknown fields | `faris-study/1.4` with unknown manifest and view properties |
| 03 minor 1.1, store layer referenced | `layers.evidence_store` with no trees, `evidence: null` |
| 04 empty study | no blobs |
| 05 assumptions only | one blob, no arrangements |
| 06 view only | view state, no blobs |
| 07 to 09 | one bundle; two bundles sharing blobs; control only |
| 10 | both arrangements, sweep and assumptions (the usual shape) |
| 11 | sweep only, recorded file with a trailing newline |
| 12 | optional `solver/transport-batch-values.json` member |
| 13 | scenario and physics blobs |
| 14, 15 | zstd levels 1 and 19 |
| 16 | `preview.png` thumbnail |
| 17 | edited what-if assumptions in the view |
| 18, 19 | Core evidence as archives, referenced and packed |
| 20, 21 | Core evidence as store trees, referenced and packed (embedded `evidence-store/`) |
| 22, 23 | history ensembles; a not-evaluated ensemble with its reasons |
| 24 | archives packed, thumbnail, ensemble and what-if together |
