# Browser page parameters

The web build's page takes these query parameters:

```text
http://HOST:PORT/?display=FILE.bob&macros=P=X,R=Y&zoom=1.5
```

| parameter | meaning |
| --- | --- |
| `display` | the display to open, a path relative to the page or a URL (default `index.bob`); a URL on another server must allow cross-origin requests, and is refused when the page has a write token |
| `macros` | macros, `P=X,R=Y` |
| `zoom` | a fixed zoom factor instead of fitting the page |
| `pvws` | another PVWS server: `host[:port]` (meaning `wss://host[:port]/pvws/pv`, or `ws://` when the page is plain `http:` and the port isn't 443), or a full `ws://` or `wss://` URL; default `/pvws/pv` on the page's own server |
| `token` | the write token of a `bobview-server` started with `--write`; removed from the address once read |

## Without bobview-server

The web build (`dist/web` from `scripts/build-web.sh`, or
`share/bobview/web` in a release) is a static site: serve it from any web
server and point `pvws` at any PVWS server. The display's server must allow
the page's origin (CORS) unless it is the same server.

## Differences from the desktop

- Displays have no windows of their own: every link navigates in place, and
  the pin and layout controls are not shown.
- Images from PVA are not sent over PVWS yet.
