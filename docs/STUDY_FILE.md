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
with zstd. Core receipts check those bytes by hash, so any rewrite that saves
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

## Container

A zip archive. Entries:

| Entry | Contents |
| --- | --- |
| `mimetype` | `application/vnd.avila-labs.faris-study`, first entry, stored uncompressed, so the type is detectable from the first bytes |
| `manifest.json` | format `faris-study/1`, roles, view state, blob table, layers |
| `blobs/<sha256>` | exact bytes of one file, named by its SHA-256; identical files are stored once |
| `preview.png` | optional thumbnail |

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
