# Choose bobview or bobview-ssh

bobview draws with OpenGL. Two commands start it:

| command | draws with | use it |
| --- | --- | --- |
| `bobview` | the machine's own OpenGL: the GPU, or whatever driver the desktop provides | normally: on a local desktop, a laptop, a Mac, the phone |
| `bobview-ssh` | Mesa in software, on the CPU of the machine bobview runs on | when the OpenGL path can't work, such as over `ssh -X` |

Both take the same arguments. Start with `bobview`, and switch to
`bobview-ssh` if it fails to start or draws nothing.

## When to use bobview-ssh

- **Over `ssh -X` or `-Y`.** This is what it was named for. Forwarded OpenGL
  reaches your local X server as indirect GLX, which current X servers
  refuse, so `bobview` exits with `glutin error: [2] BadValue`.
- **Where a GPU driver is installed but can't be used**, for example a
  remote desktop, VNC or Xvfb session on a machine with an NVIDIA driver:
  glvnd picks NVIDIA's GL, which then has no GPU display to draw on.
- **Where the GPU driver is broken** or misbehaves, as a workaround while
  you sort it out.

There is no need for it on macOS, on Wayland or X11 desktops with working
graphics, in the Android Linux Terminal, or for `--snapshot`, unless one of
the cases above applies.

## What it does

`bobview-ssh` is a small script that runs `bobview` with:

- `LIBGL_ALWAYS_SOFTWARE=1`, so Mesa renders on the CPU (llvmpipe);
- `__GLX_VENDOR_LIBRARY_NAME=mesa`, and `__EGL_VENDOR_LIBRARY_FILENAMES`
  pointing at Mesa's EGL vendor file, so libglvnd picks Mesa rather than a
  GPU vendor's library such as NVIDIA's.

Over X forwarding the finished window then travels to your X server as
plain images, which every X server accepts.

Software rendering costs CPU on the machine running bobview, but bobview's
displays are simple to draw, so it is usually quick enough. Over `ssh -X`
each redraw sends the whole window over the network, so bobview redraws a
window only for its own input and when a PV it shows changes, at most 10
times a second (see [Pinned windows and the viewer](../explanations/windows.md#redrawing)).

## Requirement

Mesa must be installed where bobview runs: `mesa-dri-drivers` on RHEL,
`libgl1-mesa-dri` on Debian and Ubuntu (both are in the
[install prerequisites](install.md)).

## Example

```bash
ssh -X control-host
bobview-ssh /path/to/screen.bob -m P=BL01T
```

Layouts restore window positions over X forwarding; see
[Save and restore a layout](layouts.md).
