<!-- kind: reference -->
# Study files (`.faris`)

A `.faris` file is one study: the arrangements, the recorded transport, the
assumptions and what-if values, and the view the author left open. Opening it
restores that state. It is the project file, in the sense that a `.psd` is for
an image; the PDF, CSV and chart exports are derived from it and name its hash.

The format is open. This page, the JSON Schemas in [`docs/schemas/`](schemas/)
and the archived fixtures are enough to write a reader without FARIS source;
[`scripts/read_study_file.py`](../scripts/read_study_file.py) is such a reader
(Python standard library only, under 300 lines, written from this page). See
"Writing a reader" below.

## Size policy (decided 2026-10-01)

Measured on the demo study (four arrangements plus the seven-point allocation
sweep, eleven recorded transport bundles):

| Encoding of the same 154 MB of unique recorded files | Size |
| --- | --- |
| JSON text as recorded | 154.1 MB |
| zip, deflate level 9 | 7.4 MB |
| zip, zstd level 19 | 5.0 MB |

One bundle is 15.8 MB of JSON. Most of that is repetition: one 0.95 MB request
block appears in three or four member files, and each of about 80,000 numbers
takes about 12 characters. As 8-byte binary values the numbers alone would take
about 0.6 MB.

**Demo (v1 as built):** recorded files are stored byte for byte, compressed
with zstd at level 15. Level 19, the 5.0 MB figure above, takes about two
minutes to write the demo study on one core; level 15 writes it in about six
seconds into 6.1 MB (level 9: 1.5 s, 6.2 MB; level 12: 3.2 s, 6.1 MB). The
high levels are a time cliff, not a ratio gain, so the writer stops at 15 and
the level can be raised with `--zstd-level` where time does not matter. Core receipts check those bytes by hash, so any rewrite that saves
space would break the "unchanged since checked" guarantee; lossless compression
does not.

**Full product:** compression will not keep up. The 20× ratio comes from text
overhead and repetition, which shrink as the data grows. Finer meshes and
energy-resolved fields (for example 100,000 cells × 175 groups × value and error,
about 35 million numbers) would be roughly 400 MB of text or 280 MB of binary,
and noisy Monte Carlo values in binary compress poorly. Two structural rules
keep the file small without weakening verification. v1 reserves both:

1. **Binary arrays.** Large numeric fields are stored as typed little-endian
   arrays, not JSON text. A derived array names the hash of the evidence it
   was derived from and the FARIS version that derived it, so the chain back to
   the checked bytes is preserved.
2. **Separable evidence.** The study layer (inputs, displayed fields,
   assumptions, view) is always inside the file. The evidence layer (Core case
   and workspace archives, complete raw solver output) is either packed or
   referenced by hash only. A referenced study opens and displays fully. Its
   Evidence step says the receipts are not included, and anyone who later
   supplies the archives can check them against the recorded hashes.
   Evidence recorded as trees of an evidence store (minor version 1.1, below)
   follows the same rule.

## Container

A zip archive in the ordinary single-disk format (PKWARE APPNOTE 6.3): no zip64,
no encryption, no multi-part files, no archive comment that matters (a reader
finds the end-of-central-directory record in the last 65 557 bytes). Entry
names are exactly the ones below, in UTF-8 with forward slashes, each once. Entry
compression is *stored* (method 0) or *zstd* (method 93, RFC 8878 frames, which
the zip format registers); a writer uses no other method, and a reader that meets
one it cannot decode says which entry and which method. Entries:

| Entry | Contents |
| --- | --- |
| `mimetype` | `application/vnd.avila-labs.faris-study`, first entry, stored uncompressed, so the type is detectable from the first bytes |
| `manifest.json` | format `faris-study/1`, roles, view state, blob table, layers |
| `blobs/<sha256>` | exact bytes of one file, named by its SHA-256; identical files are stored once |
| `evidence-store/store.json`, `evidence-store/blobs/<h0h1>/<h>.xz` | optional, only with packed store evidence: a content-addressed evidence store holding exactly the recorded trees (see "Evidence store layer"), stored uncompressed. Not listed in the blob table |
| `preview.png` | optional thumbnail of the 3D view: PNG, at most 512 px on the long side and 512 KiB (the desktop aims for under 150 kB), stored uncompressed. Written by the desktop app on save; not listed in the manifest |

