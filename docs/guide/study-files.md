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

By default the Core evidence archives (about 55 MB for the demo) are recorded by name and hash and are not stored. The study opens fully without them. Its Evidence step says "Core receipts not included", and why, and what to do.

To use the archives, put them next to the file at the recorded relative paths and reopen it. For example, `port/archives/reference-case.tar.gz` goes at that path beside the file when it sits at the package root. Archives found there are checked against their hashes and used. One whose hash differs from the record is reported and never used.

Or tick **Include Core evidence in saved files** in the File menu and save again. The label shows the added size.

## Open files from your file manager

On Linux, run `scripts/install_desktop_integration.sh /absolute/path/to/faris-app` from a source checkout. It works at user level. Add `--uninstall` to remove it. File managers do not show the file's preview thumbnail.

## The file itself

A `.faris` file is a zip container with a manifest and one content-addressed blob for each recorded file. The repository's [study file document](https://github.com/AvilaLabs/FARIS/blob/main/docs/STUDY_FILE.md) describes the format, its size policy and its reading rules. Unknown fields are ignored, so later minor versions can add fields. An unknown major version is refused.

Next: [Export a brief, tables and charts](export.md).
