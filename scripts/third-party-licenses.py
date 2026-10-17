#!/usr/bin/env python3
"""Write the licence notices for everything compiled into the release binaries.

Walks the normal (not dev or build) dependency graph of the workspace for each
target (the binaries' and wasm32-unknown-unknown for the browser build), and
for each third-party crate records its licence expression and the
licence/notice/copyright files it ships. Identical files are printed once with
the crates that use them. Fonts embedded by bobview are added at the end.

    scripts/third-party-licenses.py [TARGET] > THIRD_PARTY_LICENSES.txt
"""

import json
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

TARGET = sys.argv[1] if len(sys.argv) > 1 else "aarch64-unknown-linux-gnu"
WEB_TARGET = "wasm32-unknown-unknown"
ROOT = Path(__file__).resolve().parent.parent
NOTICE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE", "THIRD_PARTY")
EMBEDDED_FONTS = [
    ("Liberation Sans 2.1.5 (embedded in bobview)", ROOT / "crates/bobview/fonts/LICENSE"),
    ("Noto Sans Math, symbol subset (embedded in bobview)", ROOT / "crates/bobview/fonts/LICENSE-NotoSansMath"),
]

# Everything reachable from the workspace through normal dependencies.
seen = set()
packages = {}
for target in (TARGET, WEB_TARGET):
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--filter-platform", target, "--locked"],
            cwd=ROOT,
        )
    )
    packages.update((p["id"], p) for p in meta["packages"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    reached = set()
    stack = list(meta["workspace_members"])
    while stack:
        pid = stack.pop()
        if pid in reached:
            continue
        reached.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    seen |= reached
crates = sorted(
    (packages[p] for p in seen if p not in meta["workspace_members"]),
    key=lambda p: (p["name"], p["version"]),
)


MIT_TEXT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


def notice_files(pkg):
    root = Path(pkg["manifest_path"]).parent
    files = [f for f in root.iterdir() if f.is_file() and f.name.upper().startswith(NOTICE_PREFIXES)]
    # Bundled assets (e.g. fonts) keep their licences in their own folder.
    for sub in ("fonts", "licenses", "LICENSES"):
        d = root / sub
        if d.is_dir():
            files += [f for f in d.iterdir() if f.suffix in (".txt", ".md") or f.name.upper().startswith(NOTICE_PREFIXES)]
    if pkg.get("license_file"):
        files.append(root / pkg["license_file"])
    return sorted(set(f.resolve() for f in files if f.exists()))


# Group crates by identical notice text so common licences appear once.
by_text = defaultdict(list)
missing = []
for pkg in crates:
    label = f"{pkg['name']} {pkg['version']}"
    files = notice_files(pkg)
    if not files:
        missing.append(pkg)
    for f in files:
        by_text[f.read_text(encoding="utf-8", errors="replace").strip()].append(f"{label} ({f.name})")

out = sys.stdout
# UTF-8 whatever the locale (on Windows it is not), with Unix line endings.
out.reconfigure(encoding="utf-8", newline="\n")
out.write("Third-party software in the bobview release binaries\n")
out.write("=" * 53 + "\n\n")
out.write(f"bobview, bobview-probe, bobview-testioc and bobview-server (target {TARGET})\n")
out.write(f"and the browser build of bobview ({WEB_TARGET}) include the following\n")
out.write("crates. Licence expressions are as declared by each crate.\n\n")
for pkg in crates:
    licence = pkg["license"] or f"see {pkg['license_file']}"
    source = pkg.get("repository") or "crates.io"
    out.write(f"  {pkg['name']} {pkg['version']}: {licence}  <{source}>\n")

if missing:
    out.write("\nThese crates ship no licence file. Under their declared licences (MIT text\n")
    out.write("below; Apache-2.0 text at the end of this file) the copyright holders are:\n")
    for pkg in missing:
        authors = ", ".join(pkg.get("authors") or ["the authors"])
        out.write(f"  {pkg['name']} {pkg['version']}: Copyright (c) {authors}\n")
    out.write("\nMIT License\n\n" + MIT_TEXT + "\n")

for text, users in sorted(by_text.items(), key=lambda kv: kv[1][0]):
    out.write("\n" + "-" * 78 + "\n")
    out.write("Used by:\n")
    for u in users:
        out.write(f"  {u}\n")
    out.write("\n" + text + "\n")

out.write("\n" + "-" * 78 + "\n")
out.write("Apache License 2.0 (full text, for the crates above that declare it)\n\n")
out.write((ROOT / "LICENSE").read_text(encoding="utf-8").strip() + "\n")

for name, path in EMBEDDED_FONTS:
    out.write("\n" + "-" * 78 + "\n")
    out.write(f"{name}\n\n{path.read_text(encoding="utf-8").strip()}\n")
