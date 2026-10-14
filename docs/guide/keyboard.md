# Keyboard

Every action in FARIS can be reached and run from the keyboard. This page lists the keys. On macOS, read Ctrl as Cmd.

## The command palette

Press Ctrl+K anywhere to open the command palette. Type to filter (any word of the command's name or a related word matches, so "transport" finds Simulate), press Up and Down to choose, Enter to run and Esc to close. A command that cannot run now is shown with the reason, and running it puts the reason in the status line instead of doing nothing.

## Commands

Each row is a palette entry. Where a key is shown it also works on its own, without opening the palette. The digit keys do not act while you are typing in a text field or while the tour is playing.

| Command | Key |
| --- | --- |
| Open the command palette | Ctrl+K |
| Go to step 1: Design | 1 |
| Go to step 2: Simulate | 2 |
| Go to step 3: Operate | 3 |
| Go to step 4: Compare | 4 |
| Go to step 5: Evidence | 5 |
| Open a study file… | Ctrl+O |
| Save the study | Ctrl+S |
| Save the study as… | Ctrl+Shift+S |
| Store the Core evidence inside the saved file (toggle) | none, use the palette |
| Export the brief, tables and charts… | none, use the palette |
| About this browser version (browser version only) | none, use the palette |
| Focus the 3D viewport (arrows orbit, Shift+arrows pan, + and − zoom, 0 resets, Enter reads out) | none, use the palette |
| Reset the 3D view (frame scene) | none, use the palette |
| Cutaway on or off (toggle) | none, use the palette |
| Select the next component | none, use the palette |
| Select the previous component | none, use the palette |
| Show or hide the selected component (toggle) | none, use the palette |
| Switch to the next blanket and shield allocation | none, use the palette |
| Switch between the port and no-port arrangement | none, use the palette |
| Timeline: one year later | none, use the palette |
| Timeline: one year earlier | none, use the palette |
| Timeline: go to year 0 | none, use the palette |
| Timeline: go to the end of the horizon | none, use the palette |
| Read out the tritium breeding ratio and its error | none, use the palette |
| Explain why a value is NOT_EVALUATED and what would settle it | none, use the palette |
| Maintenance window (toggle) | none, use the palette |
| Design check window (toggle) | none, use the palette |
| Design flux window (toggle) | none, use the palette |
| Replay the guided tour | none, use the palette |
| Interface size: larger | none, use the palette |
| Interface size: smaller | none, use the palette |
| Interface size: 100% | none, use the palette |

The interface size also answers to the keys your desktop gives every window: Ctrl and + or −, and Ctrl 0.

## Tab and the controls in each panel

Tab and Shift+Tab move between controls. Space or Enter presses a button or toggles a checkbox, arrows change a slider or move in an open list, and a text field takes typing. This covers the run settings and Run buttons, the what-if, sensitivity and ensemble controls, the Core stage buttons, the allocation sweep controls, the timeline's plot choice, series buttons, year slider and event list, and the fields and buttons of the Maintenance, Design check and Design flux windows.

## The 3D viewport and the timeline plot

Two drawings take the keys themselves once they have focus. Press Tab until a frame appears around the drawing, or run "Focus the 3D viewport" from the palette. Scrolling with the mouse wheel or trackpad over the timeline plot does nothing; use the keys or drag inside it.

| Where | Keys | What it does |
| --- | --- | --- |
| 3D viewport | Arrow keys | Orbit |
| 3D viewport | Shift+Arrow keys | Pan |
| 3D viewport | + or = | Zoom in |
| 3D viewport | - | Zoom out |
| 3D viewport | 0 | Reset the view |
| 3D viewport | Enter or Space | Read out what is at the centre of the view |
| Timeline plot | Left or Right | Move the cursor year by 0.1 |
| Timeline plot | Shift+Left or Shift+Right | Move the cursor year by 1 |
| Timeline plot | Page Up or Page Down | Move the cursor year by 5 |
| Timeline plot | Home or End | Go to the start or end |

Enter or Space in the 3D viewport selects the component at the centre of the view, shown by a small cross, and writes what it shows into the status line at the bottom of the window. In the flux slice view it writes the bin's value and standard error. While the timeline plot has focus, a text line under it gives the cursor year and each arrangement's value there. Both drawings have a text description for screen readers.

## What is not covered yet

The guided tour is operated by Right or Enter, Left and Esc. The file and folder dialogs are the desktop's own and have their own keyboard use. The optional account controls are not in the command palette.
