# bobview

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens, written in Rust with [egui](https://github.com/emilk/egui).
It also opens the older CS-Studio BOY `.opi` screens (see [BOY `.opi` displays](#boy-opi-displays)).
It shows live EPICS Channel Access and PV Access values, including areaDetector
images, and is small enough to run on a phone, in the Android 16 Linux Terminal (Debian 13, aarch64).

Scripts (Jython/JavaScript), display editing, alarm and archiver services are
out of scope.

## Install

On the phone (Android Linux Terminal) or an x86_64 Linux desktop; `install.sh`
picks the aarch64 or x86_64 release build to match the machine:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

This installs into `~/.local`:

| file | what |
| --- | --- |
| `bin/bobview` | the viewer |
| `bin/bobview-ssh` | runs the viewer over `ssh -X`, see below |
| `bin/bobview-testioc` | a pure-Rust IOC serving the example-services BL01T PVs |
| `bin/bobview-probe` | prints live PV values from the command line |
| `share/bobview/displays/` | test screens; `index.bob` opens by default |

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
first), `BOBVIEW_LOG=debug` gives more output. The window opens at the size
of the first display (when the screen has room). With Auto-size (🗖 in the
toolbar, on by default) it is resized to each display you open, unless it
is maximised or full screen; turn it off to keep the window's size and scale
each display to fit. The toolbar, menus and keypad are scaled down on a
small high-DPI screen so that they take a small share of it; `--ui-scale F`
(or `BOBVIEW_UI_SCALE=F`) fixes their size, 1 being the desktop's own scale. Hover over a widget to see its
PV and value; right-click it (long-press on a touch screen) to copy the PV
name or value, or open the PV details. On a touch screen, pinch to zoom.

## What it draws

| widgets | notes |
| --- | --- |
| label, textupdate, textentry | formats: default, exponential, hex, binary, string; units, precision |
| led, multi_state_led, byte_monitor | |
| action_button, bool_button, choice, radio, combo, checkbox | open display and write PV actions |
| scaledslider, scrollbar, spinner | |
| linearmeter | scale, warning regions from the levels (the PV's alarm limits with `limits_from_pv`), limit markers, needle; horizontal or vertical |
| meter | a dial: scale (linear or `log_scale`), warning regions from the levels (the PV's alarm limits with `limits_from_pv`), needle |
| progressbar, tank, thermometer | drawn as bars for now |
| xyplot | line, step and bar traces with markers against index or an x PV; several y axes, autoscale, log scale, grid, legend |
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

Not yet: compressed (codec) images, image axes, ROIs and cursor PVs, the XY plot
`markers`, error bars and toolbar, strip chart widgets (drawn as placeholders), GIF and JPEG
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
closes the window. A disconnected EDM symbol shows its first frame inside
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

DLS screens open others with command actions that run a launcher script,
such as `./srCellVacuum 04`, which ends by running `runcss.sh DISPLAY -m
MACROS` to start CS-Studio. For a local display, bobview runs a copy of such a
script beside a stand-in `runcss.sh` that only records its arguments, and
opens that display with those macros. Other commands are not run.

No command is ever run for a display fetched over http(s), nor in a
`command_directory` on the web. Only a launcher script that resolves (after
macros, `..` and project links) to a local file beside or under the display,
or in a linked folder of its Eclipse project, is run; anything else, such as
an absolute path elsewhere or a `../..` escape, is refused and logged.

A screen that needs such macros can be opened directly: for an `.opi` given
on the command line, bobview tries the launcher scripts in its directory
(the one named `<screen>.sh` first) with no arguments and with each number
in the file name, bare and with an `r`, and uses the macros of the first
that opens this screen. So `linacVacuum.opi` gets `dom=LI` from
`linacVacuum.sh`, and `srCell05Vacuum.opi` and `SR05C_rack.opi` get theirs
from `srCellVacuum 05` and `srCellVacuum 05r`. Macros given with `-m`
override these. Screens with no number in their name (`srInjCellVacuum.opi`
is cell 01) need `-m`, or opening from the overview.

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
