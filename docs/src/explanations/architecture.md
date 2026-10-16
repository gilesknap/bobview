# Architecture

bobview is a Cargo workspace of five crates:

| crate | what |
| --- | --- |
| `bob` | the display model: reads `.bob` files and translates `.opi` ones into the same form, expands macros, resolves Eclipse project links and DLS launchers |
| `pv` | PV connections behind one interface: Channel Access, PV Access, PVWS for the browser, `loc://`, `sim://` and formula PVs |
| `bobview` | the viewer: an [egui](https://github.com/emilk/egui) app that draws displays, plus `bobview-probe` |
| `bobview-server` | serves the web build, `.bob` files and a PV Web Socket from one port |
| `testioc` | `bobview-testioc`, a pure-Rust IOC serving the example-services BL01T PVs |

## From file to screen

1. `bob` reads a display, from a file or over http(s), into a tree of
   widgets with their properties. An `.opi` is translated as it is read, so
   everything after this step sees only the `.bob` form.
2. Macros are expanded down the tree; rules are kept as expressions over the
   PVs they read.
3. `bobview` connects each widget's PVs through `pv` and draws the tree with
   egui every frame, re-evaluating rules as values arrive.

## Protocols

`pv`'s `ca.rs` and `pva.rs` are thin adapters over the
[epics-rs](https://github.com/epics-rs/epics-rs) `epics-ca-rs` and
`epics-pva-rs` crates, which do the protocol work. These are young, so
protocol correctness rests largely on them; the `interop` CI job tests
against other EPICS implementations.

## In the browser

The `pv` and `bobview` crates also build for `wasm32-unknown-unknown`, drawn
with WebGL. A browser can't speak CA or PVA, so there PVs come over a
[PV Web Socket](https://github.com/ornl-epics/pvws) (PVWS), as in Daedalus.
`bobview-server` provides one at `/pvws/pv`, reaching PVs through the same
`pv` crate as the desktop viewer; any other PVWS server works too.
`loc://` and `sim://` PVs are made in the page itself.

## The test IOC

`bobview-testioc` serves enough of the
[epics-containers example-services](https://github.com/epics-containers/example-services)
BL01T beamline for its screens, over CA and PVA, with no C dependencies. It
runs natively on an aarch64 phone, where the real amd64 IOC containers would
need qemu, and CI uses it to snapshot every test display. See
[bobview-testioc](../reference/testioc.md).