Entry order: `mimetype` first (so the type string sits at byte offset 30 of the
file, after the 30-byte local header and the 8-byte name), then `manifest.json`,
the blobs, then the optional entries. Order after `mimetype` carries no meaning.
Blobs are zstd-compressed, except blobs that are already compressed (evidence
`.tar.gz` archives), which are stored. `manifest.json` is zstd-compressed too. Each blob entry in the manifest records
`sha256`, `bytes`, `media_type` and `encoding`. v1 writes only `verbatim`;
`array-f64-le` and `array-f32-le` are reserved for rule 1 above.

### Manifest roles

- `arrangements`: for each role (`port`, `control`), the scenario, the physics
  inputs and the recorded transport bundles, each bundle as its schema version,
  notice and a map from member file name to blob hash. The original bundle is
  reconstructed exactly from these.
- `sweep`: the allocation-sweep bundles in the same form.
- `assumptions`: the operating-assumption file as a blob.
- `view`: workflow step, assumption preset, what-if values, calendar year, field
  view, history tab and selected arrangement and allocation. Calculated
  histories are not stored; they are recalculated on open in about a second.
- `ensembles`: the finished Monte Carlo ensembles of the operating history, one
  derived blob each, with the full key of the inputs they were calculated from
  (see "History ensembles" below). Optional.
- `layers.evidence`: `packed` or `referenced`, with each archive's role, file
  name, SHA-256 and size either way.

## Reading rules

- An unknown major format version is refused, with the version named.
- Every blob is hashed on read; a mismatch refuses the file and names the blob.
- An unknown blob encoding is refused. The reader never skips data it cannot
  interpret.
- Unknown manifest fields are ignored, so minor versions can add fields.
- Opening a file with referenced evidence never claims Core verification. The
  Evidence step shows the recorded hashes and explains how to supply the archives.

## Manifest as built (v1)

`manifest.json` is pretty-printed JSON with these top-level fields. Digests are
lowercase hexadecimal SHA-256 without a prefix.

- `format`: `faris-study/1`; `created_by`: the FARIS version.
- `arrangements.port`, `arrangements.control`: `{scenario, physics[], bundles[]}`.
  `scenario` and `physics` are blob digests. Each bundle is `{name, schema_version,
  notice, files{member name: digest}, trailing_newline}`. The reader rebuilds the
  exact `RecordedTransportBundle` and writes it back as indented JSON, with the
  trailing newline the recorded file had.
- `sweep`: bundles in the same form.
- `assumptions`: digest of the operating-assumption file.
- `view`: `{step, preset, what_if, year, field_view, history_tab, arrangement,
  allocation, sweep_blanket_m}`. `what_if` holds the full edited assumption
  values; names are the app's stable kebab-case names; unknown names fall back
  to defaults on opening.
- `ensembles`: `[{blob, scenario_sha256, variant, key}]`, omitted when there are
  none. See "History ensembles" below.
- `layers.evidence`: `{mode: packed|referenced, archives[]}`, each archive
  `{arrangement, allocation, kind: case|workspace, file_name, sha256, bytes}`.
  `file_name` is a relative path of one to four safe components, for example
  `port/archives/reference-case.tar.gz`: the two arrangements use the same base
  names, and the path is where the archive is looked for relative to the study
  file (a study saved beside the package root finds `port/archives/...`
  unchanged). Referenced archives found there are used only when size and hash
  both match; one that exists with a different hash is reported as such, never used.
- `layers.evidence_store`: Core evidence as store trees, described under
  "Evidence store layer" below. A file has this or `layers.evidence`, never both.
- `blobs`: `[{sha256, bytes, media_type, encoding}]`, one per distinct file.

Recorded-transport members are whatever the bundle holds: six required files
and three optional ones, `solver/worker-result.json`,
`solver/transport-spectra.json` and `solver/transport-batch-values.json`. The
last is the per-batch response values behind the recorded response covariance.
Like every member it is stored as a content-addressed blob and hashed on read.
It is optional on both sides: bundles recorded before the adapter wrote it
have no such member and load unchanged, and a reader accepts a file either
way. This needs no new format version, because unknown manifest fields and
member names inside the bundle's own allowed list are already covered by the
minor-version rule above (`files` is a free map of member name to digest).

Packed evidence archives are blobs with media type `application/gzip`, stored
rather than recompressed.

## Evidence store layer (minor version 1.1)

