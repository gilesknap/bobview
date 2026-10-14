#!/bin/bash
# Download the latest bobview release for aarch64 and install it into ~/.local
#   curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
set -euo pipefail

repo=gilesknap/bobview
name=bobview-aarch64-linux
prefix=${PREFIX:-$HOME/.local}
tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT

if command -v gh >/dev/null; then
    gh release download --repo "${repo}" -p "${name}.tar.gz" -D "${tmp}"
else
    curl -sSfL -o "${tmp}/${name}.tar.gz" \
        "https://github.com/${repo}/releases/latest/download/${name}.tar.gz"
fi

tar -C "${tmp}" -xzf "${tmp}/${name}.tar.gz"
mkdir -p "${prefix}/bin" "${prefix}/share/bobview"
cp "${tmp}/${name}/bin/"* "${prefix}/bin/"
cp -r "${tmp}/${name}/." "${prefix}/share/bobview/"
rm -rf "${prefix}/share/bobview/bin"
echo "installed bobview into ${prefix}/bin (support files in ${prefix}/share/bobview)"
