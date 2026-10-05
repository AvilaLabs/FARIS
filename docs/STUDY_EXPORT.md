# Study export

The export is the human-readable deliverable of a study: a folder a colleague
can read without FARIS. The `.faris` study file stays the source of truth; the
export names its SHA-256 when the study has been saved ([STUDY_FILE.md](STUDY_FILE.md)).

`faris-report` builds it from engine results. Every number comes from
`faris-engine` (`brief`, `comparison`, `sweep`, `history`); the report and the
desktop comparison view read the same functions, so they cannot disagree. The
report crate has no UI dependency.

## Folder

`<study name>-export/`, created next to the chosen parent folder. An existing
folder is never written into: files go to a temporary sibling that is renamed
only when complete.

| Path | Contents |
| --- | --- |
| `summary.pdf` | Two US Letter pages. Page 1: question, the four-arrangement table with status labels, the generated takeaway, the 2σ flags, the magnet-fluence timeline and, when captured, the 3D view. Page 2: allocation sweep charts and findings, the assumptions table with kind and provenance, caveats and unknowns (each says why and what would settle it), footer with version, date and the study-file hash or "Unsaved study — no study file hash". |
| `data/histories.csv` | A `# ` comment line with the research-screening statement (every CSV starts with it), then long format, one row per snapshot per arrangement: calendar year, operating state, magnet and blanket fluence, usable tritium, net electricity, cumulative magnet swaps. |
| `data/comparison.csv` | One row per arrangement with standard errors, transport seed and histories, and a kind column per value. |
| `data/differences.csv` | The four contrasts with 2σ screening flags. |
| `data/sweep.csv` | One row per sweep allocation. |
| `data/assumptions.csv` | Every operating assumption: value, unit, kind, full provenance. |
| `data/caveats.csv` | The caveat list as in the PDF. |
| `charts/*.svg`, `charts/*.png` | The chart drawings. Each chart is generated once as an SVG string; that string is drawn into the PDF as vectors, and with a footer line carrying the research-screening statement it is saved and rasterised to PNG with resvg. |
| `charts/3d-view.png` | The cropped 3D viewport, when the window capture worked. |
| `export-manifest.json` | Every file with SHA-256 and byte count, FARIS version, the research-screening statement, UTC timestamp, the study-file stamp or null, and whether a view image was included (with the reason when not). |

## Research-screening statement

Every output carries "Research screening only. These results are not a
licensing, safety or design basis." (requirement LEG-040). The text is the
constant `faris_model::RESEARCH_SCREENING_STATEMENT`; nothing else spells it
out. It is in both PDF pages' footers, as the first line of every CSV (a `# `
comment: skip it with `pandas.read_csv(..., comment="#")` or by dropping lines
that start with `#`), in a footer strip of every saved SVG and PNG chart, in
the manifest field `research_screening`, and in the desktop's bottom bar on
every step. The history files that `faris history run` writes are not changed:
their `notice` is part of the bytes Core receipts bind.

Values are written at full precision; units are in the CSV headers. Status
kinds (calculated, checked, authored, literature, conditional, partial, not
evaluated, failed) use the colours of the app badges.

## Fonts

PDF text is real text in the fonts the app uses: Ubuntu Light (proportional,
Ubuntu Font Licence 1.0, embedding in documents is permitted) and Hack
(monospace, MIT with Bitstream Vera terms), subset and embedded. Headings gain
weight from a thin stroke because Ubuntu Light is the only proportional face.
Licence texts sit next to the font files (`crates/faris-report/fonts/UFL.txt`
and `Hack-LICENSE.md`), are also in `licenses/`, and are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)
through `epaint_default_fonts`.

## Command line

```
faris study-file export <FILE.faris> --output <PARENT_DIR>
```

Writes the same `<study name>-export` folder inside `<PARENT_DIR>` (which must
exist) without the desktop. The file is read with the fail-closed study reader; the recorded
transport bundles are validated as the desktop validates them; the operating
histories and the allocation sweep summaries are recalculated from the
recorded assumptions (preset and edited what-if values) and transport rates,
exactly as the app does on opening the file. The study-file stamp names the
file's SHA-256. The 3D view is not captured on the command line: the PDF has
no 3D image and the manifest says so ("export from the desktop app"). The
desktop calls the same assembly function (`faris_report::assemble_report_input`),
so their data selection cannot differ. An existing export folder is never
written into, and any failure exits non-zero with nothing written. Stored
history ensembles are not part of the report input, so none are used. (The
top-level `faris export` is the unrelated scenario-manifest export.)

## API

```rust
faris_report::export_study(&ReportInput { study_file: Option<StudyFileStamp>,
    view_image: Option<Vec<u8>> /* PNG */, .. }, parent_dir)
```

The desktop calls `faris_report::assemble_report_input` from one place
(`FarisApp::export_input`), and the command line calls it from
`crates/faris-cli/src/study_export.rs`; the study-file stamp enters through
`FarisApp::export_study_file_stamp` and `StudyFileStamp::from_path`.
