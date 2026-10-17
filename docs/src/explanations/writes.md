# Writes and safety

A viewer that can write to a control system needs to be predictable about
when it does. This page explains the rules.

Every write takes one route. A control or a `write_pv` action calls
`Pv::write` (or `Pv::write_long_string`, for text a string-format widget
writes to a char array). Read-only mode refuses it there. Otherwise it goes
to the PV's writer: the queue of its backend task for CA and PVA, which
converts the text to the PV's type and sends it; the value itself for
`loc://`; or the PV Web Socket in a browser. A refused or failed write
becomes a message in the status line.

## Read-only mode

`--read-only` refuses every write to a PV: controls are disabled and a write
action says in the status line why it did nothing. `loc://` PVs are exempt,
as they live inside bobview and can't reach a control system.

## Access rights

Channel Access reports whether a PV is writable, and controls follow it. PV
Access does not report access rights, so such a PV is assumed writable (the
PV details say "unknown"), and a put the server refuses is shown as a write
error in the status line and the PV details.

## Writes are never applied late

A write to a CA or PVA PV that was connected and has dropped is refused at
once, rather than queued and applied when it comes back, possibly much
later; the status line says so. So is a write while 16 earlier writes to the
same PV still wait.

A write to a PV that has yet to connect (as a `write_pv` action to a PV not
on the display makes) waits for it, up to 5 s over CA, without holding up
the PV's other events. The writes behind it wait in that queue (so while it
waits, a 17th is refused), and fail with it if it gives up.

## Displays from the web

A `write_pv` action and the toggle script write at once, as in Phoebus,
whether the display came from a local file or an `http://` or `https://`
URL. To open displays without letting them write, use `--read-only`, which
refuses every write.

## Commands

No command is ever run for a display fetched over http(s), nor in a
`command_directory` on the web. Only a launcher script that resolves (after
macros, `..` and project links) to a local file beside or under the
display, or in a linked folder of its Eclipse project, is run. Anything
else, such as an absolute path elsewhere or a `../..` escape, is refused and
logged, as is a script that any user may change (world-writable, or in a
world-writable folder). Each script run is logged.

## In the browser

`bobview-server` writes as its own user, so it allows writes only with
`--write`, and each write must carry a token. A page with a token loads
displays only from its own server, and sends the token only to a PVWS on
its own server, so a link naming someone else's display or `&pvws=` cannot
write with it. The page removes the token from its address after reading
it.

Writes to a PV that was connected and has dropped fail at once, and at most
16 may wait for one PV, as on the desktop. One to a PV that has yet to
connect fails after 5 s, which the page sees only as the PV's write error,
not as a message.
