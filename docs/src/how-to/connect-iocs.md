# Connect to real IOCs

bobview uses the standard EPICS client variables (see
[Environment variables](../reference/environment.md)).

## Search by UDP

Name the hosts (or broadcast addresses) to search:

```bash
EPICS_CA_ADDR_LIST="ioc1.example.com 10.0.0.255" bobview screen.bob
EPICS_PVA_ADDR_LIST=10.0.0.255 bobview screen.bob
```

## Go through a gateway over TCP

When UDP search can't reach the IOCs, for example from a phone's Linux
Terminal VM behind NAT, name a gateway reached over TCP. This works through
NAT since the phone opens the connection:

```bash
EPICS_CA_NAME_SERVERS=gateway.example.com:9064 bobview screen.bob
EPICS_PVA_NAME_SERVERS=gateway.example.com:5075 bobview screen.bob
```

The example-services `ca-gateway` listens on port 9064 of its host.

## Choose the protocol

PV names prefixed `pva://` use PV Access; those with no prefix, or `ca://`,
use Channel Access. The other prefixes are in
[Macros, rules and PV types](../reference/macros-rules-pvs.md#pv-types).

## Check a PV without the viewer

`bobview-probe` prints live values from the command line, and can write:

```bash
bobview-probe bl01t:SUM pva://bl01t:A
bobview-probe --seconds 10 --put bl01t:A=3.5 bl01t:SUM
```

## Watch only, never write

Start the viewer with `--read-only` to refuse every write (see
[Writes and safety](../explanations/writes.md)):

```bash
bobview --read-only screen.bob
```

For more output while diagnosing a connection, set `BOBVIEW_LOG=debug`.