Decided 2026-10-08. The recorded package keeps its Core evidence in one
content-addressed store (`evidence-store/`, Avila Core ADR-0028,
`avila.core/evidence-store/v0.1`) instead of `.tar.gz` archives. A study file
records saved cases as trees of that store in a new optional layer. The format
string stays `faris-study/1`: the manifest has no `deny_unknown_fields`, so a
reader without this layer ignores it and reports the evidence as not included,
an existing, explained state.

```json
"layers": {
  "evidence_store": {
    "mode": "packed" | "referenced",
    "trees": [
      { "arrangement": "port" | "control", "allocation": "reference",
        "kind": "case" | "workspace", "tree": "port-reference-case",
        "files": [ { "path": "...", "sha256": "...", "bytes": 123 } ] }
    ]
  }
}
```

- **Identity.** A tree is identified by its full file listing (path, SHA-256 and
  size of every file, sorted by path; a case tree is a few dozen files). The
  tree name says where to look; the listing says what is accepted.
- **Structure.** Every saved study has exactly one case and one workspace tree.
  Arrangements are `port` or `control`; allocation and tree names are safe file
  names; role and tree name are unique; listings are sorted and bounded. A file
  has `layers.evidence` (archives) or `layers.evidence_store`, never both: a
  file that records both is refused.
- **Packed.** The zip carries a valid ADR-0028 store holding exactly the
  recorded trees: `evidence-store/store.json` and one
  `evidence-store/blobs/<first two hex digits>/<sha256>.xz` per distinct file
  content, written as stored (uncompressed) zip entries. The writer copies the
  `.xz` bytes of the source store after decompressing and checking each blob; it
  never recompresses. The reader requires the entries to be exactly `store.json`
  and the distinct digests of the listed files, each entry no larger than its
  listing allows (the file's size plus 0.1 percent plus 1 KiB, and 64 MiB for
  `store.json`), within the file's 4 GiB total. A file with store entries but no
  packed layer is refused.
- **Opening packed.** The embedded store is extracted (exclusively created,
  bounded reads) to `evidence-store/` in the study's run workspace, opened and
  verified with the FARIS store verifier (every blob decompresses to its recorded
  length and SHA-256, and nothing else is in `blobs/`), and its trees must equal
  the manifest's listings exactly. Any failure refuses the file. The app then
  inspects the saved cases in place from that store; no tree is expanded.
  `faris study-file verify` runs the same check on a scratch copy.
- **Referenced.** Only the listings are recorded. A tree is looked for, in this
  order, in (a) an `evidence-store/` folder beside the `.faris` file (the file
  was saved into an unpacked package or next to a copy of the store), then (b)
  the store of the package the desktop app was started from. It is accepted only
  where the store's index lists exactly the recorded files (path, SHA-256, size);
  a saved study needs both its trees from one store. Accepted trees are read in
  place, and every file read is checked against the store's index, so a damaged
  blob is reported by the saved-study check, not used. Otherwise the saved study
  is "not included": the Evidence step names the tree that was not found or did
  not match and where it looked, and the next step (put the file next to the
  package's `evidence-store` folder, open it from the package, or save with
  evidence packed). Nothing claims Core verification for a study that is not
  included.
- **Writing.** `faris study-file create --evidence DESCRIPTOR [--pack-evidence]`
  accepts a `faris-saved-study-store/v0.1` descriptor (store folder relative to
  the descriptor, a case tree and a workspace tree). Arrangement and allocation
  come from the tree names, `<port|control>-<allocation>-case` and `-workspace`;
  the listings are read from the store's index. The desktop app, started from a
  package, offers "Include Core evidence in saved files": packed embeds the
  package's trees for the saved cases, otherwise it records the listings. A file
  opened with new-format evidence saves it again the same way (packed stays
  packed, referenced stays referenced unless the box says otherwise). A file that
  carries old `layers.evidence` archives still saves them as archives and is not
  converted.
- **Older readers.** A reader without this layer ignores a referenced layer and
  shows the evidence as not included. It refuses a packed file, because it does
  not know the `evidence-store/` entry names (see "Reader limits as built").
  Re-saving without evidence, or from a package-launched session, gives such a
  reader a file it opens.

## History ensembles

The uncertainty ranges beside the operating history come from an ensemble: the
history re-run on rates drawn from the recorded transport means and covariance
(`docs/OPERATING_HISTORY.md`). 200 samples of four arrangements take minutes,
so each finished ensemble is stored as a derived blob and reused on open.

