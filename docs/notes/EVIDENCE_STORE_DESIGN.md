# Core evidence in one content-addressed store

Status: decided 2026-10-08. Implements Avila Core ADR-0028
(`avila.core/evidence-store/v0.1`) on the FARIS side.

## Problem

The 0.2.0 package ships Core evidence as eight tar.gz archives (four cases,
four workspaces). The app expands them into a private temporary folder at
launch: 823 MB, needing about 0.9 GB free, behind a 10-minute wait limit.
`verify.sh` needs about 2 GB. Only 296 MB of the 823 MB is distinct content,
and it compresses to about 14 MB. The FARIS stage contract v0.2 (a985b38)
already halves the case and workspace sizes; this change removes the
duplication and the expansion.

## Decisions

1. **One store, eight trees.** The package's evidence part carries one store
   directory, `evidence-store/` (`store.json` + `blobs/<h0h1>/<h>.xz`), in the
   ADR-0028 format, holding the trees `control-reference-case`,
   `control-reference-workspace`, `control-breeder-emphasis-case`,
   `control-breeder-emphasis-workspace`, and the same four for `port`. The
   packager packs once, after all four cases exist, so identical files across
   all trees share one blob. The eight tar.gz archives and their
   `.manifest.json` files are no longer produced.
2. **Index every store file.** `store.json` and every blob are ordinary
   indexed package files (`files[]`, part `evidence`, sha256 of the file as
   stored). About a hundred blobs fit far inside the 2,048-file cap, so the
   exact-inventory rule, `make_release.py` and `retarget_package.py` keep
   working unchanged. The package index records the store: path,
   `store.json` sha256, tree names with file count, uncompressed bytes and
   implicit directory count, blob count, stored bytes, distinct bytes.
   Per-arrangement records name their `case_tree` and `workspace_tree`.
   The package index schema becomes `faris-recorded-demo-package/v0.6`; the
   app opens v0.6 only and refuses older indexes with a clear message (the app
   always ships inside its own package). The package-root saved-study
   descriptor becomes `faris-saved-study-store/v0.1`, naming the two trees.
3. **Independent FARIS implementation of the format.** FARIS reads and
   writes the store with its own code, as it already verifies Core evidence
   independently: Python `scripts/evidence_store.py` (standard library:
   `lzma`, `hashlib`, `json`) for packing, reading and verifying, and a Rust
   reader in `faris-engine` (`xz2 = "=0.1.7"`, `static`, the same pin as
   Core). Every ADR-0028 rule applies: bounded decompression to `bytes`+1,
   length and SHA-256 checked before content is accepted, symlink or
   non-regular blobs refused, trailing data refused. Cross-check: a
   FARIS-written store passes `avila-core store verify` and Core's
   `verifier/store_verify.py`.
4. **Inspect in place, no expansion.** `inspect_saved_case` becomes generic
   over a tree source with `read(relative, max) -> bytes`, `contains`, and
   index-derived limits. Two sources: directory (current behaviour, kept for
   the CLI, tests and legacy `.faris` files) and store tree. Directory checks
   map as follows: the tree must exist in the index (not `canonicalize`/
   `is_dir`); `validate_tree_limits` becomes arithmetic over the index
   entries; `.exists()` becomes an index lookup. Reads are memoized by
   digest, because several files are read more than once. The execution
   report is `execution-report.json` inside the case tree.
   `faris evidence inspect` gains `--store DIR --case-tree NAME
   --workspace-tree NAME`, keeping the directory form.
5. **The app.** Opening a package opens the store and inspects the four
   saved cases on the existing worker thread. The package path no longer
   uses a temporary folder, the `Materializer`, the completion marker or the
   10-minute limit; messages about temporary space go away. The marker flow
   stays only for legacy `.faris` files that pack tar.gz evidence.
   `--check-package` inspects from the store.
6. **verify.sh.** It verifies the store (Python), inspects every saved case
   from the store through `faris evidence inspect --store ...`, and unpacks
   only one case tree at a time to a temporary directory for `avila-core
   export`, which needs a real directory until Core can verify in place. The
   space check becomes: relocated package copy + largest case tree + its
   export copy + 64 MiB. The tamper control flips a byte in a blob and must be
   rejected.
7. **`.faris` files.** Old files keep opening: packed tar.gz evidence still
   expands through the legacy path; referenced evidence still resolves beside
   the file, and is reported as not included when the archives are absent
   (an existing, explained state). New evidence records name store trees; see
   the follow-up below.

## Decision 7 follow-up: `.faris` evidence records for store trees

Decided 2026-10-08; the format is specified in `docs/STUDY_FILE.md` ("Evidence
store layer").

- A new optional manifest layer, `layers.evidence_store`: `mode` (`packed` or
  `referenced`) and `trees`, each tree `{arrangement, allocation, kind, tree,
  files[{path, sha256, bytes}]}` sorted. The full listing is the tree's identity.
  Older readers ignore the layer (no `deny_unknown_fields`) and show the evidence
  as not included. A file has the old `layers.evidence` or the new layer, never
  both. Files carrying tar.gz evidence keep their writer and reader paths and are
  not converted.
- Packed: the zip carries a valid ADR-0028 store of exactly those trees
  (`evidence-store/store.json`, `evidence-store/blobs/<h0h1>/<h>.xz`) as stored
  entries. The writer copies verified `.xz` bytes from the source store and does
  not recompress. On open the store is extracted (bounded) into the run workspace,
  verified with the FARIS verifier, matched against the manifest listings, and the
  app inspects the saved cases in place.
- Referenced: tried against `<dir of .faris>/evidence-store/`, then the running
  package's store; accepted only where the index listing equals the recorded
  listing; reads are checked per file against the index. Otherwise "not included"
  with the tree, where it looked, and the next step.
- Writers: `faris study-file create --evidence <store descriptor>
  [--pack-evidence]`; a package-launched desktop session saves evidence
  (packed or referenced); opened new-format files re-save the same way.
- Judgment calls: the descriptor carries no arrangement or allocation, so they
  are read from the tree names (`<port|control>-<allocation>-case|workspace`);
  the format string stays `faris-study/1` (the layer is optional and the reader
  already ignores unknown fields); an older reader refuses a packed file on its
  unexpected-entry rule, as it did before this change for any unknown entry.

## Out of scope here

Core verifying a store in place (ADR-0028 follow-up); compacting the history
snapshot format.
