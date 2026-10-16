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

The crates are not published to crates.io and their API is not stable; it
is documented for contributors.

In a local build the links work when the book is built with
`scripts/build-docs.sh` (see [Build from source](../how-to/build.md#build-these-docs)),
not with a plain `mdbook serve`.