- **Blob.** The `HistoryEnsemble` as JSON (sampled rates, every per-sample
  outcome, the summaries and the 361-point series bands, or the not-evaluated
  reason and next step). Media type
  `application/vnd.avila-labs.faris-history-ensemble+json`, encoding
  `verbatim`, hashed like every other blob. A not-evaluated ensemble is stored
  too, with its reasons.
- **Key.** Each manifest entry carries the full identity of the calculation:
  `method` (`faris-history-ensemble/v1`), `history_model` (the ledger's
  processing-model identity), `samples`, `seed` (as a string, so no reader
  rounds a 64-bit value through a float), `rates_sha256` (SHA-256 of the
  driving rates as JSON, covariance included) and `assumptions_sha256` (SHA-256
  of the operating assumptions as JSON). `scenario_sha256` and `variant` say
  which arrangement it was for; they do not decide reuse.
- **Reuse.** On open, the application computes the key of each arrangement from
  the loaded transport and the restored assumptions and uses a stored ensemble
  only when the key is equal in every part. Any difference (a changed what-if
  value, another sample count, a transport rerun, another FARIS method or
  ledger version) is a miss: the ensemble is recomputed in the background and
  the stored one is left unused. Nothing is ever shown for inputs it was not
  calculated from.
- **Fail closed.** A blob that fails its hash, does not parse as an ensemble
  (unknown fields included), or whose own method, seed or sample count
  disagrees with its key, refuses the file. Two entries with one key, a key
  with an unknown part, or an entry naming a blob the file does not hold are
  refused as well.
- **Old files.** A file without `ensembles` opens exactly as before and
  computes its ensembles in the background.
- **Scope.** The ranges are transport Monte Carlo sampling uncertainty only. The
  file records that, in the blob's `scope` text, next to every ensemble.

## Preview thumbnail

`preview.png` is a picture of the 3D viewport the author left open, for file
managers and quick look. Decisions (2026-10-05):

- **No manifest field, no hash.** The format already reserved the entry name and
  no manifest field, so none is added: old readers that accept the name keep
  working, and the thumbnail is not evidence (nothing a receipt binds). It is
  not part of "every blob is hashed".
- **Optional on both sides.** A file without it is complete. The desktop app
  writes it on Save when the 3D view is visible and the window capture arrives
  within 1.5 s; otherwise it saves without one. A failed capture, crop or
  encode never fails or delays the save beyond that bound.
- **Size.** Scaled down (never up), aspect kept, to 512 px on the long side,
  encoded as RGB PNG. If that exceeds 150 kB the writer retries at 384, 256, 192
  and 128 px. `write_study` stores a draft's thumbnail only when it is a PNG of
  at most 512 px a side and 512 KiB; anything else is left out silently.
- **Reading.** The reader never refuses a study for its thumbnail beyond the
  structural rule below. A thumbnail whose container CRC fails, that is not a
  PNG, or that is over 512 px or 512 KiB is ignored (`StudyReader::preview`
  returns nothing and `preview_status` names why). An entry declaring more than
  16 MiB still refuses the file, as before.
- **Inspect.** `faris study-file inspect` prints `preview`: `present`, and when
  present `usable`, `bytes`, `width` and `height`, or `ignored_because`.
  `study-file create --preview <png>` stores a thumbnail from the command line.
- **File managers.** Linux managers do not read the entry; showing it there
  needs a thumbnailer registered with the desktop, which is not done.

## Reader limits as built

Beyond the reading rules above, the reader refuses: a first entry that is not a
stored `mimetype` with the exact type string; entry names other than
`mimetype`, `manifest.json`, `preview.png`, `blobs/<64 hex>` and, with packed
store evidence, `evidence-store/store.json` and
`evidence-store/blobs/<2 hex>/<64 hex>.xz`; repeated entry
names (checked in the central directory, because the zip library silently keeps
the last of two); a blob entry missing from the table or a table entry missing
from the file; zip64 containers; more than 4 GiB of declared blob bytes or 1 GiB
for one blob; blob reads longer than the recorded size (bounded while
decompressing, so a lying header cannot expand past it); unsafe bundle names or
evidence paths in the manifest. Writing is atomic: a temporary file in the
destination directory, `fsync`, then rename.

## Writing a reader

