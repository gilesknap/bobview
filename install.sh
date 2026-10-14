#!/bin/bash
# Install bobview for aarch64 Linux (e.g. the Android Linux Terminal) into ~/.local
#
#   curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
#
# Run from inside an unpacked release tarball, it installs that instead of downloading.
set -euo pipefail

repo=gilesknap/bobview
name=bobview-aarch64-linux
prefix=${PREFIX:-$HOME/.local}
here=$(cd "$(dirname "${BASH_SOURCE[0]:-.}")" && pwd)

if [[ -x "${here}/bin/bobview" ]]; then
    src=${here}
else
    tmp=$(mktemp -d)
    trap 'rm -rf "${tmp}"' EXIT
    if command -v gh >/dev/null; then
        gh release download --repo "${repo}" -p "${name}.tar.gz" -D "${tmp}"
    else
        curl -sSfL -o "${tmp}/${name}.tar.gz" \
            "https://github.com/${repo}/releases/latest/download/${name}.tar.gz"
    fi
    tar --no-same-owner -C "${tmp}" -xzf "${tmp}/${name}.tar.gz"
    src=${tmp}/${name}
fi

mkdir -p "${prefix}/bin" "${prefix}/share/bobview"
cp "${src}/bin/"* "${prefix}/bin/"
rm -rf "${prefix}/share/bobview/displays"
cp -r "${src}/share/bobview/displays" "${prefix}/share/bobview/"
echo "installed bobview, bobview-testioc and bobview-probe into ${prefix}/bin"
echo "displays are in ${prefix}/share/bobview/displays"
