#!/bin/bash
# Build the docs: the Rust API docs, then the mdBook in docs/.
#
# The API docs are copied to docs/src/api, which mdBook copies into the site
# as it does any other file, so the book's links to api/ work in a local
# build or `mdbook serve` as they do on the published site.
#   scripts/build-docs.sh              build into docs/book
#   scripts/build-docs.sh serve --open serve, rebuilding the book on edits
# Rerun it to pick up changes to the API docs.
#
# It needs mdBook and the mdbook-mermaid preprocessor, for the diagrams, at
# the versions CI uses (.github/workflows/docs.yml).
set -euo pipefail

cd "$(dirname "$0")/.."
for tool in mdbook mdbook-mermaid; do
    if ! command -v "$tool" >/dev/null; then
        echo "$0: $tool not found; install it with" >&2
        echo "    cargo install mdbook@0.4.40 mdbook-mermaid@0.15.0" >&2
        exit 1
    fi
done
cargo doc --workspace --no-deps
rm -rf docs/src/api
cp -r target/doc docs/src/api
mdbook "${1:-build}" docs "${@:2}"
