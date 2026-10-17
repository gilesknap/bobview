# The window and its controls

## Main toolbar

From the left: **Back**, **Forward**, **Home** and **Reload**; zoom out,
the zoom, zoom in and **Fit**; the **navigator** and **Open…**; the
display's name; and on the right the window's **pin**, the **layout list**
and **☰**, a menu of the rest (Auto-size, Bring all windows to front, Close
all other windows and the layout actions).

- Clicking the zoom shows the display at 100% and sizes the window to it
  (and the navigator, when shown).
- A narrow window keeps Back, Home, the zoom and Fit, and moves the rest
  into ☰.
- On a small high-DPI screen the toolbar and menus are scaled down so they
  take a small share of it; `--ui-scale F` (or `BOBVIEW_UI_SCALE=F`) fixes
  their size.

## Window size

The window opens at the size of the first display (when the screen has
room). With **Auto-size** (in ☰, on by default) it is resized to each
display you open, unless it is maximised or full screen; turn it off to keep
the window's size and scale each display to fit. `-z ZOOM` fixes the zoom
instead of fitting the window.

## On a widget

- **Hover** to see its PV and value.
- **Right-click** (long-press on a touch screen) to copy the PV name, its
  value or a `caget`/`pvget` command, open the PV details, **Trend** it,
  **Trend with rule PVs**, or add it to an open trend. See
  [Trend PVs](../how-to/trends.md) for the trend window's own controls.
- **Middle-click** to copy its PV name, as in EDM and MEDM: a middle-click
  in a terminal then pastes it, and one on a
  [trend](../how-to/trends.md) or in
  [archiveview](https://github.com/gilesknap/archiveview) plots it (a
  Channel Access, PV Access, `loc://` or `sim://` name; for a formula use
  **Trend**). The
  widget flashes and the status line says "Copied *name*". See
  [Copying a PV name](#copying-a-pv-name).
- On a touch screen, pinch to zoom.

## Copying a PV name

A middle-click on a widget, or **Copy PV name** in its right-click menu,
puts the PV name on both the clipboard (for Ctrl+V) and the primary
selection (for a middle-click, on Linux). The name is the one other tools
take:

- macros expanded, and a Channel Access name bare and whole
  (`BL01T-EA-DET-01:COUNT`, not `ca://BL01T-EA-DET-01:COUNT`);
- a PV Access name keeps its `pva://`;
- a `loc://` PV keeps its prefix but not its type and initial value
  (`loc://x`, not `loc://x<VDouble>(1.5)`); a `sim://` PV is copied whole,
  its parameters too (`sim://sine(0, 10, 0.1)`);
- a formula copies the PVs it reads, each named as above, not its text.

A widget copies its own PV only, not the PVs its rules read. An XY plot or
strip chart, which has no PV of its own, copies its traces' PVs. Several
names are separated by spaces.

On X11 bobview holds the primary selection itself, so it is lost when
bobview exits, as with any X application; this works over `ssh -X` too. On
Wayland bobview uses the compositor's data-control protocol where it has
one, and XWayland's primary selection otherwise. Windows, macOS and the
browser have no primary selection, so there the name goes to the clipboard
only (in the browser, if it allows the page to write to the clipboard).
Middle-click in the browser is untested, and the browser may start
autoscrolling on it.

## Display windows

A display window (one opened beside the main window; see
[Pinned windows and the viewer](../explanations/windows.md)) has the native
title bar only: its toolbar (Back, Forward, Reload, zoom and Home) is hidden
until you show it. While the pointer is over the window, small buttons
appear in the top-right corner of the display: Back (while the toolbar is
hidden), the window's pin, and a chevron that shows or hides the toolbar.
The right-click menu on the display's background has **Show toolbar** and
**Close**. Pinned windows have 📌 before their title.

## Keys

| key | action |
| --- | --- |
| Ctrl+R | reload |
| Alt+← / Alt+→ | back / forward |
| Ctrl+W | close the window (the main window quits bobview and closes its windows) |
| Ctrl+T | show or hide a display window's toolbar |
| shift+click on a link | open in a new window |
| ctrl+click on a link | open in place |

## Navigator

The navigator button shows, beside the main window's display, a tree of the
displays the home display links to, the displays those link to, and so on.

- A display's links are its widgets' Open display actions, including those
  in groups, tabs and embedded displays, each display and macros once,
  named by its button's text (or, in a menu, the action's description).
- A link with no text of its own, or the same text as another link on its
  display (a row of identical device symbols), is named by the display it
  opens once that has been read: by the words of its title that macros fill
  in, such as `LB-VA-IONP-04` from `Ion Pump Controller - $(device)`, else
  by its whole title. The tooltip gives the title, file and macros.
- Clicking a node opens its display as a link in the main window would, so
  with the main window pinned it goes to the viewer; shift and ctrl work as
  for a link. Displays open in a window are in **bold**.
- Displays are read from their files, without connecting their PVs, as
  their nodes are shown, so the tree is read one level ahead of what is
  open. A display that links back to one above it is marked ↻ and not opened
  again; one that can't be read is marked ⚠. A command action's display (a
  DLS launcher script) is found by running its script, so only when its node
  is opened or clicked.
- The filter box lists the displays read so far whose name or file matches;
  ⟳ reads them all again.
- Showing or hiding the navigator widens or narrows the window by its width
  (unless maximised or full screen), so the display keeps its size and zoom.

## Status line

Shows write errors and refusals, what a copy has just put on the clipboard,
and counts "N widgets with scripts ignored" and "N unsupported widgets" for
a display that has them.
