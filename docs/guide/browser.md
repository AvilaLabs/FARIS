# The browser version

The browser version of FARIS runs at [faris.avilalabs.org](https://faris.avilalabs.org). It opens the recorded demo study in a browser tab, with nothing to download or install.

## What it is

It is an explorer of recorded results. The study that the desktop app opens from its release package opens on page load. Nothing in the browser version starts a new calculation.

## What you can do

- Look at the plant in 3D, and follow the guided tour.
- Read the recorded transport results for the four arrangements, and the recorded maintenance result.
- Move the 30-year history sliders and see the uncertainty ranges. The ranges were calculated beforehand with 200 samples per arrangement; the browser reads them and does not calculate new ones.
- Compare the four arrangements and read the allocation sweep.
- Search the commands with Ctrl+K (Cmd+K on a Mac), and read this handbook at [faris.avilalabs.org/docs](https://faris.avilalabs.org/docs/).
- Open your own `.faris` study with the file picker or by dropping it on the page.
- Export a brief, tables and charts. The browser saves them as one zip download.

## What needs the desktop app

- **Transport runs.** They start OpenMC, which runs as a program on your computer.
- **CAD design check.** It starts the CAD helper, which a browser cannot start.
- **Weight windows.** They are built by transport runs.
- **Saving a `.faris` study.** The browser version can open a study but not write one back. Use the export for a copy of the results.
- **Saved Avila Core evidence and recorded design reports.** The packaged Core evidence store and the design transport and design check windows are not in the browser version.
- **Uncertainty with 1000 samples**, and any study whose ranges were not calculated beforehand. These show as unavailable with the reason.
- **ACTINV maintenance runs.** They start ACTINV as a program.
- **Compile study.** It starts Avila Core as a program.

In the browser version these are hidden, or shown as unavailable with the reason and a link to the desktop app. [Download the desktop app](install.md) to use them; the files are on the [releases page](https://github.com/AvilaLabs/FARIS/releases).

## Your files

Your files stay in the browser tab. FARIS does not upload them. When you close the tab they are gone, so save what you want to keep.

## Signing in

Signing in with an Avila Labs account is optional. Every part of the browser version works without it. See [Avila Labs account](account.md).

## Which browsers

The 3D view needs WebGPU or WebGL2. Recent versions of Chrome, Edge, Firefox and Safari have at least one of them. If your browser has neither, the page says so and points you to the desktop app.

## First visit

The first time you open the page, a card titled "About this browser version" explains these points: the page shows recorded results, it cannot run new transport, check CAD designs or run ACTINV or Avila Core, your files stay in the tab, and signing in is optional. The guided tour follows. It is shown once. You can open it again from the command palette or from "About this browser version".
