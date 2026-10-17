# Build from source

You need a stable Rust toolchain ([rustup](https://rustup.rs)).

```bash
git clone https://github.com/gilesknap/bobview
cd bobview
cargo build --release
cargo test --workspace
```

The binaries are in `target/release`.

## Cross-build for the phone

From x86_64, with [zig](https://ziglang.org) and
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) installed:

```bash
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36
```

## Build the browser version

```bash
./scripts/build-web.sh
```

writes `dist/web`. It needs the `wasm32-unknown-unknown` target and the
`wasm-bindgen` CLI matching the version in `Cargo.lock`. In a checkout,
`bobview-server` finds `dist/web` itself; `--web DIR` picks another.

## Build these docs

The docs are an [mdBook](https://rust-lang.github.io/mdBook/):

```bash
cargo install mdbook
scripts/build-docs.sh serve --open
```

This builds the Rust API docs into the book under `api/`, then serves it;
`mdbook serve` rebuilds and reloads the page as you edit. Run it again to
pick up changes to the API docs. `scripts/build-docs.sh` alone builds the
book into `docs/book`.

## Checks CI runs

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p bobview-pv -p bobview --target wasm32-unknown-unknown -- -D warnings
cargo test --workspace
```

## How the docs are published

The `Docs` workflow builds the book and the API docs on every pull request.
On a push to `main` or a `v*` tag it publishes them to the `gh-pages` branch
in a folder named after the branch or tag, writes `switcher.json` (the list
of versions for the menu in the top bar) and leaves the other versions in
place. The site's root redirects to `main/`.
