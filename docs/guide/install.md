# Download and verify

## Requirements

- A Linux desktop session with working graphics drivers.
- `python3`. The launcher and the verifier are Python scripts.
- Free temporary space: about 0.9 GB to open the study, because `launch.sh` unpacks the Core evidence (823 MB) into a private temporary folder, and about 2 GB for `verify.sh`. Both check the exact amount before they start. The package's own `README.md` explains the sums.

You do not need OpenMC, nuclear data or network access to explore the recorded study. Only Linux is built and tested. Windows and macOS builds have not been tried.

## Download the package

Download the Linux package, `FARIS-0.1.0-linux-x86_64.tar.gz` (112 MB), and `SHA256SUMS` from the [0.1.0 release](https://github.com/AvilaLabs/FARIS/releases/tag/v0.1.0). Check the archive, then unpack it:

```bash
sha256sum -c SHA256SUMS
tar xzf FARIS-0.1.0-linux-x86_64.tar.gz
```

The package holds four recorded transport cases, their Core evidence and the allocation sweep.

## Verify it

From the package folder, run:

```bash
./verify.sh
```

This is optional. It checks the package from a relocated copy, rechecks every recorded byte and the Core receipts, and takes a while. It also confirms that a deliberately changed copy is rejected.

## Open the recorded study

```bash
./launch.sh
```

`launch.sh` checks the hashes of the bundled programs and files, then opens the app with all four cases loaded. It keeps the unpacked Core evidence in a private temporary folder until the app closes.

The bundled programs are hash-pinned but not signed. A matching hash shows the bytes are the ones recorded in the package. It does not show who made them.

## Build from source

You can build FARIS yourself with Rust. The repository README has the pinned toolchain, the build commands and the Linux packages the graphics stack needs. A source build opens an embedded geometry demo unless you give it a study. The recorded study comes with the package.

Next: [take the tour](quick-start.md).
