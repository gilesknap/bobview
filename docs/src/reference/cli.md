# Command line

## bobview

```text
bobview [OPTIONS] [DISPLAY]
```

`DISPLAY` is a `.bob` or `.opi` file, or an `http://` or `https://` URL.
With no display, bobview restores the default window layout when one has
been saved, else opens `index.bob` from the bundled displays
(`~/.local/share/bobview/displays`) when present, else a file chooser.

| option | meaning |
| --- | --- |
| `-m`, `--macros P=X,R=Y` | macros for the display |
| `-p`, `--project DIR` | open the display through the linked folders of the Eclipse project in `DIR` (its `.project`, or for a BOY `.opi` screen the links of a DLS `runcss.sh` in `DIR`); see [Open BOY screens](../how-to/opi-projects.md) |
| `-z`, `--zoom FACTOR` | fixed zoom, 0.2 to 5, instead of fitting the window |
| `-f`, `--fullscreen` | start full screen |
| `--read-only` | refuse every write to a PV except `loc://` ones; see [Writes and safety](../explanations/writes.md) |
| `--archiver URL` | the EPICS Archiver Appliance trends fetch history from, e.g. `http://archiver:17668` (`/retrieval` is added); remembered for later runs (not with `--snapshot`). See [Trend PVs](../how-to/trends.md) |
| `--no-archiver` | no archiver this run, even if `BOBVIEW_ARCHIVER` or the settings name one: trends are live only |
| `--layout NAME` or `FILE` | restore the saved layout `NAME`, or the layout in the JSON file `FILE` (a path with a directory in it or ending in `.json`); **Save layout** then writes back to that file. See [Save and restore a layout](../how-to/layouts.md) |
| `--no-layout` | don't restore the default layout |
| `--ui-scale F` or `auto` | size of the toolbar and menus relative to the desktop's scale, 1 being the desktop's own (default auto: smaller on a small high-DPI screen) |
| `--snapshot OUT.png` | render to a PNG without a window and exit; see [Take snapshots](../how-to/snapshots.md) |
| `--size WxH` or `auto` | snapshot size in pixels (default `1280x800`); `auto` sizes it as a window manager would, on a 1920x1080 monitor |
| `--scale-factor F` | snapshot as on a screen with this desktop scale, e.g. 2.75 for a phone (default 1) |
| `--wait SECONDS` | time for PVs to connect before a snapshot (default 2) |
| `--click X,Y` | click here before the snapshot (repeatable) |
| `--right-click X,Y` | right-click here before the snapshot (repeatable) |
| `--middle-click X,Y` | middle-click here before the snapshot (repeatable) |
| `--settle SECONDS` | time after the last click before the snapshot (default 1) |
| `-h`, `--help` | show help |
| `-V`, `--version` | show the version |

## bobview-ssh

```text
bobview-ssh [OPTIONS] [DISPLAY]
```

Runs `bobview` with the same arguments, rendering with Mesa in software. See
[Choose bobview or bobview-ssh](../how-to/software-rendering.md).

## bobview-probe

```text
bobview-probe [--seconds N] [--put NAME=VALUE]... NAME...
```

Prints live values of PVs through bobview's PV layer, for `N` seconds
(default 5). `--put` writes a value first (repeatable). PV names take the
same prefixes as in a display (`pva://`, `loc://` …).

## bobview-server

```text
bobview-server [OPTIONS]
```

Serves the bobview web build, `.bob` files and a PV Web Socket on one port.
Open `http://HOST:PORT/?display=FILE.bob` in a browser (default
`index.bob`). See [Deploy bobview-server for others](../how-to/deploy-server.md).

| option | meaning |
| --- | --- |
| `-p`, `--port PORT` | port to listen on (default 8080) |
| `--bind ADDR` | address to listen on (default `127.0.0.1`, this machine only; `0.0.0.0` is every interface) |
| `--web DIR` | the web build from `scripts/build-web.sh` (default `../share/bobview/web` next to this binary, else `dist/web`) |
| `--opi DIR` | serve `.bob` files from here; they take precedence over files of the same name in the web build |
| `--follow-symlinks` | serve files through symbolic links that lead out of `--opi` or `--web` (refused by default) |
| `--write` | let browsers write to PVs (off by default, as in PVWS); every write must carry the write token |
| `--write-token TOKEN` | the token `--write` wants (default: `$BOBVIEW_WRITE_TOKEN` if set and not empty, else a random one printed at startup); prefer the variable, as arguments show in `ps` |
| `--allow-origin ORIGIN` | also let pages from `ORIGIN` connect, e.g. `https://host.example.com` (repeatable) |
| `--host NAME` | let browsers call this server `NAME`, as well as an IP address, `localhost`, this machine's name and the hosts of `--allow-origin` (repeatable) |
| `--max-per-ip N` | most clients at once from one address (default 10) |
| `--throttle-ms MS` | least time between two batches of updates to one client (default 100) |
| `-h`, `--help` | show help |
| `-V`, `--version` | show the version |

## bobview-testioc

```text
bobview-testioc [--only ea|motor|cam]...
```

Serves the simulated BL01T PVs; by default all three parts. See
[bobview-testioc](testioc.md).
