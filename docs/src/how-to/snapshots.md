# Take snapshots

`--snapshot` renders a display to a PNG without opening a window, then
exits. It is useful for documentation, regression checks and CI.

```bash
bobview --snapshot out.png screen.bob
```

## Size and scale

```bash
bobview --snapshot out.png --size 1920x1080 screen.bob
bobview --snapshot out.png --size auto screen.bob       # as a window manager would size it
bobview --snapshot phone.png --scale-factor 2.75 screen.bob  # as on a high-DPI phone
```

`--size` defaults to `1280x800`. `auto` starts at the window's default size
and resizes it as a window manager would, on a monitor of 1920x1080 points.

## Wait for PVs

PVs get 2 seconds to connect by default; give more with `--wait`:

```bash
bobview --snapshot out.png --wait 5 screen.bob
```

## Click first

`--click X,Y`, `--right-click X,Y` and `--middle-click X,Y` (repeatable,
in the order given) interact with the display before the snapshot, which is
taken `--settle` seconds (default 1) after the last click. Positions are in
pixels.

```bash
bobview --snapshot menu.png --right-click 200,150 --settle 0.5 screen.bob
```

## Snapshot every test display

CI does this against `bobview-testioc`:

```bash
bobview-testioc &
export EPICS_CA_ADDR_LIST=127.0.0.1 EPICS_CA_AUTO_ADDR_LIST=NO
for f in tests/displays/*.bob; do
  bobview --snapshot "snapshots/$(basename "$f" .bob).png" --size 1280x800 --wait 5 "$f"
done
kill %1
```
