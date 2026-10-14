# bobview

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens, written in Rust with [egui](https://github.com/emilk/egui).
It shows live EPICS Channel Access values and is small enough to run on a phone,
in the Android 16 Linux Terminal (Debian 13, aarch64).

Scripts (Jython/JavaScript), display editing, alarm and archiver services are
out of scope.

## Install on the phone

In the Android Linux Terminal:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

This puts `bobview` in `~/.local/bin`.

## Build

```bash
cargo build --release
```

CI builds the aarch64 binary natively on GitHub's `ubuntu-24.04-arm` runners
and attaches `bobview-aarch64-linux.tar.gz` to a GitHub Release for every `v*` tag.
