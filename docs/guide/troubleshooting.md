# Troubleshooting

## The app will not start

The desktop needs a graphical session and compatible graphics drivers. Leave display-backend and DPI overrides unset in normal use, so a high-DPI desktop reports its native scale. The recorded frame rates were measured on both X11 and Wayland.

**The app opens with nothing loaded.**
A check of the package failed, and the app says which and why. Download the files again and check them against `SHA256SUMS`, then unpack the app archive into a new folder. A package that was changed, or unpacked only in part, is refused.

**Windows SmartScreen warns, or macOS will not open the app.**
The programs are not signed, and on macOS not notarized, so both systems warn. This is expected, and a matching hash shows the bytes are the recorded ones. On Windows, choose More info, then Run anyway. On macOS, run `xattr -dr com.apple.quarantine FARIS-0.1.1` once in Terminal after unpacking, or open `bin/faris-app` and allow it under System Settings, Privacy & Security, Open Anyway. See [Download and verify](install.md).

**The saved Core evidence gives up unpacking.**
With the evidence pack, the app unpacks 823 MB into a private temporary folder, so it needs about 0.9 GB free. It waits up to 10 minutes and says why if it gives up. Free some temporary space and reopen FARIS. The rest of the study is not affected.

## Messages when you open a study

**"recorded case, mesh or response definitions differ from this FARIS build: …"**
The study was recorded by an earlier build. The full message continues: "The run was probably recorded by an earlier version; rerun the transport case with this build to use it."

**"Cannot open NAME: …"**
The file could not be opened. The reason follows. Reading is fail-closed. A file is refused if a blob does not match its recorded SHA-256 ("blob … does not match its recorded SHA-256"), if its size is wrong, if it uses an unsupported encoding, or if its version is not major version 1 ("unsupported study-file version …"). Run `faris study-file verify FILE` to see which part failed.

**"Core receipts not included"**
For the recorded package, the evidence pack is not unpacked in the package folder. The message says why and the next step: download `FARIS-0.1.1-evidence.tar.gz` from the same release, unpack it into the `FARIS-0.1.1` folder, and reopen FARIS. Everything else works without it. For a `.faris` file, the Core archives are referenced, not packed. See [Study files](study-files.md). A listed archive that is "not found" is missing from beside the file. One whose SHA-256 differs from the record is not used.

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
