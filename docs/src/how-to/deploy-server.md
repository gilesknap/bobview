# Deploy bobview-server for others

[Serve displays to a browser](../tutorials/browser.md) runs `bobview-server`
for this machine only. This guide makes it reachable from others, safely.
All of its options are in [Command line](../reference/cli.md#bobview-server).

## Run it where PVs are reachable

The server reaches PVs through bobview's own CA/PVA client, so run it
anywhere `caget` works, with the usual `EPICS_*` variables:

```bash
bobview-server --opi /path/to/displays --bind 0.0.0.0
```

The release tarball includes the web build, which the server finds in
`share/bobview/web`. Files in `--opi` take precedence over files of the same
name in the web build (which has the test displays).

## Choose who can reach it

By default it listens on `--bind 127.0.0.1 --port 8080`. Use `--bind 0.0.0.0`
(every interface) or a specific address to serve others, ideally behind a
firewall or an authenticating reverse proxy, since reading PVs needs no
login.

Browsers let any page open a websocket, so the server accepts only pages
from itself (by `Origin`), plus any you name:

```bash
bobview-server --opi DIR --bind 0.0.0.0 --allow-origin https://ops.example.com
```

Those pages must also call the server by an IP address, `localhost`, this
machine's name (as the kernel has it, without its domain, or in a domain of
`/etc/resolv.conf`), the host of an `--allow-origin`, or a name given with
`--host NAME` (repeatable), so that a page cannot reach it through a DNS
name of its own.

## Behind a reverse proxy

- Give the public name the proxy passes on as `Host` with `--host NAME`.
- Every client has the proxy's address, so raise `--max-per-ip` (default 10)
  and expect the logs to show the proxy.

## Allow writes

Writes are off unless the server has `--write`, since they go out as the
server's user. Each write must then carry a token. Set it in the environment
rather than with `--write-token`, since arguments show in `ps`:

```bash
BOBVIEW_WRITE_TOKEN=s3cret bobview-server --opi DIR --write
```

With neither, the server makes a random token and prints it to stderr at
startup; an empty `BOBVIEW_WRITE_TOKEN` counts as unset. Users open the page
with `&token=TOKEN`. See [Writes and safety](../explanations/writes.md) for
how the token is protected.

## Limits it enforces

- At most 100 clients and 50,000 subscribed PVs.
- At most 10 clients from one address (or one IPv6 /64), `--max-per-ip N`.
- A client subscribing to more than 1,000 new PVs a second, after its first
  10,000, is slowed down.
- A client that stops answering websocket pings, or takes over 10 s to take
  one message, is dropped.
- PV names over 256 characters, or with spaces or non-ASCII characters, are
  refused.
- Each client gets the PVs that changed at most every `--throttle-ms`
  (100 ms), each with its latest state.

It serves no `loc://` or `sim://` PVs: the page makes those itself. Images
from PVA are not sent over PVWS yet.

## Files it will not serve

Dot-files such as `.git` are never served, nor files reached through a
symbolic link that leads out of `--opi` or `--web` (unless
`--follow-symlinks`). Other sites cannot frame the pages (`X-Frame-Options`,
`frame-ancestors 'self'`).
