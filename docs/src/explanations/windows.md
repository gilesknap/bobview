# Pinned windows and the viewer

For a control room, several displays can be open side by side, each in a
window of its own, and the arrangement saved as a
[layout](../how-to/layouts.md). The question is where a link opens its
display: in place, or in another window? bobview answers it with one idea:
each window is **pinned** or not.

## The rule

A link (an action or menu button, a symbol click …) opens its display by
these rules, whatever target the screen gives it:

| Click | Display already open somewhere | Not open |
|---|---|---|
| in a pinned window | raise that window | open in the viewer, or a new window if every window is pinned |
| in an unpinned window | raise that window | open in place |
| **shift**+click | raise that window | open in a new window |
| **ctrl**+click | open in place | open in place |

The **viewer** is the unpinned window used last (focused, opened or
unpinned), so clicking a window once makes it where the next link goes.
"Already open" means the same display with the same macros, in another
window. The macros are all those the display would be opened with: the
link's own, and those of the group, widget or embedded display it is on, so
each gauge of a cell, opened through one gauge screen, gets its own window. A BOY
display's `$(DID)` doesn't count, since each window gives it its own.

## Why it works this way

The main window starts pinned, so it stays where it is while the displays
it links to come and go in one other window: an overview on one side and a
detail on the other. Pin that one too to keep it, and the next link opens a
new viewer. Unpinned, the main window navigates in place, as a single
window does. Holding shift opens every display in a window of its own, as
the old multi-window mode did.

A display opened in place or in the viewer keeps that window's history for
Back and Forward. A link to the display its own window shows reloads it in
place, as a Refresh button wants, without adding to Back. BOY Exit buttons
close the display window they are in (in the main window they go back, or
quit bobview when there is nothing to go back to).

In a browser, and on a touch or small screen, displays have no windows of
their own: every link navigates in place, and the pin and layout controls
are not shown.

## Where new windows go

A display window opens sized to its display at 100%, within the monitor,
beside the window pinned last, so a row of small windows grows beside a big
one: on the first of its right, below it, its left and above it where there
is room on the monitor clear of the pinned windows. Failing that it goes
beside the window whose link opened it, then beside the other pinned
windows, or where it covers them least. The viewer is sized and placed so
again for each display a pinned window's link opens in it.

Wayland doesn't let applications place their windows, so there the
compositor does.

## Redrawing

Each window is redrawn on its own: for its own input, and when a PV its
display shows changes (at most 10 times a second), not when another window
is. That matters most over `ssh -X`, where each redraw sends the whole
window over the network.

The controls of each window are listed in
[The window and its controls](../reference/controls.md).
