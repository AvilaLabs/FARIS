# Take the tour

## Start the app

Open `bin/faris-app` from the package folder (see [Download and verify](install.md)). The first time the app starts without a study file, it plays a 12-stop tour of the recorded study. It takes about a minute.

Use **Next** (or the Right arrow, or Enter) and **Back** (Left arrow) to move. Choose **Skip tour** (or press Esc) to leave. Finishing or skipping writes a marker file, so the tour does not play again. The marker is `tour-completed`. On Linux it is in a `faris` folder under `$XDG_CONFIG_HOME`, or under `~/.config` when that is not set. On macOS it is in `~/Library/Application Support/FARIS`, and on Windows in `%APPDATA%\FARIS`.

Choose **Tour** in the top bar to replay it. Start the app with `--tour always` or `--tour never` to override the default.

## The five steps

The workspace has five steps. The top bar has one button for each, and the keys 1 to 5 switch between them.

| Key | Step | What you do there |
| --- | --- | --- |
| 1 | [Design](design.md) | Look at the plant in 3D. Choose the port and the blanket/shield allocation. |
| 2 | [Simulate](simulate.md) | Read the transport results and colour the 3D model with flux, heating or fluence. |
| 3 | [Operate](operate.md) | Change the operating assumptions and watch the 30-year timeline. Run an [uncertainty ensemble](uncertainty.md). |
| 4 | [Compare](compare.md) | Put the four arrangements side by side. Slide through the allocation sweep. |
| 5 | [Evidence](evidence.md) | See what was run, with which inputs and receipts, and what is not evaluated. |

The left panel shows the controls of the current step. The right panel is the Outliner, where you show or hide components, and the Properties of the selected component. The centre is the 3D viewport. The bottom panel is the timeline, or the comparison on step 4.

## Move in the viewport

- Drag to orbit.
- Shift-drag to pan.
- Scroll to zoom.
- Click a component to select it.

**Frame scene** resets the camera. **Cutaway** is a display option only. It does not change the model or any reported volume.

## Change the interface size

Choose **Interface size** in the top bar. It offers 100, 125, 150, 175 and 200 per cent, and scales text, controls, panels and plots together. Ctrl/Cmd + and − adjust it; Ctrl/Cmd 0 resets it. The 100 per cent setting follows your desktop's display scaling. To start larger, pass `--interface-scale 1.25`. The allowed range is 0.75 to 2.

## Try three things

1. Press 3. Move **Magnet service limit** and watch the timeline.
2. Press 4. Read **What changes** first, then the flags.
3. Press 5. Read why the scientific verdict reads NOT_EVALUATED.

Next: [Design: the plant in 3D](design.md).
