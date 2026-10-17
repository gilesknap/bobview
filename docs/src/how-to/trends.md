# Trend PVs

A trend window plots one or more PVs against time.

## Open a trend

- Right-click a widget (long-press on a touch screen) and pick **Trend** for
  its PV, or **Trend with rule PVs** for its PV and the PVs its rules read.
  The same menu adds them to a trend already open.
- In a BOY screen, a button that would open a Data Browser or StripTool
  opens a trend of its PVs instead (see [How BOY screens are translated](../explanations/boy.md)).

Several trends may be open at once, each in its own window over the display,
and they stay open as you move between displays. Layouts save them.

## Use a trend

- Pick the time span from the window's menu: 1 minute to 1 day (10 minutes
  by default). The value axis fits the data.
- Switch **Log scale** on for a value axis on a log scale.
- The legend below the plot gives each PV's colour, name and current value,
  or Disconnected. **✖** removes a PV, and **Add PV** adds one.

## Limits

A trend is live only: each PV gets a sample on every update from when the
window opened (up to 100 000 samples, and a day, per PV), with no archived
history. Each value holds until the next, and a gap marks where a PV was
disconnected.
