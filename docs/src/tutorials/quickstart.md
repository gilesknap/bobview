# Quick start

This tutorial installs bobview, starts a simulated beamline on your own
machine and walks you around its displays. It takes about ten minutes and
needs no EPICS installation and no IOCs: `bobview-testioc` plays the part of
the [epics-containers example-services](https://github.com/epics-containers/example-services)
BL01T beamline.

You need an x86_64 or aarch64 Linux machine (an Android phone's Linux
Terminal works), or a Mac. To build it with Cargo instead, see
[Install from crates.io](crates-io.md).

## 1. Install

On Debian or Ubuntu, first install the graphics libraries:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

(on RHEL and Fedora see [Install bobview](../how-to/install.md); macOS needs
nothing). Then install bobview into `~/.local`:

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/bobview/main/install.sh | bash
```

Check that it is on your `PATH`:

```bash
bobview --version
```

If the command is not found, add `export PATH=$HOME/.local/bin:$PATH` to your
shell's start-up file and open a new terminal.

## 2. Start the simulated beamline

```bash
bobview-testioc &
```

This serves, over Channel Access and PV Access, a calculator IOC
(`bl01t:A`, `bl01t:B`, `bl01t:SUM`), two motors and a simulated area
detector that streams an image.

## 3. Open the displays

Tell the Channel Access client to look for PVs on this machine, and start
the viewer:

```bash
EPICS_CA_ADDR_LIST=127.0.0.1 bobview
```

With no display named, bobview opens `index.bob` from the test screens it
installed. You should see live values within a second or two.

## 4. Look around

- **Hover** over a widget: a tooltip shows its PV and value.
- **Right-click** a widget (long-press on a touch screen): copy the PV name
  or value, open the PV's details, or **Trend** it. A trend window plots the
  PV against time; leave it open.
- **Middle-click** a widget to copy its PV name; a middle-click in a
  terminal pastes it.
- **Click a button** that opens another display. Use **Back** and
  **Forward** in the toolbar to move between them, and **Home** to return
  to `index.bob`.
- Click the **zoom** in the toolbar to see the display at 100%, then **Fit**
  to fit it to the window again. On a touch screen, pinch to zoom.
- Open the **navigator** from the toolbar to see a tree of every display
  reachable from the home display.

## 5. Look at the camera

From the home display, open the camera display. The image comes over PV
Access from `BL01T-DI-CAM-01:PVA:OUTPUT`: drifting fringes with three spots
moving across them. Use the image toolbar to zoom, pan and place a crosshair
that reads out the pixel under it.

## 6. Change a value

From the home display, open the `ea-test` display. Type a new number into
the text entry for `bl01t:A` and press Enter, or drag its slider.
`bl01t:SUM` follows, as the calculator record adds `A` and `B`.

## 7. Finish

Close the window to quit bobview, then stop the IOC:

```bash
kill %1
```

## Next steps

- Point bobview at your own IOCs: [Connect to real IOCs](../how-to/connect-iocs.md).
- Open your own screens: `bobview path/to/screen.bob -m P=BL01T,R=:DET:`.
- Show the same displays in a web browser: [Serve displays to a browser](browser.md).
