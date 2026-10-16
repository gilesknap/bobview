# Install bobview

bobview runs on x86_64 and aarch64 Linux (including the Android 16 Linux
Terminal, Debian 13) and on macOS 11 or later (Apple silicon or Intel).
`install.sh` picks the release build to match the machine.

## Install the prerequisites

On Debian and Ubuntu:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

On RHEL, Rocky, Alma and Fedora:

```bash
sudo dnf install libxkbcommon libwayland-client libglvnd-egl mesa-dri-drivers
```

None are needed on macOS.

## Install the release

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

This installs into `~/.local`:

| file | what |
| --- | --- |
| `bin/bobview` | the viewer |
| `bin/bobview-ssh` | runs the viewer with software OpenGL, for `ssh -X` and machines whose GPU can't be used; see [Choose bobview or bobview-ssh](software-rendering.md) |
| `bin/bobview-testioc` | a pure-Rust IOC serving the example-services BL01T PVs |
| `bin/bobview-probe` | prints live PV values from the command line |
| `bin/bobview-server` | serves bobview to browsers, see [Deploy bobview-server](deploy-server.md) |
| `share/bobview/displays/` | test screens; `index.bob` opens by default |
| `share/bobview/web/` | the browser build that `bobview-server` serves |

The Linux binaries are linked against glibc 2.28, so they run on RHEL 8+,
Ubuntu 20.04+ and Debian 10+.

## Put it on your PATH

On macOS `~/.local/bin` is not on `PATH` by default; add this to `~/.zshrc`:

```bash
export PATH=$HOME/.local/bin:$PATH
```

## Upgrade or remove

Run the install command again to upgrade; it replaces the binaries and the
`displays` and `web` folders. To remove bobview, delete the `bin/bobview*`
files and `share/bobview` from `~/.local`.
