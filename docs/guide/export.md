# Export a brief, tables and charts

An export is a folder a colleague can read without FARIS. The `.faris` file stays the source of truth, and the export names its SHA-256 when the study has been saved.

## Export from the desktop

Choose **Export…** in the top bar. The app asks for a folder and writes `<study name>-export/` into it. It never writes into an existing export folder.

Export is unavailable while the histories, uncertainty ranges, sweep or saved evidence are still calculating or loading. Hover the button for the reason. It is also unavailable when the study has no operating assumptions.

## What the folder holds

| File | Contents |
| --- | --- |
| `summary.pdf` | Two US Letter pages: the comparison, the flags, the timeline, the sweep, the assumptions, and the caveats and unknowns. A third page, on uncertainty, appears when an ensemble exists. |
| `data/*.csv` | Histories, comparison, differences, sweep, assumptions and caveats. With ensembles, also the ensemble samples, summary and paired comparison. |
| `charts/` | The charts as SVG and PNG, and the 3D view when it was captured. |
| `export-manifest.json` | Every file with its SHA-256 and size, the FARIS version, the research-screening statement, the time, and the study-file hash. |

The PDF footer carries the version, the date and the study-file hash. For an unsaved study it reads "Unsaved study — no study file hash".

Every PDF page, CSV, chart and the manifest carry the research-screening statement. Each CSV starts with a `# ` comment line holding it. To skip that line when you load a table, use:

```python
pandas.read_csv("data/histories.csv", comment="#")
```

Values are written at full precision, and units are in the CSV headers. Each kind column uses the labels in [Reading the numbers](results.md).

## Uncertainty in the export

With an ensemble for at least one arrangement, the export gains the third PDF page, a shaded P5 to P95 band on the magnet-fluence timeline, two charts of tritium and net electricity bands, and the three ensemble CSVs. An arrangement whose ensemble is not evaluated keeps its nominal values, labelled "no uncertainty range" with the reason. The next step is written as text beside it. Every range is labelled as transport Monte Carlo sampling uncertainty only.

## Export from the command line

```bash
faris study-file export demo.faris --output /path/to/parent
```

The parent folder must exist. This writes the same `<study name>-export` folder without the desktop. It recalculates the histories and the sweep from the recorded assumptions and rates, as the app does on opening the file. It has two differences from the desktop:

- It has no 3D view. The PDF has no 3D image, and the manifest says so.
- It reuses only the ensembles stored in the file and never calculates new ones. An arrangement with none is exported as "not calculated". Run the ensembles in the desktop, or with `faris history ensemble`, and save first.

Any failure exits non-zero with nothing written.

## If the 3D view is missing

The export notes "The window capture did not arrive; this graphics backend may not support screenshots." The rest of the export is written.

Next: [Command line](cli.md).
