# Troubleshooting

## The app will not start

The desktop needs a graphical session and compatible graphics drivers. Leave display-backend and DPI overrides unset in normal use, so a high-DPI desktop reports its native scale. The recorded frame rates were measured on both X11 and Wayland.

`launch.sh` needs `python3` and enough free temporary space. If it stops before opening the app, read its message and check the amounts in the package's `README.md`.

## Messages when you open a study

**"recorded case, mesh or response definitions differ from this FARIS build: …"**
The study was recorded by an earlier build. The full message continues: "The run was probably recorded by an earlier version; rerun the transport case with this build to use it."

**"Cannot open NAME: …"**
The file could not be opened. The reason follows. Reading is fail-closed. A file is refused if a blob does not match its recorded SHA-256 ("blob … does not match its recorded SHA-256"), if its size is wrong, if it uses an unsupported encoding, or if its version is not major version 1 ("unsupported study-file version …"). Run `faris study-file verify FILE` to see which part failed.

**"Core receipts not included"**
The Core archives are referenced, not packed. See [Study files](study-files.md). A listed archive that is "not found" is missing from beside the file. One whose SHA-256 differs from the record is not used.

## Uncertainty and export

**"No uncertainty range: …"**
The ensemble is not evaluated. The line gives the reason and the next step. Typical reasons are a transport record without covariance, or more than 1 per cent of draws rejected. See [Uncertainty ensembles](uncertainty.md).

**Export is unavailable.**
Hover the button. It says when histories are still calculating, or when the study has no operating assumptions.

**The 3D view is missing from an export.**
The export notes "The window capture did not arrive; this graphics backend may not support screenshots." The rest of the export is written.

**A command-line export has no ensembles.**
It uses only the ensembles stored in the file. Run them in the desktop and save, or use `faris history ensemble`.

## Operate and Evidence

**Save is greyed out.**
Hover it for the reason. Nothing recorded can be saved when the study was started from `--run` records.

**A badge says "receipts cover the loaded assumptions".**
The saved Core receipts were executed with the loaded assumptions, and the current preset differs. Choose **Use the covered assumptions**.

**Compile study says the Core executable is unavailable.**
Choose the installed Avila Core file under **Compiler settings**, or start the app with `--core`.

## Still stuck

Report reproducible problems through [GitHub issues](https://github.com/AvilaLabs/FARIS/issues). Include your version, the command or step, and the message.
