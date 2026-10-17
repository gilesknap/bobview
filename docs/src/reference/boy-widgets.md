# BOY widgets

How each CS-Studio BOY widget in an `.opi` is drawn. Background on the
translation is in [How BOY screens are translated](../explanations/boy.md).

| BOY widget | drawn as |
| --- | --- |
| Rectangle, RoundedRectangle, Ellipse, arc, polyline, polygon | the shapes (BOY borders become outlines, points made relative, `alpha`) |
| Label, TextUpdate, TextInput, NativeText | label, textupdate, textentry (`format_type`, `precision_from_pv`, borders inside the bounds, `rotation_angle`); a text update or input with text and no PV is a label |
| groupingContainer, linkingContainer | group (style from `border_style`), embedded (`opi_file`, `resize_behaviour`; a title bar or group box border becomes a group around it) |
| ActionButton, NativeButton, MenuButton, BoolButton, ImageBoolButton, BoolSwitch | action_button (the `push_action_index` action first; an EDM message button, a BoolButton with no PV, becomes one that writes its push action), combo or action button with a menu, bool_button, slide_button |
| choiceButton, radioBox, checkbox, combo, spinner, scrollbar | choice, radio, checkbox, combo, spinner, scrollbar |
| LED, bytemonitor, progressbar, tank, thermometer, meter, gauge, scaledslider, knob | led or multi_state_led, byte_monitor, progressbar with its value label, tank, thermometer, meter, scaledslider |
| edm.symbolwidget, MultistateMonitorWidget, BoolMonitorWidget, ImageBoolIndicator, Image | symbol (an image strip, or a numbered set of images, nothing for a state without one), picture |
| detailpanel (DLS) | a group with each row's name and content |
| xyGraph, intensityGraph, tab (TabbedContainer) | xyplot (numbered axes and traces, rules on a trace's properties), image, tabs (one grouping container per tab) |
| edm.menumux | combo on its first target PV (writes the item's label) |
| others | placeholders |
