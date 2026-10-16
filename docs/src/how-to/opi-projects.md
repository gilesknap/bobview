# Open BOY screens from an Eclipse project or DLS module

Any `.opi` file opens like a `.bob`:

```bash
bobview screen.opi
```

BOY screens often name each other through the CS-Studio workspace, though:
an Eclipse project whose `.project` links folder names (`mks937a`, `dlsPLC` …)
to the directories that hold each module's screens, so `../mks937a/gauge.opi`
means the `mks937a` link. This page shows how to open such screens so that
their links work. [How BOY screens are translated](../explanations/boy.md)
explains the rules behind it.

## Through an Eclipse project

Open the screen through its project path:

```bash
bobview /workspaces/VacOPIS/vacuum_4-100/vacuum/diamondVacuumOverview.opi
```

for a project in `/workspaces/VacOPIS/vacuum_4-100`. bobview follows the
project's links (the linked folders need not exist on disk; **Open…** lists
them).

Or give the screen's real path and name the project with `--project`:

```bash
bobview -p /workspaces/VacOPIS/vacuum_4-100 /dls_sw/…/vacuumApp/opi/opi/diamondVacuumOverview.opi
```

## A DLS module

DLS modules need neither. There is no `.project` on disk, but the generated
launcher `runcss.sh` beside a module's screens (`vacuumApp/opi/opi`) lists
the links it gives CS-Studio. bobview reads that list (it does not run the
script), so just open the screen by its real path:

```bash
bobview /dls_sw/…/vacuum/4-100/vacuumApp/opi/opi/diamondVacuumOverview.opi
```

This works for a BOY screen reached by its real path from anywhere (the file
chooser, a layout, an absolute path in another screen), with the launcher in
its folder or up to two above.

To open another module's BOY screen through this module's links, give `-p`
the module's screens folder (or its `runcss.sh`).

## A screen that needs launcher macros

DLS screens are often opened by a launcher script, such as
`./srCellVacuum 04`, that supplies macros. For an `.opi` given on the command
line in an Eclipse project (through its project path or with `--project`),
bobview tries the launcher scripts in its directory to find them: the one
named `<screen>.sh` first, with no arguments and with each number in the
file name, bare and with an `r`. So:

| screen | macros from |
| --- | --- |
| `linacVacuum.opi` | `linacVacuum.sh` (`dom=LI`) |
| `srCell05Vacuum.opi` | `srCellVacuum 05` |
| `SR05C_rack.opi` | `srCellVacuum 05r` |

Macros given with `-m` override these. Screens with no number in their name
(`srInjCellVacuum.opi` is cell 01) need `-m` with the macros their
launcher would give, or opening from the overview.
