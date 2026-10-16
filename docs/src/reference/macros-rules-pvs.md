# Macros, rules and PV types

## Macros

- `$(X)` and `${X}`, with defaults.
- Widget properties such as `$(pv_name)`, and `$(actions)` in a tooltip.
- Macros pass down through groups, tabs and embedded displays.
- In displays opened from files, a macro that isn't defined falls back to
  the environment variable of the same name (in widget properties and the
  display name).
- The display name, with macros expanded (or the file name if it has none),
  is the window title.

Give macros on the command line with `-m P=X,R=Y`, or in a browser with
`&macros=P=X,R=Y`.

## Rules

Rules are expressions over `pv0`, `pvStr0`, `pvSev0`, `pvLegacySev0` … that
set any property, or one item of a list such as `symbols[0]`, including the
position, size and PV. Output expression rules are supported, and macros in
rules are expanded.

- Operators: arithmetic, bitwise (including shifts) and logical.
- Operators bind as in Phoebus's Jython for a `.bob`, or BOY's JavaScript
  for an `.opi`.
- A lone `=` is `==` in a `.bob`, as Phoebus makes it.
- Unlike Jython, bitwise operators take the integer part of a double such
  as `pv0`, where Jython fails the rule.
- In an `.opi`, `pvSev0` becomes `pvLegacySev0` (BOY's severity numbers) and
  `widget.getValue()` reads the widget's own PV.

## PV types

| PV name | what |
| --- | --- |
| `name` | Channel Access |
| `ca://name` | Channel Access |
| `pva://name` | PV Access |
| `loc://name` | a local PV inside bobview, never written to the control system |
| `sim://…` | a simulated PV, such as `sim://sine(0, 100, 0.1)` |
| `=expression`, `eq://expression` | a formula PV |

Not yet supported: `sys://` and other PV types (shown as not supported), and
PV Access field paths (`pva://name/field`). In a browser, CA and PVA PVs
come over PVWS, and `loc://` and `sim://` PVs are made in the page.
