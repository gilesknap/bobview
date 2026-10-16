# Rust API

The workspace's crates are documented with rustdoc, published alongside
this book:

- [`bob`](../api/bob/index.html): the display model, `.bob` and `.opi`
  reading, macros and project links
- [`pv`](../api/pv/index.html): PV connections over CA, PVA, PVWS and local
  sources
- [`testioc`](../api/testioc/index.html): the test IOC

The crates are not published to crates.io and their API is not stable; it
is documented for contributors.

The links above work on the published site, where CI puts the rustdoc
output under `api/`. They don't work in a local `mdbook serve`; build and
open the API docs locally instead:

```bash
cargo doc --workspace --no-deps --open
```
