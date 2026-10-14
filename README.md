# bobview

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens, written in Rust with [egui](https://github.com/emilk/egui).
It shows live EPICS Channel Access values and is small enough to run on a
phone, in the Android 16 Linux Terminal (Debian 13, aarch64).

Scripts (Jython/JavaScript), display editing, alarm and archiver services are
out of scope.

## Install on the phone

In the Android Linux Terminal:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

This installs into `~/.local`:

| file | what |
| --- | --- |
| `bin/bobview` | the viewer |
| `bin/bobview-testioc` | a pure-Rust IOC serving the example-services BL01T PVs |
| `bin/bobview-probe` | prints live PV values from the command line |
| `share/bobview/displays/` | test screens; `index.bob` opens by default |

## Run

```bash
bobview-testioc &                    # simulated BL01T IOCs on this machine
EPICS_CA_ADDR_LIST=127.0.0.1 bobview # opens share/bobview/displays/index.bob
bobview path/to/screen.bob -m P=BL01T,R=:DET:
```

`bobview-testioc` serves the PVs of the
[epics-containers example-services](https://github.com/epics-containers/example-services)
beamline natively, so the amd64 IOC containers are not needed on the phone:
`bl01t-ea-test-01` (its `ioc.db` verbatim), two motor records on a simulated
axis, and a simulated area detector without image data.
See [crates/testioc](crates/testioc/README.md).

To reach real IOCs from the phone instead, point Channel Access at them:
`EPICS_CA_ADDR_LIST` for UDP search, or `EPICS_CA_NAME_SERVERS=host:port` for
a ca-gateway reached over TCP (this works through the phone VM's NAT, since
the phone opens the connection). The example-services `ca-gateway` listens on
port 9064 of its host.

Options: `-z ZOOM` fixes the zoom instead of fitting the window, `-f` starts
full screen, `--snapshot out.png` renders headless and exits (with `--size WxH`
and `--wait SECONDS`), `BOBVIEW_LOG=debug` gives more output. On a touch
screen, pinch to zoom; long-press a widget to see its PV and value.

## What it draws

| widgets | notes |
| --- | --- |
| label, textupdate, textentry | formats: default, exponential, hex, binary, string; units, precision |
| led, multi_state_led, byte_monitor | |
| action_button, bool_button, choice, radio, combo, checkbox | open display and write PV actions |
| scaledslider, scrollbar, spinner | |
| progressbar, tank, thermometer, linearmeter, meter | all drawn as bars for now |
| rectangle, ellipse, arc, polyline, polygon | |
| group, tabs, embedded, navtabs | macros pass down; embedded resize modes 0–2 |

Also: macros (`$(X)`, `${X}`, defaults, widget properties such as
`$(pv_name)`), rules (expressions over `pv0`, `pvStr0`, `pvSev0` … that set any
property), alarm borders and dashed borders for disconnected PVs, back and
forward navigation, `loc://` and `sim://` PVs.

Not yet: PV Access (`pva://` PVs show as unsupported), image, XY plot and
strip chart widgets (drawn as placeholders), pictures.

## Build

```bash
cargo build --release
cargo test --workspace
# for the phone from x86_64, with zig and cargo-zigbuild installed:
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36
```

CI builds the aarch64 binaries natively on GitHub's `ubuntu-24.04-arm`
runners and attaches `bobview-aarch64-linux.tar.gz` to a GitHub Release for
every `v*` tag.

## Licences

bobview is Apache-2.0. It embeds the Liberation Sans fonts (SIL Open Font
Licence, `crates/bobview/fonts/LICENSE`). `tests/displays/demo.bob` and the
test IOC's `ea.db` come from epics-containers example-services (Apache-2.0).
