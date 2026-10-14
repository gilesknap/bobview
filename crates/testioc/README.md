# bobview-testioc

A pure-Rust Channel Access IOC that serves a subset of the PVs of the
[epics-containers example-services](https://github.com/epics-containers/example-services)
BL01T beamline, enough for its `opi/demo.bob` screen. It has no C
dependencies, so it runs natively on an aarch64 phone where the real amd64
IOC containers would need qemu. Built on the
[epics-rs](https://github.com/epics-rs/epics-rs) crates; uses about 7 MB RSS.

| part | PVs | how |
| --- | --- | --- |
| `ea` | `bl01t:A`, `bl01t:B`, `bl01t:SUM` | bl01t-ea-test-01's `ioc.db` verbatim, A=2.54, B=2.61 |
| `motor` | `BL01T-MO-SIMC-01:M1`, `:M2` | real motor record (motor-rs) on a simulated axis |
| `cam` | `BL01T-DI-CAM-01:DET:`, `:ARR:`, `:PVA:` | soft records plus a small driver loop |

The camera is not a real simDetector: it serves the ADBase / NDPluginBase
records the demo screen uses (Acquire, AcquireTime, AcquirePeriod,
ArrayCounter, ImageMode, NumImages, DetectorState_RBV, ...) and counts frames
at the acquire period, honouring Single / Multiple / Continuous. No image data
is produced and `pva://BL01T-DI-CAM-01:PVA:OUTPUT` is not served. As in its
`ioc.yaml` it starts acquiring continuously at AcquireTime 0.1.

## Run

```
bobview-testioc [--only ea|motor|cam]...    # default: all three
```

It needs no shell and keeps running with stdin closed. The usual
`EPICS_CA_SERVER_PORT` / `EPICS_CAS_*` variables apply.

On the phone (Linux Terminal VM):

```
bobview-testioc &
EPICS_CA_ADDR_LIST=127.0.0.1 bobview
```

Build for the phone from x86_64:

```
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36 -p testioc
```
