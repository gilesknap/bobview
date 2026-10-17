# bobview

[![CI](https://github.com/gilesknap/bobview/actions/workflows/ci.yml/badge.svg)](https://github.com/gilesknap/bobview/actions/workflows/ci.yml)
[![Docs](https://github.com/gilesknap/bobview/actions/workflows/docs.yml/badge.svg)](https://gilesknap.github.io/bobview/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens (and CS-Studio BOY `.opi` screens), written in Rust with
[egui](https://github.com/emilk/egui). It shows live EPICS Channel Access and
PV Access values, including areaDetector images, and is small enough to run
on a phone. The same app runs in a browser, served by `bobview-server`.

| | |
| --- | --- |
| Source | <https://github.com/gilesknap/bobview> |
| Documentation | <https://gilesknap.github.io/bobview> |
| Releases | <https://github.com/gilesknap/bobview/releases> |

Install on Linux (x86_64 or aarch64) or macOS, then open the bundled
displays against the simulated beamline:

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
bobview-testioc &
EPICS_CA_ADDR_LIST=127.0.0.1 bobview
```

Linux needs a few graphics libraries first; see
[Install bobview](https://gilesknap.github.io/bobview/main/how-to/install.html).
Or build it with Cargo: `cargo install bobview bobview-testioc`; see
[Install from crates.io](https://gilesknap.github.io/bobview/main/tutorials/crates-io.html).

See the [documentation](https://gilesknap.github.io/bobview) for everything else.
