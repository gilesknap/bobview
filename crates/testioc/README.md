# bobview-testioc

A pure-Rust EPICS IOC that serves, over Channel Access and PV Access, a subset of the PVs of the
[epics-containers example-services](https://github.com/epics-containers/example-services)
BL01T beamline, enough for its `opi/demo.bob` screen. It has no C
dependencies, so it runs natively on an aarch64 phone where the real amd64
IOC containers would need qemu. Built on the
[epics-rs](https://github.com/epics-rs/epics-rs) crates; uses about 15 MB RSS
while streaming images.

| part | PVs | how |
| --- | --- | --- |
| `ea` | `bl01t:A`, `bl01t:B`, `bl01t:SUM` | bl01t-ea-test-01's `ioc.db` verbatim, A=2.54, B=2.61 |
| `motor` | `BL01T-MO-SIMC-01:M1`, `:M2` | real motor record (motor-rs) on a simulated axis |
| `cam` | `BL01T-DI-CAM-01:DET:`, `:ARR:`, `:PVA:` | soft records plus a small driver loop |

The camera is not a real simDetector: it serves the ADBase / NDPluginBase
records the demo screen uses (Acquire, AcquireTime, AcquirePeriod,
ArrayCounter, ImageMode, NumImages, DetectorState_RBV, FilePath, ...) and counts frames
at the acquire period, honouring Single / Multiple / Continuous. As in its
`ioc.yaml` it starts acquiring continuously at AcquireTime 0.1.

## PV Access

Every record is also served over PVA the way QSRV does it: NTScalar for
analog and long records (with display, control and alarm metadata), NTEnum
for bi / bo / mbbi / mbbo, NTScalarArray for waveforms; gets, puts and
monitors all work.

`BL01T-DI-CAM-01:PVA:OUTPUT` is an NTNDArray like NDPluginPVA's output. The
image is simulated: 1024x1024 Int8 (`byteValue`, pixels 0..255 stored as i8
bit patterns, as an Int8 simDetector produces), uncompressed, with drifting
diagonal fringes, three gaussian spots on Lissajous paths and some noise.
A frame is posted for every frame counted (10 Hz by default) while
`DET:ArrayCallbacks` and `PVA:EnableCallbacks` are both on; `uniqueId` is the
frame counter. To look at a frame without a viewer:

```
cargo run -p bobview-testioc --example frame_png -- /tmp/frame 1 50
```

## Run

```
bobview-testioc [--only ea|motor|cam]...    # default: all three
```

It needs no shell and keeps running with stdin closed. The usual
`EPICS_CA_SERVER_PORT` / `EPICS_CAS_*` variables apply for CA, and
`EPICS_PVAS_SERVER_PORT` / `EPICS_PVA_SERVER_PORT` (TCP, default 5075) and
`EPICS_PVAS_BROADCAST_PORT` (UDP search, default 5076) for PVA. `--only`
applies to both protocols.

On the phone (Linux Terminal VM):

```
bobview-testioc &
EPICS_CA_ADDR_LIST=127.0.0.1 bobview
```

Build for the phone from x86_64:

```
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36 -p bobview-testioc
```
