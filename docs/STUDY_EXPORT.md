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
| `data/histories.csv` | Long format, one row per snapshot per arrangement: calendar year, operating state, magnet and blanket fluence, usable tritium, net electricity, cumulative magnet swaps. |
| `data/comparison.csv` | One row per arrangement with standard errors, transport seed and histories, and a kind column per value. |
| `data/differences.csv` | The four contrasts with 2σ screening flags. |
| `data/sweep.csv` | One row per sweep allocation. |
| `data/assumptions.csv` | Every operating assumption: value, unit, kind, full provenance. |
| `data/caveats.csv` | The caveat list as in the PDF. |
| `charts/*.svg`, `charts/*.png` | The chart drawings. Each chart is generated once as an SVG string; that string is saved, rasterised to PNG with resvg, and drawn into the PDF as vectors. |
| `charts/3d-view.png` | The cropped 3D viewport, when the window capture worked. |
| `export-manifest.json` | Every file with SHA-256 and byte count, FARIS version, UTC timestamp, the study-file stamp or null, and whether a view image was included (with the reason when not). |

Values are written at full precision; units are in the CSV headers. Status
kinds (calculated, checked, authored, literature, conditional, partial, not
evaluated, failed) use the colours of the app badges.

## Fonts

PDF text is real text in the fonts the app uses: Ubuntu Light (proportional,
Ubuntu Font Licence 1.0, embedding in documents is permitted) and Hack
(monospace, MIT with Bitstream Vera terms), subset and embedded. Headings gain
weight from a thin stroke because Ubuntu Light is the only proportional face.
Licence texts are in `licenses/ubuntu-font-licence-1.0.txt` and
`licenses/hack-font-licence.txt`, and in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)
through `epaint_default_fonts`.

## API

```rust
faris_report::export_study(&ReportInput { study_file: Option<StudyFileStamp>,
    view_image: Option<Vec<u8>> /* PNG */, .. }, parent_dir)
```

The desktop calls it from one place (`FarisApp::export_input`); the study-file
stamp enters through `FarisApp::export_study_file_stamp`.
