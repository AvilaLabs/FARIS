# Study files (.faris)

A `.faris` file is one saved study. It holds both arrangements (with and without the port), the recorded transport, the allocation sweep, the operating assumptions, any finished uncertainty ensembles, and the view you left open: the step, preset, what-if values, year, field view, history tab, and selected arrangement and allocation.

Calculated histories are not stored. They recalculate on opening, in about a second.

## Open a study

Open a file in any of these ways:

- Choose **File** > **Open…** (Ctrl+O).
- Drop the file on the window.
- Pass it on the command line: `faris-app demo.faris`.

The window title shows the file name, with a dot after it when the view differs from what was saved.

## Save a study

Choose **File** > **Save** (Ctrl+S) or **Save as…** (Ctrl+Shift+S). Save is greyed out, with a reason on hover, when nothing recorded can be saved. A study started from your own transport run records cannot be saved.

## What is checked

Recorded files are stored byte for byte and checked by hash on every open. This keeps the Core receipts' "unchanged since checked" guarantee through a save. A damaged file is refused, and the message names the damaged part. A file is also refused if a size is wrong, if it uses an unsupported encoding, or if its version is not major version 1.

Run this to see which part of a file fails:

```bash
faris study-file verify demo.faris
```

## Referenced or packed evidence

By default the Core evidence (the File menu shows its size) is recorded by name, file list and hash and is not stored. The study opens fully without it. Its Evidence step says "Core receipts not included", and why, and what to do.

For the current recorded package the evidence is the package's evidence store. A study file records each saved case as a tree of that store, with the full list of its files. The app looks for the trees in two places, in this order:

1. An `evidence-store` folder next to the `.faris` file. Save the file into an unpacked package folder, or copy the package's `evidence-store` folder next to it.
2. The evidence store of the package the app was started from.

A tree is used only when that store lists exactly the files the study recorded; every file read from it is checked against its hash. If a tree is missing or does not match, the Evidence step names it, says where the app looked, and says what to do: put the file next to the package's `evidence-store` folder, open it with FARIS started from that package, or save it again with the evidence included.

Study files saved with older packages record `.tar.gz` archives instead. They still open: put the archives next to the file at the recorded relative paths (for example `port/archives/reference-case.tar.gz` at the package root) and reopen it. Archives found there are checked against their hashes and used. One whose hash differs from the record is reported and never used.

To keep the evidence inside the file, tick **Include Core evidence in saved files** in the File menu and save. The label shows the added size. A package-launched session can do this: the file then carries the evidence store for exactly the saved cases, checked on every open, and the receipts can be re-checked from that one file. A file opened with its evidence included keeps it on the next save unless you clear the box.

An older FARIS that does not know store evidence opens a referenced file with the evidence marked as not included, and refuses a file with evidence included. Save the study without the box ticked to give it a file it opens.

## Open files from your file manager

On Linux, run `scripts/install_desktop_integration.sh /absolute/path/to/faris-app` from a source checkout. It works at user level. Add `--uninstall` to remove it. File managers do not show the file's preview thumbnail.

## The file itself

A `.faris` file is a zip container with a manifest and one content-addressed blob for each recorded file. The repository's [study file document](https://github.com/AvilaLabs/FARIS/blob/main/docs/STUDY_FILE.md) describes the format, its size policy and its reading rules. Unknown fields are ignored, so later minor versions can add fields. An unknown major version is refused.

Next: [Export a brief, tables and charts](export.md).
