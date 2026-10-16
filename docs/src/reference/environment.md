# Environment variables

## EPICS

bobview, `bobview-probe` and `bobview-server` read the standard EPICS client
variables:

| variable | meaning |
| --- | --- |
| `EPICS_CA_ADDR_LIST` | hosts or broadcast addresses to search for CA PVs |
| `EPICS_CA_AUTO_ADDR_LIST` | `NO` to search only `EPICS_CA_ADDR_LIST` |
| `EPICS_CA_NAME_SERVERS` | `host:port` of CA name servers (gateways) reached over TCP |
| `EPICS_PVA_ADDR_LIST` | hosts or broadcast addresses to search for PVA PVs |
| `EPICS_PVA_AUTO_ADDR_LIST` | `NO` to search only `EPICS_PVA_ADDR_LIST` |
| `EPICS_PVA_NAME_SERVERS` | `host:port` of PVA servers reached over TCP |

`bobview-testioc` reads the server variables: `EPICS_CA_SERVER_PORT`,
`EPICS_CAS_*`, `EPICS_PVAS_SERVER_PORT` / `EPICS_PVA_SERVER_PORT` (TCP,
default 5075) and `EPICS_PVAS_BROADCAST_PORT` (UDP search, default 5076).

## bobview

| variable | meaning |
| --- | --- |
| `BOBVIEW_LOG` | log level, e.g. `debug` for more output |
| `BOBVIEW_UI_SCALE` | as `--ui-scale` |
| `XDG_CONFIG_HOME` | layouts are in `$XDG_CONFIG_HOME/bobview/layouts` (default `~/.config/bobview/layouts`) |
| `WAYLAND_DISPLAY` | set it empty to run under XWayland, so layouts keep window positions |

Displays opened from files also fall back to environment variables for
macros they don't define; see [Macros, rules and PV types](macros-rules-pvs.md).

## bobview-server

| variable | meaning |
| --- | --- |
| `BOBVIEW_WRITE_TOKEN` | the write token for `--write`; an empty value counts as unset |

## Tests

The interop tests in `crates/pv/tests/interop.rs` are skipped unless these
name PVs to read:

| variable | meaning |
| --- | --- |
| `BOBVIEW_INTEROP_CA_PV` | a PV served by the IOC over Channel Access |
| `BOBVIEW_INTEROP_PVA_PV` | a PV served by the IOC over PV Access (PVXS) |
| `BOBVIEW_INTEROP_GW_PV` | a PV reached through ca-gateway (CA) |
| `BOBVIEW_INTEROP_WRITE_PV` | optional writable numeric PV; set to 7 and read back |
| `BOBVIEW_INTEROP_REQUIRE` | `1` to fail instead of skipping when a variable is missing |
