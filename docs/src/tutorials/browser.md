# Serve displays to a browser

bobview also builds for the browser (WebAssembly, drawn with WebGL).
`bobview-server` serves the web build, your `.bob` files and a
[PV Web Socket](https://github.com/ornl-epics/pvws) (PVWS) from one port. In
this tutorial you serve the simulated beamline to a browser on your own
machine.

Do the [Quick start](quickstart.md) first, so that bobview and
`bobview-testioc` are installed.

## 1. Start the IOC and the server

```bash
bobview-testioc &
EPICS_CA_ADDR_LIST=127.0.0.1 bobview-server --opi ~/.local/share/bobview/displays &
```

The server reaches PVs with bobview's own Channel Access and PV Access
client, so it needs the same `EPICS_*` variables as the viewer. It listens
on `127.0.0.1:8080`, so only this machine can reach it.

## 2. Open the page

Browse to
[http://localhost:8080/?display=index.bob](http://localhost:8080/?display=index.bob).

The same displays appear as in the desktop viewer, drawn in the page. Click
through them as before. In a browser displays have no windows of their own:
every link navigates in place.

## 3. Try page parameters

Add macros and a fixed zoom to the address:

```text
http://localhost:8080/?display=demo.bob&macros=P=BL01T&zoom=1.5
```

All the parameters are listed in [Browser page parameters](../reference/browser.md).

## 4. Allow writes

Writes are off unless the server allows them, since they go out as the
server's user. Stop the server and start it again with `--write`:

```bash
kill %2
EPICS_CA_ADDR_LIST=127.0.0.1 bobview-server --opi ~/.local/share/bobview/displays --write
```

It prints a random write token to the terminal. Open the page with it:

```text
http://localhost:8080/?display=index.bob&token=TOKEN
```

The page removes the token from its address once it has read it, so a
reload needs the token again.

## Next steps

To let other machines use the server, read
[Deploy bobview-server for others](../how-to/deploy-server.md) before
binding it to a public address.
