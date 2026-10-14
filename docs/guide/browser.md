# The browser version

The browser version of FARIS runs at [faris.avilalabs.org](https://faris.avilalabs.org). It opens the recorded demo study in a browser tab, with nothing to download or install.

## What it is

It is an explorer of recorded results. The study that the desktop app opens from its release package opens on page load. Nothing in the browser version starts a new calculation.

## What you can do

- Look at the plant in 3D, and follow the guided tour.
- Read the recorded transport results and the saved Avila Core evidence.
- Move the 30-year history sliders and see the uncertainty ensembles.
- Compare the four arrangements and read the allocation sweep.
- Search the commands with Ctrl+K (Cmd+K on a Mac), and read this handbook at [faris.avilalabs.org/docs](https://faris.avilalabs.org/docs/).
- Open your own `.faris` study, or a recorded design transport result or design check report, with the file picker or by dropping it on the page.
- Export a brief, tables and charts. The browser saves them as downloads.

## What needs the desktop app

- **Transport runs.** They start OpenMC, which runs as a program on your computer.
- **CAD design check.** It starts the CAD helper, which a browser cannot start.
- **Weight windows.** They are built by transport runs.
- **ACTINV maintenance runs.** They start ACTINV as a program.
- **Compile study.** It starts Avila Core as a program.

In the browser version these are hidden, or shown as unavailable with the reason. [Download the desktop app](install.md) to use them; the files are on the [releases page](https://github.com/AvilaLabs/FARIS/releases).

## Your files

Your files stay in the browser tab. FARIS does not upload them. When you close the tab they are gone, so save what you want to keep.

## Signing in

Signing in with an Avila Labs account is optional. Every part of the browser version works without it. See [Avila Labs account](account.md).

## Which browsers

The 3D view needs WebGPU or WebGL2. Recent versions of Chrome, Edge, Firefox and Safari have at least one of them. If your browser has neither, the page says so and points you to the desktop app.

## First visit

The first time you open the page, a card explains these points and the tour follows. It is shown once. You can open it again from the command palette or from "About this browser version".
