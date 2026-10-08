# Study files (`.faris`)

A `.faris` file is one study: the arrangements, the recorded transport, the
assumptions and what-if values, and the view the author left open. Opening it
restores that state. It is the project file, in the sense that a `.psd` is for
an image; the PDF, CSV and chart exports are derived from it and name its hash.

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
   supplies the archives can check them against the recorded hashes. A
   package descriptor that names trees in an evidence store
   (`faris-saved-study-store/v0.1`) cannot yet be recorded here:
   `faris study-file create --evidence` refuses it and says so. Evidence
   records for store trees come in a later change; use a descriptor from an
   older package that names archives, or create the file without `--evidence`.

## Container

A zip archive. Entries:

| Entry | Contents |
| --- | --- |
| `mimetype` | `application/vnd.avila-labs.faris-study`, first entry, stored uncompressed, so the type is detectable from the first bytes |
| `manifest.json` | format `faris-study/1`, roles, view state, blob table, layers |
| `blobs/<sha256>` | exact bytes of one file, named by its SHA-256; identical files are stored once |
| `preview.png` | optional thumbnail of the 3D view: PNG, at most 512 px on the long side and 512 KiB (the desktop aims for under 150 kB), stored uncompressed. Written by the desktop app on save; not listed in the manifest |

Blobs are zstd-compressed, except blobs that are already compressed (evidence
`.tar.gz` archives), which are stored. Each blob entry in the manifest records
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
`mimetype`, `manifest.json`, `preview.png` and `blobs/<64 hex>`; repeated entry
names (checked in the central directory, because the zip library silently keeps
the last of two); a blob entry missing from the table or a table entry missing
from the file; zip64 containers; more than 4 GiB of declared blob bytes or 1 GiB
for one blob; blob reads longer than the recorded size (bounded while
decompressing, so a lying header cannot expand past it); unsafe bundle names or
evidence paths in the manifest. Writing is atomic: a temporary file in the
destination directory, `fsync`, then rename.

## Opening in the app

Opening unpacks the recorded files to a workspace directory inside the runs
directory (not `/tmp`, which may be memory), then loads them through the same
code as the command-line flags, off the UI thread behind a progress card. The
workspace is removed when another study is opened or the app exits normally.
Core evidence archives, packed or found beside the file, are extracted
(regular files only, bounded, created exclusively) in the background and
reopened by the existing saved-study code, so the study is usable first and
the receipts verify afterwards. Saving writes the inputs from those files plus
the current view; a session started from `--run` records cannot be saved
because a study file holds recorded-transport bundles.
