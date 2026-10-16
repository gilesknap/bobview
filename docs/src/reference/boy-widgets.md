# BOY widgets

How each CS-Studio BOY widget in an `.opi` is drawn. Background on the
translation is in [How BOY screens are translated](../explanations/boy.md).

| BOY widget | drawn as |
| --- | --- |
| Rectangle, RoundedRectangle, Ellipse, arc, polyline, polygon | the shapes (BOY borders become outlines, points made relative, `alpha`) |
| Label, TextUpdate, TextInput | label, textupdate, textentry (`format_type`, `precision_from_pv`, borders inside the bounds, `rotation_angle`); a text update or input with text and no PV is a label |
| groupingContainer, linkingContainer | group (style from `border_style`), embedded (`opi_file`, `resize_behaviour`; a title bar or group box border becomes a group around it) |
| ActionButton, MenuButton, BoolButton, choiceButton | action_button (the `push_action_index` action first), combo or action button with a menu, bool_button (an EDM message button with no PV writes its push action), choice |
| LED, bytemonitor, progressbar, meter, scaledslider | led or multi_state_led, byte_monitor, progressbar with its value label, meter, scaledslider |
| edm.symbolwidget, MultistateMonitorWidget, ImageBoolIndicator, Image | symbol (an image strip, or a numbered set of images, nothing for a state without one), picture |
| detailpanel (DLS) | a group with each row's name and content |
| xyGraph, tab (TabbedContainer) | xyplot (numbered axes and traces, rules on a trace's properties), tabs (one grouping container per tab) |
| edm.menumux | combo on its first target PV (writes the item's label) |
| others | placeholders |
