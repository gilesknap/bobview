# Rust API

The workspace's crates are documented with rustdoc, built into this book
under `api/`.

Libraries:

- [`bob`](../api/bob/index.html): the display model, `.bob` and `.opi`
  reading, macros and project links
- [`pv`](../api/pv/index.html): PV connections over CA, PVA, PVWS and local
  sources
- [`testioc`](../api/testioc/index.html): the test IOC

Programs, documented with their private items, since that is all they have:

- [`bobview`](../api/bobview/index.html): the viewer, its egui app and
  widgets
- [`bobview_server`](../api/bobview_server/index.html): the web server and
  its PV Web Socket
- [`bobview_probe`](../api/bobview_probe/index.html) and
  [`bobview_testioc`](../api/bobview_testioc/index.html): the command-line
  front ends of `bobview-probe` and `bobview-testioc`

The crates are on crates.io (see [Install from crates.io](../tutorials/crates-io.md)),
but their API is not stable; it is documented for contributors.

In a local build the links work when the book is built with
`scripts/build-docs.sh` (see [Build from source](../how-to/build.md#build-these-docs)),
not with a plain `mdbook serve`.

## Source map

The main source files, for finding your way in. See
[Architecture](../explanations/architecture.md) for how they fit together.

| file | what |
| --- | --- |
| **`crates/bob/src/`** | |
| `lib.rs` | `Display` and `Widget`: a display kept close to its XML, parsed from a file or text |
| `opi.rs` | translates a BOY `.opi`, element by element, into the `.bob` form |
| `macros.rs` | `$(X)` and `${X}` expansion |
| `source.rs` | where a display comes from (file or URL), and finding the files it names |
| `project.rs` | Eclipse project linked folders and DLS launcher paths |
| `group_link.rs`, `menu_mux.rs`, `opi_file_script.rs`, `trend.rs` | BOY scripts and launchers recognised and done natively |
| **`crates/pv/src/`** | |
| `lib.rs` | `PvManager`, `Pv` and `PvState`: subscriptions, writes and the per-window repaint tags |
| `ca.rs`, `pva.rs` | Channel Access and PV Access backends over epics-rs |
| `pvws.rs`, `pvws_msg.rs` | the browser backend over a PV Web Socket |
| `local.rs`, `sim.rs`, `formula.rs` | `loc://`, `sim://` and formula PVs |
| `name.rs`, `value.rs` | PV names and their prefixes; values and their conversions |
| **`crates/bobview/src/`** | |
| `main.rs` | the command line, and starting the native or browser app |
| `app.rs` | `BobApp`: the windows, layouts, requests and repaints (see [Threads, windows and repaints](../explanations/repaints.md)) |
| `view.rs` | one window's display, its loading and its Back and Forward |
| `fetch.rs` | reading displays from files or over http(s), and caching them |
| `screen.rs` | the screen builder: widgets with positions, macros, PVs and rules |
| `rules.rs` | the rule expression language |
| `widgets.rs` | painting and interaction for each widget type, and `Request` |
| `image.rs`, `symbol.rs`, `svg.rs` | the image widget; symbol and picture widgets, loaded on worker threads; safe SVG parsing |
| `plot.rs`, `meter.rs`, `trend.rs`, `axis.rs` | XY plots, meters, trends and their shared scales |
| `format.rs` | values as text and back, as Phoebus formats them |
| `tree.rs` | the navigator |
| `pvmenu.rs` | the PV context menu and details window |
| `layout.rs` | saved window layouts |
| `launch.rs` | running DLS launcher scripts for `.opi` command actions |
| `snapshot.rs` | headless rendering to PNG |
| **`crates/bobview-server/src/`** | |
| `main.rs`, `files.rs`, `pvws.rs` | the server, its file serving, and its PVWS onto `pv` |
