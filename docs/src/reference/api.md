# Rust API

The workspace's crates are documented with rustdoc, built into this book
under `api/`:

- [`bob`](../api/bob/index.html): the display model, `.bob` and `.opi`
  reading, macros and project links
- [`pv`](../api/pv/index.html): PV connections over CA, PVA, PVWS and local
  sources
- [`testioc`](../api/testioc/index.html): the test IOC

The crates are not published to crates.io and their API is not stable; it
is documented for contributors.

In a local build the links work when the book is built with
`scripts/build-docs.sh` (see [Build from source](../how-to/build.md#build-these-docs)),
not with a plain `mdbook serve`.
