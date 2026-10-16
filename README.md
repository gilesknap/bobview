# bobview

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens, written in Rust with [egui](https://github.com/emilk/egui).
It also opens the older CS-Studio BOY `.opi` screens (see [BOY `.opi` displays](#boy-opi-displays)).
It shows live EPICS Channel Access and PV Access values, including areaDetector
images (decimated when a side is over 2048 pixels, to keep drawing fast), and is small enough to run on a phone, in the Android 16 Linux Terminal (Debian 13, aarch64).

Scripts (Jython/JavaScript), display editing, alarm and archiver services are
out of scope. The status line shows "N widgets with scripts ignored" (`.bob` and `.opi`,
counting widgets, not scripts; the DLS vacuum group-link script is run, so not
counted) and "N unsupported widgets" for a display that has them.

Fidelity to Phoebus is approximate: sizes, fonts (scaled by 0.95) and layout
were tuned by eye, and output has not been compared with Phoebus's own.

## Install

On the phone (Android Linux Terminal), an x86_64 Linux desktop or a Mac
(Apple silicon or Intel, macOS 11 or later); `install.sh` picks the aarch64
or x86_64, Linux or macOS release build to match the machine.

Pre-requisites on Debian and Ubuntu:
```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

On RHEL, Rocky, Alma and Fedora:
```bash
sudo dnf install libxkbcommon libwayland-client libglvnd-egl mesa-dri-drivers
```

None are needed on macOS.

Install:
```
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

This installs into `~/.local`:

| file | what |
| --- | --- |
| `bin/bobview` | the viewer |
| `bin/bobview-ssh` | runs the viewer over `ssh -X`, see below |
| `bin/bobview-testioc` | a pure-Rust IOC serving the example-services BL01T PVs |
| `bin/bobview-probe` | prints live PV values from the command line |
| `bin/bobview-server` | serves bobview to browsers, see [In a browser](#in-a-browser) |
| `share/bobview/displays/` | test screens; `index.bob` opens by default |
| `share/bobview/web/` | the browser build that `bobview-server` serves |

On macOS `~/.local/bin` is not on `PATH` by default; add
`export PATH=$HOME/.local/bin:$PATH` to `~/.zshrc`.

## Run

```bash
bobview-testioc &                    # simulated BL01T IOCs on this machine
EPICS_CA_ADDR_LIST=127.0.0.1 bobview # opens share/bobview/displays/index.bob
bobview path/to/screen.bob -m P=BL01T,R=:DET:
bobview http://host:8080/opi/demo.bob  # displays can be served over http(s)
bobview /path/to/project/module/screen.opi  # a BOY screen, through an Eclipse project
```

A display can be a file or an `http://` / `https://` URL, on the command line
or typed into Open…. Files it names (buttons, embedded displays, navigation
tabs) are found relative to it, so a directory of screens served by any web
server (e.g. `python3 -m http.server`) works as it does from disk. Fetched
screens are kept in memory for Back and Forward; Reload fetches them again.

`bobview-testioc` serves the PVs of the
[epics-containers example-services](https://github.com/epics-containers/example-services)
beamline natively, so the amd64 IOC containers are not needed on the phone:
`bl01t-ea-test-01` (its `ioc.db` verbatim), two motor records on a simulated
axis, and a simulated area detector whose image is served over PV Access as
`BL01T-DI-CAM-01:PVA:OUTPUT`. Every record is also served over PVA.
See [crates/testioc](crates/testioc/README.md).

To reach real IOCs from the phone instead, point the clients at them with the
standard variables: `EPICS_CA_ADDR_LIST` / `EPICS_PVA_ADDR_LIST` for UDP
search, or `EPICS_CA_NAME_SERVERS=host:port` / `EPICS_PVA_NAME_SERVERS=host:port`
for gateways reached over TCP (this works through the phone VM's NAT, since
the phone opens the connection). The example-services `ca-gateway` listens on
port 9064 of its host. PVs prefixed `pva://` use PV Access; others use Channel
Access.

Protocol correctness rests on the third-party
[epics-rs](https://github.com/epics-rs/epics-rs) `epics-ca-rs` and
`epics-pva-rs` 0.31 crates, which are young. bobview's own `ca.rs` and `pva.rs`
are thin adapters over them. So far bobview has only been exercised against the
in-repo Rust `bobview-testioc`, not a real softIOC, PVXS or ca-gateway; the
`interop` CI job and `crates/pv/tests/interop.rs` are there to start changing
that.

Over `ssh -X` or `-Y`, run `bobview-ssh` (same arguments) instead. Forwarded
OpenGL reaches your local X server as indirect GLX, which current X servers
refuse, so `bobview` exits with `glutin error: [2] BadValue`. `bobview-ssh`
makes Mesa render in software on the remote machine (`LIBGL_ALWAYS_SOFTWARE=1`,
and the glvnd vendor forced to Mesa so a GPU driver such as NVIDIA's is not
picked instead). It needs Mesa there: `mesa-dri-drivers` on RHEL,
`libgl1-mesa-dri` on Debian.

Options: `-z ZOOM` fixes the zoom instead of fitting the window, `-f` starts
full screen, `--snapshot out.png` renders headless and exits (with `--size WxH`
or `--size auto`, `--scale-factor F` to render as on a high-DPI screen,
`--wait SECONDS`, and `--click X,Y` or `--right-click X,Y` to interact
first, with `--settle SECONDS` after the last click), `BOBVIEW_LOG=debug` gives more output. The window opens at the size
of the first display (when the screen has room). With Auto-size (🗖 in the
toolbar, on by default) it is resized to each display you open, unless it
is maximised or full screen; turn it off to keep the window's size and scale
each display to fit. The toolbar, menus and keypad are scaled down on a
small high-DPI screen so that they take a small share of it; `--ui-scale F`
(or `BOBVIEW_UI_SCALE=F`) fixes their size, 1 being the desktop's own scale. Hover over a widget to see its
PV and value; right-click it (long-press on a touch screen) to copy the PV
name or value, open the PV details, or trend it (see [Trends](#trends)). On a touch screen, pinch to zoom.

### Writes

`--read-only` refuses every write to a PV: controls are disabled and a write
action says in the status line why it did nothing. `loc://` PVs are exempt,
as they live inside bobview and cannot reach a control system. PV Access
does not report access rights, so such a PV is assumed writable (the PV
details say "unknown") and a put the server refuses is shown as a write
error in the status line and the PV details. A `write_pv` action (or the
toggle script) of a display loaded from an `http://` or `https://` URL asks
for confirmation before it writes; displays from local files write at once.

## What it draws

| widgets | notes |
| --- | --- |
| label, textupdate, textentry | formats: default, exponential, hex, binary, string; units, precision |
| led, multi_state_led, byte_monitor | |
| action_button, bool_button, choice, radio, combo, checkbox | open display and write PV actions |
| scaledslider, scrollbar, spinner | |
| linearmeter | scale, warning regions from the levels (the PV's alarm limits with `limits_from_pv`), limit markers, needle; horizontal or vertical |
| meter | a dial: scale (linear or `log_scale`), warning regions from the levels (the PV's alarm limits with `limits_from_pv`), needle |
| progressbar, tank, thermometer | one shared bar renderer (tank and thermometer are vertical bars with their own default fill colours) |
| xyplot | line, step and bar traces with markers against index or an x PV; several y axes, autoscale, log scale, grid, legend |
| stripchart | its traces' PVs over the last `time_range`, sampled from when the display opened, on the first y axis (autoscale or fixed, log scale, grid), legend |
| rectangle, ellipse, arc, polyline, polygon | dashed and dotted `line_style` for rectangles and polylines |
| group, tabs, embedded, navtabs | macros pass down; embedded resize modes 0–2 and 4 (crop) |
| image | PVA NTNDArray (uncompressed, mono or RGB1) or a CA array shaped by `data_width`/`data_height`; Phoebus colour maps, autoscale or fixed range, `unsigned`, colour bar with a scale; toolbar to zoom, pan and place a crosshair that reads out the pixel value |
| symbol, picture | SVG or PNG files, local or fetched next to the display; the PV value picks the symbol (`initial_index`, `array_index`, `fallback_symbol`; an empty one draws nothing), rotation, `preserve_ratio`/`stretch_image`, opacity, overlay for a disconnected PV; `run_actions_on_mouse_click` runs the symbol's actions when clicked; images inside an SVG are drawn only when embedded (`data:` URLs); a strip of frames `sub_image_width` wide (DLS EDM symbols) shows frame `image_index` |

Also: macros (`$(X)`, `${X}`, defaults, widget properties such as
`$(pv_name)`, and `$(actions)` in a tooltip), rules (expressions over `pv0`, `pvStr0`, `pvSev0`,
`pvLegacySev0` … with arithmetic, bitwise and logical operators, that set any
property, or one item of a list such as `symbols[0]`; output expression rules),
`border_width`/`border_color` borders, label and text update `rotation_step`,
alarm borders and dashed borders for disconnected PVs, back and
forward navigation, `loc://` and `sim://` PVs. The display name, with macros
expanded (or the file name if it has none), is the window title.

Unsupported widgets, such as table, webbrowser, array, fileselector and
byte-array, are drawn as a grey placeholder labelled with the widget type
and PV name.

Not yet: compressed (codec) images, image axes, ROIs and cursor PVs, the XY plot
`markers`, error bars and toolbar, the strip chart's toolbar and second y axes, GIF and JPEG
images (symbols and pictures show their fallback or a placeholder), text inside
SVG images, and scripts: an `execute` action logs that it is not supported,
except the "toggle the widget's PV" script that techui-builder puts in
synoptic symbols, which bobview recognises and runs itself.

## BOY `.opi` displays

An `.opi` file (CS-Studio BOY) is translated as it is read into the `.bob`
form, the way Phoebus's own legacy import converts it, so it is drawn by the
same widgets. It is accepted everywhere a `.bob` is: on the command line, in
Open… (which lists `.opi` files too), in actions and linking containers, and
over http(s). As in Phoebus, a `.bob` next to a named `.opi` file is opened
in its place.

| BOY widget | drawn as |
| --- | --- |
| Rectangle, RoundedRectangle, Ellipse, arc, polyline, polygon | the shapes (BOY borders become outlines, points made relative, `alpha`) |
| Label, TextUpdate, TextInput | label, textupdate, textentry (`format_type`, `precision_from_pv`, borders inside the bounds, `rotation_angle`); a text update or input with text and no PV is a label |
| groupingContainer, linkingContainer | group (style from `border_style`), embedded (`opi_file`, `resize_behaviour`; a title bar or group box border becomes a group around it) |
| ActionButton, MenuButton, BoolButton, choiceButton | action_button (the `push_action_index` action first), combo or action button with a menu, bool_button (an EDM message button with no PV writes its push action), choice |
| LED, bytemonitor, progressbar, meter, scaledslider | led or multi_state_led, byte_monitor, progressbar with its value label, meter, scaledslider |
| edm.symbolwidget, MultistateMonitorWidget, ImageBoolIndicator, Image | symbol (an image strip, or a numbered set of images, nothing for a state without one), picture |
| detailpanel (DLS) | a group with each row's name and content |
| xyGraph, tab (TabbedContainer) | xyplot (numbered axes and traces, rules on a trace's properties), tabs (one grouping container per tab) |
| edm.menumux | combo on its first target PV (writes the item's label) |
| others | placeholders |

Colours keep the RGB the file gives; a colour with only a name is looked up
in the EDM `colors.list` DLS converted its screens with, then in the Phoebus
palette. Fonts in pixels keep their size, fonts in points are divided by 1.01,
and the Phoebus named fonts (`Default`, `Header 1` …) use the Phoebus sizes.
Rules keep BOY's severity numbers (`pvSev0` becomes `pvLegacySev0`) and
`widget.getValue()` reads the widget's own PV. A click on a shape, symbol or
LED with a BOY click hook runs its actions. Scripts and JavaScript or Python
actions are not run, except that the EDM "Exit" button's script
(`ScriptUtil.closeAssociatedOPI`) goes back to the previous display, or
closes the window, and a script that opens a Data Browser on some PVs
(`ScriptUtil.executeEclipseCommand` of
`org.csstudio.trends.databrowser2.newWindow` with `['pvnames', 'A,B']`, as
the DLS vacuum "Graph" buttons do) opens a [trend](#trends) of them. The
DLS vacuum widget script `get_pv_name.py`, recognised by its file name, is
done natively: its second PV, a vacuum space's link field such as
`SR03S-VA-SPACE-01:IONPSTA.INPA`, names a group (`SR03S-VA-GIONP-01:STA NPP
NMS` names `SR03S-VA-GIONP-01`), and each time it names a different one a
label shows the group's name, and any other widget reads the group's `:STA`
and takes its name, so its tooltip, right-click menu and the `$(name)` its
click passes as `device` follow; an EDM symbol of a gauge, ion pump or valve
strip shows the frame for that status (0 when disconnected). Other scripts
are ignored. A disconnected EDM symbol shows its first frame inside
the disconnected border, without Phoebus' overlay.

An `.opi` keeps its declared width and height, and widgets outside them are
clipped, as in BOY: EDM conversions often leave stray widgets far off the
display, which Phoebus (and bobview, for a `.bob`) would grow the display to
show. This size is also what Fit, `--size auto` and an embedding container
that sizes itself to its content use.

BOY screens name each other through the CS-Studio workspace: an Eclipse
project whose `.project` links folder names (`mks937a`, `dlsPLC` …) to the
directories that hold each module's screens, so `../mks937a/gauge.opi` means
the `mks937a` link. Open a screen through its project path, for example
`bobview /workspaces/VacOPIS/vacuum_4-100/vacuum/diamondVacuumOverview.opi`
for a project in `/workspaces/VacOPIS/vacuum_4-100`, and bobview follows the
links (the linked folders need not exist on disk; Open… lists them). A
screen's real path can be given with `--project DIR` instead:
`bobview -p /workspaces/VacOPIS/vacuum_4-100 /dls_sw/…/vacuumApp/opi/opi/diamondVacuumOverview.opi`.

DLS modules need neither: there is no `.project` on disk, but the generated
launcher `runcss.sh` beside a module's screens (`vacuumApp/opi/opi`) lists
the links it gives CS-Studio (`links="${SCRIPT_DIR}=${project}/${module},…"`).
bobview reads that list (it does not run the script, which needs
`configure-ioc`), taking `version` from the module's `configure/VERSION`
(else `dev`), so a BOY `.opi` screen such as
`bobview /dls_sw/…/vacuum/4-100/vacuumApp/opi/opi/diamondVacuumOverview.opi`
opens as
`…/vacuumApp/opi/opi/runcss.sh/vacuum_4-100/vacuum/diamondVacuumOverview.opi`,
the CS-Studio path `/vacuum_4-100/vacuum/diamondVacuumOverview.opi` below the
launcher, and links to other modules work. The same goes for a BOY `.opi`
screen reached by its real path from anywhere else (the file chooser, a
layout, an absolute path in another screen), with the launcher in its
folder or up to two above. This is for BOY screens only: a `.bob` (even one
Phoebus-style in place of an `.opi`, or one picked in a launcher project's
folder) goes by its real path. A `.project` nearer the screen than the
launcher, or beside it, wins when it links the screen (an empty one, as many
DLS modules have, does not). `-p` also
takes a module's screens folder (or its `runcss.sh`), to open another
module's BOY screen through that module's links.

DLS screens open others with command actions that run a launcher script,
such as `./srCellVacuum 04`, which ends by running `runcss.sh DISPLAY -m
MACROS` to start CS-Studio. For a local display, bobview runs a copy of such a
script beside a stand-in `runcss.sh` that only records its arguments, and
opens that display with those macros. A command that starts StripTool
(`strip.py`, `strip.sh` or `buildStripToolconfig.sh` and the PV names, as
in the DLS vacuum, gauge and RGA screens) opens a [trend](#trends) of those
PVs instead; a last word without a `:`, such as `log_mbar`, is ignored.
Other commands are not run.

No command is ever run for a display fetched over http(s), nor in a
`command_directory` on the web. Only a launcher script that resolves (after
macros, `..` and project links) to a local file beside or under the display,
or in a linked folder of its Eclipse project, is run; anything else, such as
an absolute path elsewhere or a `../..` escape, is refused and logged, as is
a script that any user may change (world-writable, or in a world-writable
folder). Each script run is logged.

A screen that needs such macros can be opened directly: for an `.opi` given
on the command line in an Eclipse project (through its project path or with
`--project`, so not for any `.opi` file lying around), bobview tries the
launcher scripts in its directory
(the one named `<screen>.sh` first) with no arguments and with each number
in the file name, bare and with an `r`, and uses the macros of the first
that opens this screen. So `linacVacuum.opi` gets `dom=LI` from
`linacVacuum.sh`, and `srCell05Vacuum.opi` and `SR05C_rack.opi` get theirs
from `srCellVacuum 05` and `srCellVacuum 05r`. Macros given with `-m`
override these. Screens with no number in their name (`srInjCellVacuum.opi`
is cell 01) need `-m`, or opening from the overview.

## Trends

A trend window plots one or more PVs against time. Open one from a widget's
right-click menu (Trend: its PV; Trend with rule PVs: the PVs its rules
read too), or with a BOY button that would open a Data Browser or StripTool
(see [BOY `.opi` displays](#boy-opi-displays)). Several may be open at once,
each in its own window over the display, and they stay open as you move
between displays.

A trend is live only: each PV gets a sample on every update from when the
window opened (up to 100 000 samples, and a day, per PV), with no archived
history. The window shows the last 10 minutes by default (1 minute to 1 day
from its menu), with a value axis that fits the data and a Log scale
switch; each value holds until the next, and a gap marks where a PV was
disconnected. Below the plot a legend gives each PV's colour, name and
current value, or Disconnected; ✖ removes a PV, and Add PV adds one.

## Pinned windows and layouts

For a control room, several displays can be open side by side, each in a
window of its own, and the arrangement saved and restored. Each window is
**pinned** or not, which decides where its links (an action or menu button,
a symbol click …) open their displays, whatever their target:

| Click | Display already open somewhere | Not open |
|---|---|---|
| in a pinned window | raise that window | open in the viewer, or a new window if every window is pinned |
| in an unpinned window | raise that window | open in place |
| **shift**+click | raise that window | open in a new window |
| **ctrl**+click | open in place | open in place |

The **viewer** is the unpinned window used last (focused, opened or
unpinned), so clicking a window once makes it where the next link goes.
"Already open" means the same display with the same macros. A display
opened in place or in the viewer keeps that window's history for Back and
Forward.

The main window starts pinned, so it stays where it is while the displays
it links to come and go in one other window; pin that one too to keep it,
and the next link opens a new viewer. Unpinned, the main window navigates in
place as one window does. Holding shift opens every display in a window of
its own, as the old multi-window mode did. The main window's pin is 📌 on
its toolbar; a display window's is in the corner of its display (below).
Pinned windows have 📌 before their title. In a browser, and on a touch or
small screen, displays have no windows of their own: every link navigates
in place, and the pin and layout controls are not shown. BOY Exit buttons
close the display window they are in (in the main window they go back
first, as before).

A display window opens sized to its display at 100%, within the monitor,
beside the window pinned last, so a row of small windows grows beside a
big one: on the first of its right, below it, its left and above it where
there is room on the monitor clear of the pinned windows. Failing that it
goes beside the window whose link opened it, then beside the other pinned
windows, or where it covers them least (not on Wayland, where the
compositor places windows).
The viewer is sized and
placed so again for each display a pinned window's link opens in it. The
display fits the window as you resize it. It has the native title
bar only: its toolbar (Back, Forward, Reload, zoom and Home) is hidden until
you show it. While the pointer is over the window, small buttons appear in
the top-right corner of the display: ◀ Back (while the toolbar is hidden),
📌 the window's pin, and a chevron that shows or hides the toolbar, as
**Ctrl+T** and Show toolbar in the right-click menu on the display's
background (which also has Close) do. In
every window Ctrl+R reloads, Alt+← and Alt+→ go back and forward, and
Ctrl+W closes the window. Closing the main window quits bobview and closes
its windows.

A layout records the open windows: each one's display, Eclipse project and
effective macros, its position and size, whether it is pinned, the main
window's zoom and whether a display window shows its toolbar (a layout from
before windows were pinned pins them all if it was saved in multi-window
mode, and none if not). It records the trend windows too: each
one's PVs, time span, log scale, position and size. Layouts are JSON files in
`$XDG_CONFIG_HOME/bobview/layouts` (by default `~/.config/bobview/layouts`).
The main toolbar's layout list loads one (closing the display and trend
windows open now), and the ▾ menu next to it has Save (to the layout last loaded or
saved), Save as default, Save as… (asking before replacing a layout),
Delete, and Save default on exit (off unless ticked).

```bash
bobview                     # restores the default layout if there is one
bobview DISPLAY             # opens DISPLAY; the default layout is not restored
bobview --layout rf-shift   # restores layouts/rf-shift.json
bobview --no-layout         # ignores the default layout
```

`--snapshot` ignores layouts. A saved position that is on none of the
monitors now (a laptop that was docked) is dropped and the window manager
places the window.

**Wayland** does not let an application read or set its windows' positions,
so on a Wayland desktop a layout restores the windows and their sizes but the
compositor places them, and bobview logs this once. Running bobview under
XWayland, with `WAYLAND_DISPLAY= bobview`, keeps positions. X11 desktops,
including `bobview-ssh` over X forwarding, restore positions.

## In a browser

The same app also builds for the browser (WebAssembly, drawn with WebGL),
with PVs coming over a [PV Web Socket](https://github.com/ornl-epics/pvws)
(PVWS) as in Daedalus. `bobview-server` serves the lot from one port: the web
build, `.bob` files and a PVWS at `/pvws/pv` that reaches PVs through
bobview's own CA/PVA client. Run it anywhere `caget` works:

```bash
bobview-server --opi /path/to/displays
```

and open `http://localhost:8080/?display=index.bob` (on this machine; from another, start the server with `--bind` and use its name). The release tarball includes
`bobview-server` and the web build, which it finds in `share/bobview/web`; in
a checkout, `./scripts/build-web.sh` writes `dist/web` (it needs the
`wasm-bindgen` CLI matching `Cargo.lock`) and `--web DIR` picks another. Files
in `--opi` take precedence over same-named files in the web build (which has
the test displays); dot-files such as `.git` are never served. The server
listens on `--bind 127.0.0.1 --port 8080` by default, so only this machine can
reach it; use `--bind 0.0.0.0` (every interface) or a specific address to serve
others, ideally behind a firewall or an authenticating reverse proxy, since
reading PVs needs no login. The server holds at most 100 clients and 50,000
subscribed PVs, and refuses PV names over 256 characters or with spaces or
non-ASCII characters (so `loc://s("hello world")` is refused). One address
may hold 10 clients (`--max-per-ip N`), and a client that stops answering
websocket pings is dropped. Behind a reverse proxy every client has the
proxy's address, so raise `--max-per-ip` and expect the logs to show the proxy;
the `Host` the proxy passes on must also be one of the server's names, so give
a public name with `--host NAME` (the Origin/Host check now applies to read-only
servers too).

The page also takes `&macros=P=X,R=Y`, `&zoom=F`, `&keypad` and, to use
another PVWS server, `&pvws=host[:port]` (meaning `wss://host[:port]/pvws/pv`,
or `ws://` when the page is plain `http:` and the port isn't 443) or a full
`ws://` or `wss://` URL; `display` may be a URL on another server that allows
cross-origin requests. Writes are off unless the server has `--write`, since
they go out as the server's user, and then each write must carry a token:
`--write-token TOKEN` (arguments show in `ps`, so prefer the
`BOBVIEW_WRITE_TOKEN` environment variable), or a random one that the server
prints to stderr at startup; open the page with `&token=TOKEN`. The page removes
the token from its address after reading it, so a reload needs it again. Browsers let any page open a websocket, so the
server always accepts only pages from itself (by `Origin`), plus
any named with `--allow-origin https://host.example.com` (repeatable). Those
pages must also call the server by an IP address, `localhost`, this machine's
name, the host of an `--allow-origin` or a name given with `--host NAME`
(repeatable), so a page cannot reach it through a DNS name of its own.
Images from PVA are not sent over PVWS yet.

## Build

```bash
cargo build --release
cargo test --workspace
# for the phone from x86_64, with zig and cargo-zigbuild installed:
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36
```

CI builds the x86_64 and aarch64 binaries on GitHub's `ubuntu-24.04` and
`ubuntu-24.04-arm` runners with `cargo zigbuild` and attaches `bobview-x86_64-linux.tar.gz` and
`bobview-aarch64-linux.tar.gz` to a GitHub Release for every `v*` tag. They
are linked against glibc 2.28, so they run on RHEL 8+, Ubuntu 20.04+ and
Debian 10+.

To release, push a tag such as `v0.2.2`. CI creates the GitHub Release itself
and builds with the version taken from the tag, so `Cargo.toml` needs no bump.

## Licences

bobview is Apache-2.0. The release binaries include third-party crates, among
them the [epics-rs](https://github.com/epics-rs/epics-rs) CA and PVA libraries
(EPICS-style open licence), and embed the Liberation Sans fonts and a symbol
subset of Noto Sans Math (both SIL Open Font Licence) and egui's default
fonts. Their notices are in
`THIRD_PARTY_LICENSES.txt` in each release tarball, generated by
`scripts/third-party-licenses.py`. `tests/displays/demo.bob` and the test
IOC's `ea.db` come from epics-containers example-services (Apache-2.0).
