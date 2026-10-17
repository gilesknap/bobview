#!/bin/bash
# Build bobview for the browser into a static site (default: dist/web).
#
# Serve the directory from any web server. Open index.html with
#   ?display=<URL or path of a .bob, relative to the page; default index.bob>
#   &macros=P=X,R=Y  &pvws=<host[:port] or ws(s):// URL of a PV Web Socket>
# The display's server must allow the page's origin (CORS) unless it is the
# same server; the PVWS default is /pvws/pv on the page's own server.
# Needs the wasm32-unknown-unknown target and a wasm-bindgen CLI matching
# the wasm-bindgen version in Cargo.lock.
set -euo pipefail

out=${1:-$(dirname "$0")/../dist/web}
mkdir -p "${out}"
out=$(cd "${out}" && pwd)
cd "$(dirname "$0")/.."
if [[ "$(pwd)/" == "${out}/"* ]]; then
    echo "will not replace ${out}, which holds the source" >&2
    exit 1
fi
# It is replaced, so it must be empty or an earlier build.
if [[ -n "$(ls -A "${out}")" && ! -e "${out}/bobview.js" && ! -e "${out}/index.html" ]]; then
    echo "will not replace ${out}, which is not empty and not a web build" >&2
    exit 1
fi

want=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [[ "${have}" != "${want}" ]]; then
    echo "need wasm-bindgen ${want}, found ${have:-none}:" >&2
    echo "  cargo install wasm-bindgen-cli --version ${want} --locked" >&2
    exit 1
fi

cargo build --locked --release --target wasm32-unknown-unknown -p bobview --bin bobview
target=$(cargo metadata --format-version 1 --no-deps --locked |
    "${PYTHON:-python3}" -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
rm -rf "${out}"
mkdir -p "${out}"
wasm-bindgen --target web --no-typescript --out-dir "${out}" \
    "${target}/wasm32-unknown-unknown/release/bobview.wasm"
cp web/index.html "${out}/"
# The test displays, so the bundle shows something with no arguments.
cp -r tests/displays/. "${out}/"
echo "wrote ${out}"
