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
# Local mode only when this is a real file (an unpacked release tarball). Under
# `curl | bash` there is no script file, and $PWD must never be trusted: it
# could be a shared directory where someone planted bin/bobview.
here=
if [[ -n ${BASH_SOURCE[0]:-} && -f ${BASH_SOURCE[0]} ]]; then
    here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fi

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}

if [[ -n ${here} && -x "${here}/bin/bobview" ]]; then
    src=${here}
else
    tmp=$(mktemp -d)
    trap 'rm -rf "${tmp}"' EXIT
    if command -v gh >/dev/null; then
        gh release download --repo "${repo}" -p "${name}.tar.gz" -p SHA256SUMS -D "${tmp}"
    else
        curl -sSfL -o "${tmp}/${name}.tar.gz" \
            "https://github.com/${repo}/releases/latest/download/${name}.tar.gz"
        # a release without checksums is refused below, with a message
        curl -sSfL -o "${tmp}/SHA256SUMS" \
            "https://github.com/${repo}/releases/latest/download/SHA256SUMS" || true
    fi
    # the release publishes SHA256SUMS; refuse a tarball that does not match
    if [[ ! -f ${tmp}/SHA256SUMS ]]; then
        echo "bobview: this release has no SHA256SUMS, refusing to install an unverified download" >&2
        exit 1
    fi
    want=$(awk -v f="${name}.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "${tmp}/SHA256SUMS")
    got=$(sha256 "${tmp}/${name}.tar.gz" | awk '{ print $1 }')
    if [[ -z ${want} || ${want} != "${got}" ]]; then
        echo "bobview: checksum mismatch for ${name}.tar.gz, not installing" >&2
        exit 1
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
