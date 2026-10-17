#!/bin/bash
# Bundle the release binaries and supporting files into <name>.tar.gz, or on
# Windows (Git Bash) <name>.zip
#
# Layout:  <name>/bin/{bobview,bobview-ssh,bobview-probe,bobview-testioc,bobview-server}
#          <name>/share/bobview/displays/   (the test display set)
#          <name>/share/bobview/web/        (the browser build, from build-web.sh)
#          <name>/README.md, LICENSE, THIRD_PARTY_LICENSES.txt, install.sh
# On Windows the binaries are .exe, and bobview-ssh and install.sh are left out.
set -euo pipefail

name=${1:?usage: package.sh <name>}
target=${TARGET_DIR:-target/release}
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) windows=1 exe=.exe ;;
    *) windows= exe= ;;
esac
out=$(mktemp -d)/${name}
mkdir -p "${out}/bin" "${out}/share/bobview"

for b in bobview bobview-probe bobview-testioc bobview-server; do
    cp "${target}/${b}${exe}" "${out}/bin/"
done
cp -r tests/displays "${out}/share/bobview/displays"
cp -r "${WEB_DIR:-dist/web}" "${out}/share/bobview/web"
cp README.md LICENSE "${out}/"
if [[ -z "${windows}" ]]; then
    cp scripts/bobview-ssh "${out}/bin/"
    cp install.sh "${out}/"
fi
"${PYTHON:-python3}" "$(dirname "$0")/third-party-licenses.py" \
    "${LICENSE_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}" > "${out}/THIRD_PARTY_LICENSES.txt"

if [[ -n "${windows}" ]]; then
    rm -f "${name}.zip"
    (cd "$(dirname "${out}")" && 7z a -tzip -bso0 "${OLDPWD}/${name}.zip" "${name}")
    echo "wrote ${name}.zip"
    exit
fi

# owned by root in the archive; GNU tar and bsdtar (macOS) spell this differently
if tar --version | grep -q GNU; then
    owner=(--owner=0 --group=0)
else
    owner=(--uid 0 --gid 0)
fi
tar "${owner[@]}" -C "$(dirname "${out}")" -czf "${name}.tar.gz" "${name}"
echo "wrote ${name}.tar.gz"
