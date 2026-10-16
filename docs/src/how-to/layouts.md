# Save and restore a layout

A layout records the open windows so that a control-room arrangement can be
put back with one command. [Pinned windows and the viewer](../explanations/windows.md)
explains how windows are opened in the first place.

## Save

Arrange the windows, then use the main toolbar's **☰** menu:

- **Save layout as…** names a new layout (asking before replacing one).
- **Save layout** saves to the layout last loaded or saved.
- **Save as default** makes it the layout bobview starts with.
- **Save default on exit** (off unless ticked) saves the default each time
  you quit.
- **Delete layout** removes one.

## Restore

Pick a layout from the toolbar's layout list; this closes the display and
trend windows open now. From the command line:

```bash
bobview                     # restores the default layout if there is one
bobview DISPLAY             # opens DISPLAY; the default layout is not restored
bobview --layout rf-shift   # restores layouts/rf-shift.json
bobview --no-layout         # ignores the default layout
```

`--snapshot` ignores layouts.

## What a layout holds

For each window: its display, Eclipse project and effective macros, its
position and size, whether it is pinned, and whether a display window shows
its toolbar; for the main window also its zoom and whether it shows the
navigator. For each trend window: its PVs, time span, log scale, position
and size. A layout from before windows were pinned pins them all if it was
saved in multi-window mode, and none if not.

Layouts are JSON files in `$XDG_CONFIG_HOME/bobview/layouts` (by default
`~/.config/bobview/layouts`), so they can be copied between machines or
kept under version control.

## Positions on Wayland and with changed monitors

A saved position that is on none of the monitors now (a laptop that was
docked) is dropped and the window manager places the window.

**Wayland** does not let an application read or set its windows' positions,
so on a Wayland desktop a layout restores the windows and their sizes but the
compositor places them, and bobview logs this once. To keep positions, run
bobview under XWayland:

```bash
WAYLAND_DISPLAY= bobview
```

X11 desktops, including `bobview-ssh` over X forwarding, restore positions.
