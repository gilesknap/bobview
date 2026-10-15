#!/bin/bash
# Build bobview for the browser into a static site (default: dist/web).
#
# Serve the directory from any web server. Open index.html with
#   ?display=<URL or path of a .bob, relative to the page; default index.bob>
#   &macros=P=X,R=Y  &pvws=<host[:port] or wss:// URL of a PV Web Socket>
# The display's server must allow the page's origin (CORS) unless it is the
# same server; the PVWS default is /pvws/pv on the page's own server.
# Needs the wasm32-unknown-unknown target and a wasm-bindgen CLI matching
# the wasm-bindgen version in Cargo.lock.
set -euo pipefail

out=${1:-dist/web}
cargo build --release --target wasm32-unknown-unknown -p bobview --bin bobview
mkdir -p "${out}"
wasm-bindgen --target web --no-typescript --out-dir "${out}" \
    target/wasm32-unknown-unknown/release/bobview.wasm
cp web/index.html "${out}/"
# The test displays, so the bundle shows something with no arguments.
cp -r tests/displays/. "${out}/"
echo "wrote ${out}"
