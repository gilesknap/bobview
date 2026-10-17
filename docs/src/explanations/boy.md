# How BOY screens are translated

## One renderer for both formats

An `.opi` file (CS-Studio BOY) is translated as it is read into the `.bob`
form, the way Phoebus's own legacy import converts it, so it is drawn by the
same widgets as a `.bob`. It is accepted everywhere a `.bob` is: on the
command line, in **Open…** (which lists `.opi` files too), in actions and
linking containers, and over http(s). As in Phoebus, a `.bob` next to a named
`.opi` file is opened in its place.

How each BOY widget is drawn is listed in [BOY widgets](../reference/boy-widgets.md).

## Colours, fonts and rules

Colours keep the RGB the file gives; a colour with only a name is looked up
in the EDM `colors.list` DLS converted its screens with, then in the Phoebus
palette. Fonts in pixels keep their size, fonts in points are divided by
1.01, and the Phoebus named fonts (`Default`, `Header 1` …) use the Phoebus
sizes. Rules keep BOY's severity numbers (`pvSev0` becomes `pvLegacySev0`)
and `widget.getValue()` reads the widget's own PV.

## Size and clipping

An `.opi` keeps its declared width and height, and widgets outside them are
clipped, as in BOY. EDM conversions often leave stray widgets far off the
display, which Phoebus (and bobview, for a `.bob`) would grow the display to
show. This size is also what Fit, `--size auto` and an embedding container
that sizes itself to its content use.

## Scripts that are done natively

Scripts and JavaScript or Python actions are not run, with these
exceptions, recognised and done by bobview itself:

- The EDM **Exit** button's script (`ScriptUtil.closeAssociatedOPI`) goes
  back to the previous display, or closes the window.
- A script that opens a **Data Browser** on some PVs
  (`ScriptUtil.executeEclipseCommand` of
  `org.csstudio.trends.databrowser2.newWindow` with `['pvnames', 'A,B']`, as
  the DLS vacuum "Graph" buttons do) opens a [trend](../how-to/trends.md) of
  them.
- The DLS vacuum widget script `get_pv_name.py`, recognised by its file
  name. Its second PV, a vacuum space's link field such as
  `SR03S-VA-SPACE-01:IONPSTA.INPA`, names a group
  (`SR03S-VA-GIONP-01:STA NPP NMS` names `SR03S-VA-GIONP-01`). Each time it
  names a different one, a label shows the group's name, and any other
  widget reads the group's `:STA` and takes its name, so its tooltip,
  right-click menu and the `$(name)` its click passes as `device` follow. An
  EDM symbol of a gauge, ion pump or valve strip shows the frame for that
  status (0 when disconnected).

A click on a shape, symbol or LED with a BOY click hook runs its actions. A
disconnected EDM symbol shows its first frame inside the disconnected
border, without Phoebus' overlay.

## Eclipse projects and DLS launchers

BOY screens name each other through the CS-Studio workspace: an Eclipse
project whose `.project` links folder names to the directories that hold
each module's screens. bobview resolves these links itself.

DLS modules have no `.project` on disk, but the generated launcher
`runcss.sh` beside a module's screens lists the links it gives CS-Studio
(`links="${SCRIPT_DIR}=${project}/${module},…"`). bobview reads that list
(it does not run the script, which needs `configure-ioc`), taking `version`
from the module's `configure/VERSION` (else `dev`). So
`…/vacuum/4-100/vacuumApp/opi/opi/diamondVacuumOverview.opi` opens as
`…/vacuumApp/opi/opi/runcss.sh/vacuum_4-100/vacuum/diamondVacuumOverview.opi`:
the CS-Studio path `/vacuum_4-100/vacuum/diamondVacuumOverview.opi` below the
launcher, and links to other modules work.

This is for BOY screens only: a `.bob` (even one Phoebus-style in place of an
`.opi`, or one picked in a launcher project's folder) goes by its real path.
A `.project` nearer the screen than the launcher, or beside it, wins when it
links the screen (an empty one, as many DLS modules have, does not).

The steps to open such screens are in
[Open BOY screens from an Eclipse project or DLS module](../how-to/opi-projects.md).

## Launcher commands

DLS screens open others with command actions that run a launcher script,
such as `./srCellVacuum 04`, which ends by running `runcss.sh DISPLAY -m
MACROS` to start CS-Studio. For a local display, bobview runs a copy of such
a script beside a stand-in `runcss.sh` that only records its arguments, and
opens that display with those macros. These are shell scripts, so they are
not run on Windows, where such commands do nothing.

A command that starts StripTool (`strip.py`, `strip.sh` or
`buildStripToolconfig.sh` and the PV names, as in the DLS vacuum, gauge and
RGA screens) opens a trend of those PVs instead; a last word without a `:`,
such as `log_mbar`, is ignored. Other commands are not run.

Running scripts is limited for safety; see
[Writes and safety](writes.md#commands).
