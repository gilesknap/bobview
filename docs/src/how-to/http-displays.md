# Open displays over http(s)

A display can be a file or an `http://` / `https://` URL, on the command line
or typed into **Open…**:

```bash
bobview http://host:8080/opi/demo.bob
```

Files that a display names (buttons, embedded displays, navigation tabs) are
found relative to it, so a directory of screens served by any web server
works as it does from disk. For example, to serve a folder of screens:

```bash
cd /path/to/screens && python3 -m http.server 8000
bobview http://localhost:8000/index.bob
```

Fetched screens are kept in memory for **Back** and **Forward**; **Reload**
fetches them again.

A display loaded over http(s) is treated with more caution than a local
file: a `write_pv` action asks for confirmation before it writes, and no
command is ever run for it. See [Writes and safety](../explanations/writes.md).
