# Trend PVs

A trend window plots one or more PVs against time: their archived history
from an [EPICS Archiver Appliance](https://github.com/archiver-appliance/epicsarchiverap),
joined to their live values. The plot is
[archiveview](https://github.com/gilesknap/archiveview)'s, so a trend in
bobview works as the standalone `archiveview` does.

## Open a trend

- Right-click a widget (long-press on a touch screen) and pick **Trend** for
  its PV, or **Trend with rule PVs** for its PV and the PVs its rules read.
  The same menu adds them to a trend already open.
- In a BOY screen, a button that would open a Data Browser or StripTool
  opens a trend of its PVs instead (see [How BOY screens are translated](../explanations/boy.md)).

Several trends may be open at once, each in its own window over the display,
and they stay open as you move between displays. Layouts save them.

## Give bobview an archiver

Without an archiver a trend is live only: each PV's line starts when the
window opens. To see history, name the appliance's retrieval service:

```bash
bobview --archiver http://archiver.example.com:17668 DISPLAY
```

`/retrieval` is added unless the URL ends with it, and Phoebus's
`pbraw://host:port/retrieval` form works too. The archiver is chosen in this
order:

1. `--archiver URL`, which is also remembered for later runs (not with
   `--snapshot`);
2. none with `--no-archiver`, for this run only;
3. the `BOBVIEW_ARCHIVER` environment variable (not remembered), for a
   site-wide default set in a launcher script or profile;
4. the last archiver given with `--archiver` or picked in a trend's archiver
   pill, from `settings.json` beside the [layouts](layouts.md).

The pill in each trend's top bar shows the archiver and whether it can be
reached (green), is being checked, or cannot be reached (red). Click it to
switch to another archiver, or to none; the switch applies to every trend
window, and is remembered for the next run unless `--archiver` or
`BOBVIEW_ARCHIVER` names another.

To try it without an appliance, run archiveview's mock archiver, which
serves synthetic history for the test IOC's `bl01t:SUM` and a few
`MOCK:` PVs:

```bash
cargo install archiveview-mockarchiver
archiveview-mockarchiver &
bobview --archiver http://127.0.0.1:17668
```

## Use a trend

- **Time.** The top bar's presets show the last hour, 8 hours, day or week;
  **⋯** takes a span such as `90m` or `2 days`, or two dates. Drag to pan
  and scroll to zoom time about the pointer. Moving into the past leaves
  **Live**; click it (or press `L`) to follow now again.
- **Values.** Each value axis fits its data until you zoom it: drag an axis,
  or shift-scroll over it. PVs with different units or scales get axes of
  their own (up to four), on a log scale where that suits the values.
  Double-click the plot to reset.
- **PVs.** Type in the search box to add a PV; with an archiver it also
  searches the archived names. The legend chips under the plot give each
  PV's colour, name and value: click one to hide its line, hover to pick it
  out, and **×** to remove it. Ctrl+V over the plot adds the PVs named in
  the clipboard.
- **Middle-click.** On Linux (X11 or Wayland) a middle-click on the plot or
  legend adds the Channel Access and PV Access PVs named in the primary
  selection, as in the standalone archiveview: middle-click a widget in a
  display to select its PV name, or select names in a terminal or editor,
  then middle-click the trend. Other PVs, such as `loc://` and `sim://`
  ones, aren't added this way: use **Trend** in the widget's right-click
  menu. A middle-click in the search box pastes the selection there, and
  text selected in the search box becomes the primary selection. A note over
  the plot says what was added, or that the selection named no PVs (one
  naming more than 20 adds none). Windows and macOS have no primary
  selection, so there use Ctrl+V; in a snapshot a middle-click on a trend
  pastes nothing.
- **Undo.** **↶** and **↷** (or Ctrl+Z and Ctrl+Shift+Z) undo and redo
  changes to the plot.
- Hover over the plot for a readout of every PV at that time.

## Limits

- A PV the archiver doesn't hold is plotted live only, and its chip says so.
- Waveforms are not plotted: their chip shows the latest array.
- In the browser build trends are live only: a browser can't reach the
  archiver, and a middle-click pastes nothing: use Ctrl+V.