This is the algorithm, in the order a reader should apply it. Everything here
is checked by the archived fixtures and by `scripts/read_study_file.py`. A
reader that follows it opens every file FARIS writes and refuses every file
FARIS refuses.

**1. Container.** Open the zip. Refuse a file that is not a zip, that uses zip64
or encryption, that repeats an entry name (read names from the central
directory: some zip libraries silently keep the last of two), or whose first
entry is not a *stored* entry named `mimetype` holding exactly
`application/vnd.avila-labs.faris-study`. Refuse any entry name other than
`mimetype`, `manifest.json`, `preview.png`, `blobs/<64 lowercase hex>` and the
embedded-store names in step 6.

**2. Manifest.** Read `manifest.json` (at most 64 MiB) as UTF-8 JSON. It must be
an object with a string `format`. The format string is `faris-study/<major>`
optionally followed by dot-separated minor numbers. Refuse a major other than `1`
and name the version found ("faris-study/2"); accept any minor of 1. Ignore
manifest properties you do not know, at every level (step 8 says what to do
with them). Digests everywhere are 64 lowercase hexadecimal characters, the
SHA-256 of the bytes they name, with no prefix. Numbers that exceed 53 bits are
written as text (`seed`), so no reader rounds them.

**3. Blob table.** `blobs` is a list of `{sha256, bytes, media_type, encoding}`.
Refuse an invalid digest, a digest listed twice, a `bytes` outside 0 to 1 GiB, a
total over 4 GiB, and any `encoding` other than `verbatim` (`array-f64-le` and
`array-f32-le` are reserved names; a reader never skips data it cannot interpret).
The `blobs/<sha256>` entries and the table must name exactly the same digests.

**4. Verify every blob.** Decompress each blob entry, reading at most `bytes` + 1
bytes (a lying header must not expand past its record). Refuse a blob whose
length is not `bytes` or whose SHA-256 is not its name, and name the blob. Blobs
hold exact file contents, so a reader that wants the original file writes the
bytes unchanged. Blobs shared by several records are stored once.

**5. References.** Every digest the manifest uses to name a blob (`assumptions`,
`arrangements.<role>.scenario` and `.physics[]`, each bundle's `files` values,
`ensembles[].blob`, and `layers.evidence.archives[].sha256` when `mode` is
`packed`) must be in the blob table. Bundle names are safe file names and unique
within their list; ensemble keys are unique; evidence-archive roles
`(arrangement, allocation, kind)` are unique, with `arrangement` one of `port`
or `control`, a safe `allocation`, a `file_name` that is a safe relative path,
and, when packed, a `bytes` equal to the blob's. A *safe file name* is 1 to 128
characters of letters, digits, `.`, `_`, `-`, and is not `.` or `..`; a *safe
relative path* is one to four such names joined by `/`. A file with both
`layers.evidence` and `layers.evidence_store` is refused.

**6. Embedded evidence store.** With `layers.evidence_store.mode` `packed`, the
file holds exactly the entries `evidence-store/store.json` and, for each distinct
`sha256` among the listed files, `evidence-store/blobs/<first two hex digits>/<sha256>.xz`
(an xz stream, entry stored). Refuse more, fewer or differently named entries,
and refuse any `evidence-store/` entry when the layer is absent or `referenced`.
`store.json` is `{schema_version, codec, trees[{name, files[{path, sha256, bytes}]}]}`;
each tree in the manifest must equal the tree of the same name there
(`tree` = `name`, same files). Each `.xz` blob must decompress to exactly
`bytes` bytes whose SHA-256 is the entry's name. These blobs are not in the
blob table.

**7. Optional parts.** `preview.png` is a thumbnail: never refuse a file for it,
and use it only if it is a PNG at most 512 pixels a side and 512 KiB. A
`referenced` layer records only identities; a reader reports the evidence as not
included and never claims Core verification.

**8. Unknown fields.** A reader may drop unknown properties when it only reads.
FARIS preserves unknown *top-level* manifest properties when it re-saves a
file it opened (see "Newer minor versions" below).

**What to report.** A refusal names the rule and the blob or entry. FARIS and the
reference reader print one sentence and exit non-zero; neither returns partial
data from a refused file.

## Versioning

The format string is `faris-study/<major>[.<minor>]`. Today it is `faris-study/1`.

