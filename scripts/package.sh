#!/bin/bash
# Bundle the release binaries and supporting files into <name>.tar.gz
set -euo pipefail

name=${1:?usage: package.sh <name>}
out=$(mktemp -d)/${name}
mkdir -p "${out}/bin"

cp target/release/bobview target/release/bobview-probe target/release/bobview-testioc "${out}/bin/"
cp README.md LICENSE "${out}/"

tar -C "$(dirname "${out}")" -czf "${name}.tar.gz" "${name}"
echo "wrote ${name}.tar.gz"
