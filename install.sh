#!/bin/bash
# Install bobview for aarch64 or x86_64 Linux (e.g. the Android Linux Terminal)
# or macOS into ~/.local
#
#   curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
#
# Run from inside an unpacked release tarball, it installs that instead of downloading.
set -euo pipefail

repo=gilesknap/bobview
case $(uname -m) in
    aarch64 | arm64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) echo "bobview: no release build for $(uname -m)" >&2; exit 1 ;;
esac
case $(uname -s) in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) echo "bobview: no release build for $(uname -s)" >&2; exit 1 ;;
esac
name=bobview-${arch}-${os}
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
rm -rf "${prefix}/share/bobview/web"
if [[ -d "${src}/share/bobview/web" ]]; then
    cp -r "${src}/share/bobview/web" "${prefix}/share/bobview/"
fi
cp "${src}/README.md" "${src}/LICENSE" "${prefix}/share/bobview/"
if [[ -f "${src}/THIRD_PARTY_LICENSES.txt" ]]; then
    cp "${src}/THIRD_PARTY_LICENSES.txt" "${prefix}/share/bobview/"
fi
echo "installed bobview, bobview-ssh, bobview-testioc, bobview-probe and bobview-server into ${prefix}/bin"
echo "displays are in ${prefix}/share/bobview/displays"