- **Minor versions** only add optional properties, optional entries whose names
  are listed above, or new optional layers (`layers.evidence_store` was minor 1.1
  and kept the string `faris-study/1`). A reader written for 1.0 reads every
  1.x file, ignoring what it does not know, and may say that evidence recorded
  in a layer it does not know is not included.
- **A major version** is needed to change what an existing property means,
  remove a required one, add a blob encoding a reader must interpret, or change
  the container. FARIS refuses a major it does not know, naming it, and
  opens every file written by any released version of the previous two majors
  and all of the current one (PRV-053). It never rewrites a file it opened:
  saving writes a new file.
- **Archived fixtures.** `crates/faris-study/tests/fixtures/` holds files written
  by the writer and frozen, one summary beside each (`*.summary.json`, schema
  `faris-study-summary-1`). Each release adds the files its writer produces if
  the format or a layer changed; a fixture is never regenerated to make a test
  pass. See `FIXTURES.md` there for the set and how it was made.

### Newer minor versions

A file written by a newer minor version opens (it is format 1, so nothing in it
is refused), and the reader lists the top-level manifest properties it does not
know (`StudyReader::unknown_manifest_fields`) so the application can say so.
`write_study_preserving` writes them back into the manifest of a re-saved study,
unchanged. Unknown properties nested inside known ones (inside `view`, a bundle
or a layer) are ignored on read and are not carried over; a version that adds
nested properties must say so here and move them to the top level or bump the
major version.

## Schemas

Machine-readable, JSON Schema draft 2020-12, in [`docs/schemas/`](schemas/):

| Schema | Describes |
| --- | --- |
| [`faris-study-manifest-1.schema.json`](schemas/faris-study-manifest-1.schema.json) | `manifest.json` |
| [`faris-history-ensemble-1.schema.json`](schemas/faris-history-ensemble-1.schema.json) | an ensemble blob (media type `application/vnd.avila-labs.faris-history-ensemble+json`) |
| [`faris-evidence-store-index-1.schema.json`](schemas/faris-evidence-store-index-1.schema.json) | `evidence-store/store.json` in a packed file |
| [`faris-study-summary-1.schema.json`](schemas/faris-study-summary-1.schema.json) | the summary two readers compare (`read_study_file.py --summary`) |
| [`faris-diff-1.schema.json`](schemas/faris-diff-1.schema.json) | the output of `faris diff --json` (`faris-diff/v1`) |

The manifest schema allows unknown properties (minor versions add them); the
ensemble and store-index schemas forbid them, as FARIS does for those blobs.
Constraints a schema cannot state (the blobs exist, hashes match, sizes agree,
references resolve) are the rules above. Recorded transport bundles, scenario,
physics and assumption files are blobs of other FARIS formats, described in
[`FILE_FORMATS.md`](FILE_FORMATS.md); a reader that does not need them treats them
as bytes. A test validates every file the writer produces against these schemas.

## The reference reader

    python3 scripts/read_study_file.py study.faris             # verify and list
    python3 scripts/read_study_file.py study.faris --summary   # JSON for comparing readers
    python3 scripts/read_study_file.py study.faris --records   # bundles, ensembles, evidence
    python3 scripts/read_study_file.py study.faris --manifest  # the manifest as stored
    python3 scripts/read_study_file.py study.faris --blob SHA  # one verified blob to stdout

Exit status 0 means every hash verified; 1 means the file was refused (the
reason is printed); 2 means this Python cannot decompress the file. Zstd zip
entries need Python 3.14 or newer (`zipfile` gained zstd support there); on an
older Python the reader says so and names the entry, and its test skips.
`records` are the bundles (with `group` `port`, `control` or `sweep`),
ensembles, evidence archives and evidence-store trees, which together make
the summary's `record_count`.

## Opening in the app

Opening unpacks the recorded files to a workspace directory inside the runs
directory (not `/tmp`, which may be memory), then loads them through the same
code as the command-line flags, off the UI thread behind a progress card. The
workspace is removed when another study is opened or the app exits normally.
Core evidence archives, packed or found beside the file, are extracted
(regular files only, bounded, created exclusively) in the background and
reopened by the existing saved-study code, so the study is usable first and
the receipts verify afterwards. Evidence recorded as store trees is not
expanded at all: a packed store is extracted into the workspace and the saved
cases are inspected in place from it, and referenced trees are read in place
from the store beside the file or the package's store. Saving writes the inputs from those files plus
the current view; a session started from `--run` records cannot be saved
because a study file holds recorded-transport bundles.
