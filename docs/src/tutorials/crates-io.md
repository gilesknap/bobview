# Install from crates.io

This tutorial builds bobview from [crates.io](https://crates.io/crates/bobview)
with Cargo, instead of downloading a release, and opens the test displays
against the simulated beamline. Use it on a machine the release builds don't
cover, or to build with your own Rust toolchain. It takes about fifteen
minutes, most of it compiling.

## 1. Install Rust

Install Rust with [rustup](https://rustup.rs) if you don't have it:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

and open a new terminal, so that `~/.cargo/bin` is on your `PATH`. The build
also needs a C compiler: `sudo apt install build-essential` on Debian and
Ubuntu, `sudo dnf install gcc` on RHEL and Fedora, or `xcode-select --install`
on macOS.

## 2. Install the graphics libraries

bobview needs these at run time, as the release does. On Debian or Ubuntu:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

On RHEL, Rocky, Alma and Fedora:

```bash
sudo dnf install libxkbcommon libwayland-client libglvnd-egl mesa-dri-drivers
```

macOS needs nothing.

## 3. Install bobview and the test IOC

```bash
cargo install bobview bobview-testioc
```

This builds and installs into `~/.cargo/bin`:

| command | what |
| --- | --- |
| `bobview` | the viewer |
| `bobview-probe` | prints live PV values from the command line |
| `bobview-testioc` | a pure-Rust IOC serving the example-services BL01T PVs |

Check it works:

```bash
bobview --version
```

## 4. Start the simulated beamline

```bash
bobview-testioc &
```

## 5. Open the test displays

Cargo installs only the programs, not the test displays that a release
installs. bobview opens displays over https, so open them from GitHub:

```bash
EPICS_CA_ADDR_LIST=127.0.0.1 bobview https://raw.githubusercontent.com/gilesknap/bobview/main/tests/displays/index.bob
```

Their links to other displays and images are fetched from the same place.
You should see live values within a second or two. From here, carry on from
[step 4 of the Quick start](quickstart.md#4-look-around).

## Upgrade or remove

Run `cargo install bobview bobview-testioc` again to upgrade to the latest
version, and `cargo uninstall bobview bobview-testioc` to remove them.

## What Cargo does not install

- `bobview-ssh`, the wrapper for `ssh -X` (see
  [Choose bobview or bobview-ssh](../how-to/software-rendering.md)). It only
  sets three environment variables, so you can set them yourself or copy
  [the script](https://github.com/gilesknap/bobview/blob/main/scripts/bobview-ssh)
  next to `bobview`.
- The browser build. `cargo install bobview-server` installs the server,
  but it needs the web build from a release or from
  [Build the browser version](../how-to/build.md#build-the-browser-version),
  given with `--web DIR`. The [browser tutorial](browser.md) uses the release.
