#!/bin/bash
# Bundle the release binaries and supporting files into <name>.tar.gz
#
# Layout:  <name>/bin/{bobview,bobview-probe,bobview-testioc}
#          <name>/share/bobview/displays/   (the test display set)
#          <name>/README.md, LICENSE, install.sh
set -euo pipefail

name=${1:?usage: package.sh <name>}
target=${TARGET_DIR:-target/release}
out=$(mktemp -d)/${name}
mkdir -p "${out}/bin" "${out}/share/bobview"

cp "${target}/bobview" "${target}/bobview-probe" "${target}/bobview-testioc" "${out}/bin/"
cp -r tests/displays "${out}/share/bobview/displays"
cp README.md LICENSE install.sh "${out}/"

tar -C "$(dirname "${out}")" -czf "${name}.tar.gz" "${name}"
echo "wrote ${name}.tar.gz"
