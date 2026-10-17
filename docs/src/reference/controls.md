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
  **Trend with rule PVs**, or add it to an open trend.
- On a touch screen, pinch to zoom.

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

Shows write errors and refusals, and counts "N widgets with scripts ignored"
and "N unsupported widgets" for a display that has them.
