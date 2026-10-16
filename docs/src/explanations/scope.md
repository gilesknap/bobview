# Scope and fidelity to Phoebus

## What bobview is for

bobview is a *viewer*. It opens Phoebus Display Builder `.bob` screens and
CS-Studio BOY `.opi` screens and shows live EPICS values on them, from a
single small binary that starts quickly and runs on modest hardware: down to
a phone, in the Android 16 Linux Terminal (Debian 13, aarch64). The same code
builds for the browser.

## What is out of scope

- **Scripts** (Jython and JavaScript) are not run. The status line shows
  "N widgets with scripts ignored", counting widgets, not scripts. A few
  scripts that are common in DLS and techui-builder screens are recognised by
  name and done natively instead; see [How BOY screens are translated](boy.md)
  and [Widgets](../reference/widgets.md).
- **Display editing.** Use Phoebus to edit screens.
- **Alarm and archiver services.** Trends are live only, with no archived
  history.

Widgets bobview can't draw are shown as a grey placeholder labelled with the
widget type and PV name, and the status line counts "N unsupported widgets".
What is and isn't supported is listed in [Widgets](../reference/widgets.md).

## How close it is to Phoebus

Fidelity is approximate. Sizes, fonts (scaled by 0.95) and layout were tuned
by eye, and output has not been compared with Phoebus's own pixel for pixel.

Where behaviour matters more than looks, bobview follows Phoebus closely:
number formats, how typed text is parsed into a value (units dropped, `0x`
and `0b` numbers, enum labels), macro expansion, and the precedence of
operators in rules (as Phoebus's Jython for a `.bob`, as BOY's JavaScript
for an `.opi`). Where it departs from Phoebus it says so in the reference,
for example rules taking the integer part of a double for bitwise operators
where Jython would fail.

## Protocol maturity

Channel Access and PV Access come from the third-party
[epics-rs](https://github.com/epics-rs/epics-rs) crates, which are young; see
[Architecture](architecture.md). So far bobview has mainly been exercised
against the in-repo `bobview-testioc`, not a real softIOC, PVXS or
ca-gateway. The `interop` CI job and `crates/pv/tests/interop.rs` are there
to change that.
