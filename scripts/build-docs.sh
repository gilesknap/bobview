#!/bin/bash
# Build the docs: the Rust API docs, then the mdBook in docs/.
#
# The API docs are copied to docs/src/api, which mdBook copies into the site
# as it does any other file, so the book's links to api/ work in a local
# build or `mdbook serve` as they do on the published site.
#   scripts/build-docs.sh              build into docs/book
#   scripts/build-docs.sh serve --open serve, rebuilding the book on edits
# Rerun it to pick up changes to the API docs.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo doc --workspace --no-deps
rm -rf docs/src/api
cp -r target/doc docs/src/api
mdbook "${1:-build}" docs "${@:2}"
