# bobview

A lean viewer for [Phoebus Display Builder](https://control-system-studio.readthedocs.io/en/latest/app/display/editor/doc/index.html)
`.bob` screens, written in Rust with [egui](https://github.com/emilk/egui).
It also opens the older CS-Studio BOY `.opi` screens.
It shows live EPICS Channel Access and PV Access values, including areaDetector
images, and is small enough to run on a phone, in the Android 16 Linux
Terminal (Debian 13, aarch64). The same app runs in a browser, served by
`bobview-server`.

Scripts (Jython/JavaScript), display editing, alarm and archiver services are
out of scope; see [Scope and fidelity to Phoebus](explanations/scope.md).

<div class="compass">
  <div class="axis axis-top-left">Learning: acquiring skills</div>
  <div class="axis axis-top-right">Working: applying skills</div>
  <div class="axis axis-left-top">Practical: doing</div>
  <div class="axis axis-left-bottom">Theoretical: knowing</div>
  <a class="grid-card tutorials" href="tutorials/quickstart.html">
    <span class="card-title">📚 Tutorials</span>
    <span class="orientation">learning-oriented</span>
    <p>New to bobview? Install it, start the simulated beamline and open
    your first live display.</p>
  </a>
  <a class="grid-card how-to" href="how-to/install.html">
    <span class="card-title">🧭 How-to guides</span>
    <span class="orientation">goal-oriented</span>
    <p>Practical steps: connect to real IOCs, run over ssh, open BOY
    screens, save layouts, deploy the web server.</p>
  </a>
  <a class="grid-card explanations" href="explanations/scope.html">
    <span class="card-title">💡 Explanations</span>
    <span class="orientation">understanding-oriented</span>
    <p>Background and design: what bobview aims to do, how it is built,
    how its windows and writes behave.</p>
  </a>
  <a class="grid-card reference" href="reference/cli.html">
    <span class="card-title">📖 Reference</span>
    <span class="orientation">information-oriented</span>
    <p>Command-line options, supported widgets, macros and rules, browser
    parameters and the Rust API.</p>
  </a>
</div>

The [Rust API docs](reference/api.md) for the workspace's crates are
published alongside this book.

The docs follow the [Diátaxis](https://diataxis.fr) framework: four kinds of
page for four needs, learning, doing, understanding and looking things up.
