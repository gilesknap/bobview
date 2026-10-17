# Widgets

The Phoebus Display Builder widgets bobview draws, and how far it goes with
each. BOY `.opi` widgets are translated to these; see
[BOY widgets](boy-widgets.md).

| widgets | notes |
| --- | --- |
| label, textupdate, textentry | formats: default, decimal, exponential, engineering, hex, compact, string, sexagesimal (also HMS and DMS), binary, significant; units, precision; typed text is parsed as Phoebus does (units dropped, `0x`/`0b` numbers, enum labels) |
| led, multi_state_led, byte_monitor | |
| action_button, bool_button, slide_button, choice, radio, combo, checkbox | open display, write PV and open webpage actions; command actions only for DLS launcher scripts and StripTool (see [How BOY screens are translated](../explanations/boy.md#launcher-commands)) |
| scaledslider, scrollbar | set by a click or drag with the primary (left) button |
| spinner | its arrows (on the left with `buttons_on_left`), or Up and Down while it has focus, step the value by `increment` within its range; or type a value and press Enter. As in Phoebus, a drag does nothing |
| linearmeter | scale, warning regions from the levels (the PV's alarm limits with `limits_from_pv`), limit markers, needle; horizontal or vertical |
| meter | a dial: scale (linear or `log_scale`), warning regions from the levels (the PV's alarm limits with `limits_from_pv`), needle |
| progressbar, tank, thermometer | one shared bar renderer (tank and thermometer are vertical bars with their own default fill colours) |
| xyplot | line, step and bar traces with markers against index or an x PV; several y axes, autoscale, log scale, grid, legend |
| stripchart | its traces' PVs over the last `time_range`, sampled from when the display opened, on the first y axis (autoscale or fixed, log scale, grid), legend |
| rectangle, ellipse, arc, polyline, polygon | dashed and dotted `line_style` for rectangles and polylines |
| group, tabs, embedded, navtabs | macros pass down; embedded resize modes 0–2 and 4 (crop) |
| image | PVA NTNDArray (uncompressed, mono or RGB1) or a CA array shaped by `data_width`/`data_height`; Phoebus colour maps, autoscale or fixed range, `unsigned`, colour bar with a scale; toolbar to zoom, pan and place a crosshair that reads out the pixel value; decimated when a side is over 2048 pixels, to keep drawing fast |
| symbol, picture | SVG or PNG files, local or fetched next to the display; the PV value picks the symbol (`initial_index`, `array_index`, `fallback_symbol`; an empty one draws nothing), rotation, `preserve_ratio`/`stretch_image`, opacity, overlay for a disconnected PV; `run_actions_on_mouse_click` runs the symbol's actions when clicked; images inside an SVG are drawn only when embedded (`data:` URLs); a strip of frames `sub_image_width` wide (DLS EDM symbols) shows frame `image_index` |

## Common to all widgets

`border_width`/`border_color` borders, alarm borders, dashed borders for
disconnected PVs, and label and text update `rotation_step`. Macros and
rules are described in [Macros, rules and PV types](macros-rules-pvs.md).

## Unsupported

Unsupported widgets, such as table, webbrowser, array, fileselector and
byte-array, are drawn as a grey placeholder labelled with the widget type
and PV name, and counted in the status line.

## Not yet

- PV Access field paths (`pva://name/field`), and `sys://` and other PV
  types (shown as not supported)
- compressed (codec) images, image axes, ROIs and cursor PVs
- the XY plot `markers`, error bars and toolbar
- the strip chart's toolbar and second y axes
- GIF and JPEG images (symbols and pictures show their fallback or a
  placeholder), and text inside SVG images
- scripts: an `execute` action logs that it is not supported, except the
  "toggle the widget's PV" script that techui-builder puts in synoptic
  symbols, which bobview recognises and runs itself
