<!-- kind: how-to -->
# Undo, recovery and backups

This page covers what FARIS keeps so that a mistake, a crash or a power cut does not cost you work: undo and redo, the automatic recovery copy, earlier versions of a saved study, and the crash records kept on your computer. Nothing on this page is sent anywhere.

## Undo and redo

Press Ctrl+Z to undo the last change, and Ctrl+Shift+Z or Ctrl+Y to redo it. On macOS, read Ctrl as Cmd. The same actions are in the **Edit** menu and in the command palette (Ctrl+K). While you are typing in a text field, Ctrl+Z and Ctrl+Y belong to that field and undo your typing there.

Undo covers the choices you make in a study: the blanket and shield allocation, the port or no-port arrangement, the operating preset and every what-if value, the timeline year, the field view and plot, the allocation picked in the sweep, the Core study selections, which components are hidden, the cutaway, whether Core evidence is stored in saved files, and the interface size. It does not cover moving the camera, selecting a component, or switching between the five steps, because those change what you look at, not the study.

One gesture is one step. Dragging a slider, typing a value and clicking a checkbox each undo in one go. Hovering and selecting never make a step.

Undo and redo take effect at once. When your change goes back to inputs FARIS has already calculated, the earlier result is shown again with no new calculation. After a new calculation has started, going back and forward between the last eight sets of inputs is instant.

### The edit history

Choose **Edit** > **Edit history…** to see every change made in this session, newest first, each with a plain label and the time. Click a line to go to the state after that change. The line marked "You are here" is the current state. Lines in grey are changes you undid; making a new change removes them.

The list is limited by memory, not by a short count: it keeps up to 10,000 steps or 500 MB, whichever comes first. If the limit is reached, the oldest step is dropped and the status line tells you, so a change is never forgotten silently. Opening another study starts a new list, and the status line says the old one was cleared.

Below the list, **Settings changes** shows the last 100 settings changed in the window, with the time, the setting name, the old and new value, and where the value is kept. Undo restores the exact earlier value and adds its own line.

### What undo cannot take back

Undo changes what the window holds. It does not change files or running jobs:

- **Saving** writes the study file. Undo does not unwrite it. The Save entries say this before you use them, and the history list notes the save.
- **Exporting** writes files into a folder. Undo leaves them there.
- **A transport run** is a program running on your computer. Undoing past the start of a run stops it; it never rewinds what the run already did.

Each of these appears in the history as a grey note, and the status line repeats it when an undo passes one.

## Automatic recovery

While you have changes that are not saved, FARIS writes them to a recovery copy every 20 seconds, so a crash or a power cut loses at most about 30 seconds of work. The copy is a separate file in FARIS's own folder (see below). It is never written over your study file. A recovery copy is written in one piece: if the computer stops while it is being written, the previous complete copy is still there.

The copy is deleted when you save, and when you quit normally. If it is still there the next time FARIS starts, FARIS tells you it did not close normally and asks:

- **Restore** reopens the study that was open and puts your unsaved changes back. The study is marked as changed, so closing it asks to save.
- **Discard** deletes the recovery copy.
- **Decide later** keeps it and asks again at the next start.

If the study file has moved or been deleted, Restore is not offered, because the changes have nowhere to go; the message says which file is missing.

If FARIS cannot write the recovery copy (for example, the disk is full), the status line says why and what to do. Saving the study always works on its own.

Recovery keeps the study, the view and the changes listed under Undo above. It does not keep the undo history, a transport run in progress, or text typed into a window but not yet entered.

## Earlier versions of a saved study

Each time you save over an existing study file, FARIS first copies the file that is there into a folder beside it named after the file, for example `plant.faris.backups`. The copy is byte for byte, so a restored version has exactly the hash the file had when you saved it. The newest 3 versions are kept; the setting `backups.keep` changes the number (1 to 100). See [Settings](settings.md).

To go back, choose **File** > **Earlier versions** and pick one by its age. FARIS replaces the study file with that version and reopens it. The version it replaced is kept too, so a restore can itself be undone from the same menu. The menu is disabled while you have unsaved changes: save or discard them first.

If the earlier version cannot be kept (for example, the folder is read-only), the save still goes ahead and the status line says the backup was not made and why.

## Upgraded studies are new files

If a study file was written by an older version of the study format, FARIS never rewrites it in place. Saving writes a new file beside it named like `plant (upgraded).faris`, and leaves the original with the bytes it had. The status line says where the new file is. Studies written by the current format are saved in place, as usual, with the backup above.

## Preferences and caches are disposable

You can delete FARIS's configuration folder, its caches and its scratch folders at any time. A study always opens without them; the settings go back to their defaults, and you only lose the saved tool paths, the interface size and any crash records or recovery copy.

## Crash records

If the program stops on an internal error, FARIS writes a short record on your computer and tries to save the recovery copy one last time. Choose **File** > **Crash records and diagnostics…** to see the records.

A record holds the FARIS version, your operating system and processor type, an error code, the place in the program code, and the names of the program's functions. It never holds study content, file names, folders, names you typed, geometry, or the error text (which can quote a file name). There is no upload feature: FARIS sends nothing, and you attach a file to a bug report only if you choose to.

Records are deleted after 30 days, and only the newest 20 are kept. The setting `crash.keep_days` changes the time. **Clear now** deletes every record at once. The same rule removes old diagnostics files and old recovery copies.

### The diagnostics bundle

**Make a diagnostics bundle** writes one file you can read before you share it. It lists what it holds first, then the program and system details, the settings that are set (a path is shown only as "set"), whether a recovery copy exists, and the crash records. Anything that looks like a path, an e-mail address, a key or your user name is replaced by `[redacted]`, and the window says how many items that was. The file is kept in the `diagnostics` folder of the configuration folder.

## Where things are kept

FARIS keeps its own files in its configuration folder: `~/.config/faris` on Linux (or under `$XDG_CONFIG_HOME`), `~/Library/Application Support/faris` on macOS, and `%APPDATA%\faris` on Windows. Set `FARIS_CONFIG_DIR` to a folder to use another place.

| Folder | What it holds |
| --- | --- |
| `recovery` | The recovery copy of unsaved changes, one file per running FARIS |
| `crashes` | Crash records |
| `diagnostics` | Diagnostics bundles you made |

Earlier versions of a study are kept beside the study itself, not in this folder.

## What this version does not do

- Undo does not cover the transport run settings, the Maintenance, Design check and Design flux windows, or the tool paths in the settings file. Changes there are kept, but not in the history.
- The recovery copy is checked in tests with simulated kills; a kill of the real window was not tried by hand in this version.
- Two copies of FARIS running at once each keep their own recovery copy. On macOS and Windows, FARIS cannot tell whether another copy is still running, so it may offer the other copy's recovery file at start.
- There is no way yet to send a crash record to Avila Labs from the program.
