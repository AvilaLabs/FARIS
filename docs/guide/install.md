# Download and verify

## Requirements

- A desktop session with working graphics drivers: Linux, Windows or macOS.
- Free temporary space of about 0.9 GB to open the study with the evidence pack, because the app unpacks the Core evidence (823 MB) into a private temporary folder while it runs. Without the evidence pack it needs none.
- For `verify.sh` only: Linux, `python3`, the evidence pack and about 2 GB of temporary space. It checks the exact amount before it starts.

Python is not needed to open the study. You do not need OpenMC, nuclear data or network access to explore the recorded study.

Linux is built and tested on the reference laptop. Windows and macOS builds are new in 0.1.1. They are built by CI from the same commit. Report problems on [GitHub issues](https://github.com/AvilaLabs/FARIS/issues).

## What to download

FARIS 0.1.1 is the same demo study and results as 0.1.0. The download is smaller and split in two. Get the files from the [0.1.1 release](https://github.com/AvilaLabs/FARIS/releases/tag/v0.1.1):

| File | What it is |
| --- | --- |
| `FARIS-0.1.1-linux-x86_64.tar.gz` | The app for Linux (about 35 MB). |
| `FARIS-0.1.1-windows-x86_64.zip` | The app for Windows. |
| `FARIS-0.1.1-macos-aarch64.tar.gz` | The app for a Mac with Apple silicon. |
| `FARIS-0.1.1-macos-x86_64.tar.gz` | The app for an Intel Mac. |
| `FARIS-0.1.1-evidence.tar.gz` | Optional. The Core receipts and the files `verify.sh` checks (about 78 MB). The same for every platform. |
| `SHA256SUMS` | The SHA-256 of every file above. |

Download the app for your platform, and `SHA256SUMS`. 0.1.0 was one 112 MB archive. The app download alone opens the whole study: the four recorded arrangements and the allocation sweep. The evidence pack adds the Avila Core receipts to the [Evidence](evidence.md) step.

Each archive unpacks to a folder `FARIS-0.1.1/`. Unpack the evidence pack into the same place, so that its files merge into `FARIS-0.1.1/`.

## Open the recorded study

Double-click `bin/faris-app` (`bin\faris-app.exe` on Windows), or run it from a terminal. The app finds the study beside its `bin` folder, checks the SHA-256 of every file it needs, and opens the four recorded arrangements and the sweep. To open a package somewhere else, run `faris-app --package DIR`.

If a check fails, the app opens with nothing loaded and says why and what to do: download the files again and check them against `SHA256SUMS`.

The programs are not signed. A matching hash shows the bytes are the ones recorded in the package. It does not show who made them.

## Linux

Check the archives, unpack them and open the app:

```bash
sha256sum -c SHA256SUMS --ignore-missing
tar xzf FARIS-0.1.1-linux-x86_64.tar.gz
tar xzf FARIS-0.1.1-evidence.tar.gz    # optional
FARIS-0.1.1/bin/faris-app
```

To recheck every recorded byte and the Core receipts, run `./verify.sh` from the package folder. It needs `python3` and the evidence pack. It checks the package from a relocated copy, takes a while, and confirms that a deliberately changed copy is rejected.

## Windows

Check `FARIS-0.1.1-windows-x86_64.zip` against `SHA256SUMS`: in PowerShell, `Get-FileHash FARIS-0.1.1-windows-x86_64.zip` prints the SHA-256, which must equal that file's line in `SHA256SUMS`. Then unpack it with Extract All. If you want the Core receipts, unpack the evidence pack into the same place: in the folder that holds `FARIS-0.1.1`, run `tar -xzf FARIS-0.1.1-evidence.tar.gz` in a terminal (Windows 10 and 11 include `tar`). Then double-click `bin\faris-app.exe`.

Windows SmartScreen may warn, because the programs are not signed. Choose More info, then Run anyway. The app opens without a console window. `verify.sh` does not run on Windows. The app checks the package itself at launch.

## macOS

Use `FARIS-0.1.1-macos-aarch64.tar.gz` on a Mac with Apple silicon and `FARIS-0.1.1-macos-x86_64.tar.gz` on an Intel Mac. Check it with `shasum -a 256 -c SHA256SUMS --ignore-missing` and unpack it, with the evidence pack if you want it, into the same place. The programs are neither signed nor notarized, so macOS blocks them at first. After unpacking, run this once in Terminal:

```bash
xattr -dr com.apple.quarantine FARIS-0.1.1
```

Then double-click `bin/faris-app`. Or open it first and allow it under System Settings, Privacy & Security, Open Anyway. `verify.sh` does not run on macOS. The app checks the package itself at launch.

## Without the evidence pack

Everything works except the saved receipts. The Evidence step says "Core receipts not included", why, and the next step: download the evidence pack, unpack it into the folder, and reopen FARIS. See [Troubleshooting](troubleshooting.md).

## Where FARIS keeps your files

Nothing is written inside the package. Runs that FARIS generates go to a folder for your user:

| System | Runs folder |
| --- | --- |
| Linux | `$XDG_STATE_HOME/faris/recorded-demo-runs/<folder>`, or `~/.local/state/faris/recorded-demo-runs/<folder>` if unset |
| macOS | `~/Library/Application Support/FARIS/recorded-demo-runs/<folder>` |
| Windows | `%LOCALAPPDATA%\FARIS\recorded-demo-runs\<folder>` |

`--runs-directory DIR` chooses another folder. It must be outside the package.

## Build from source

You can build FARIS yourself with Rust. The repository README has the pinned toolchain, the build commands and the Linux packages the graphics stack needs. A source build opens an embedded geometry demo unless you give it a study. The recorded study comes with the package.

Next: [take the tour](quick-start.md).
