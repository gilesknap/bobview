# Install bobview

bobview runs on x86_64 and aarch64 Linux (including the Android 16 Linux
Terminal, Debian 13), on macOS 11 or later (Apple silicon or Intel), and on
x86_64 Windows ([see below](#windows)). `install.sh` picks the release build
to match the machine. To build it from crates.io with Cargo instead, see
[Install from crates.io](../tutorials/crates-io.md).

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

## Windows

The Windows build is new, and has not yet been tried against real IOCs.

Download `bobview-x86_64-windows.zip` from the
[latest release](https://github.com/gilesknap/bobview/releases/latest), unzip
it anywhere, and run `bin\bobview.exe`. Nothing else needs installing. The
zip holds the same files as the table above, as `.exe`, without `bobview-ssh`
and `install.sh`; add its `bin` folder to `PATH` to run the commands from a
terminal. To try it against the simulated beamline, in PowerShell:

```powershell
Start-Process bin\bobview-testioc.exe
$env:EPICS_CA_ADDR_LIST = "127.0.0.1"
bin\bobview.exe
```

Windows may ask to let `bobview-testioc` and `bobview` through the firewall:
Channel Access and PV Access search for PVs over UDP.

bobview draws with OpenGL, which a GPU driver provides. A virtual machine or
remote desktop session without one has only Windows' built-in OpenGL 1.1,
which is too old, and bobview fails to start. There, put Mesa's software
`opengl32.dll` (llvmpipe, for example from
[mesa-dist-win](https://github.com/pal1000/mesa-dist-win/releases)) beside
`bobview.exe`, and it draws on the CPU instead.

Layouts are saved in `%APPDATA%\bobview\layouts`. DLS launcher scripts,
which are shell scripts, are not run on Windows, so their command actions do
nothing (see [How BOY screens are translated](../explanations/boy.md#launcher-commands)).
To remove bobview, delete the unzipped folder and `%APPDATA%\bobview`.
