//! The toolbars' icons, drawn as strokes so they look the same on every
//! platform (the fonts' symbols vary, and some are emoji).

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    Back,
    Forward,
    Home,
    Reload,
    ZoomOut,
    ZoomIn,
    Navigator,
    Open,
    Pin,
    Menu,
}

impl Icon {
    /// The strokes, as polylines in a 16 × 16 box.
    fn lines(self) -> Vec<Vec<[f32; 2]>> {
        match self {
            Icon::Back => vec![vec![[10.0, 3.0], [5.0, 8.0], [10.0, 13.0]]],
            Icon::Forward => vec![vec![[6.0, 3.0], [11.0, 8.0], [6.0, 13.0]]],
            Icon::Home => vec![
                vec![[2.5, 7.5], [8.0, 3.0], [13.5, 7.5]],
                vec![
                    [4.0, 6.5],
                    [4.0, 13.0],
                    [7.0, 13.0],
                    [7.0, 9.5],
                    [9.0, 9.5],
                    [9.0, 13.0],
                    [12.0, 13.0],
                    [12.0, 6.5],
                ],
            ],
            Icon::Reload => {
                // Most of a circle, clockwise from the top right, and an
                // arrowhead where it starts.
                let arc = (0..=12)
                    .map(|i| {
                        let a = (-50.0 + 25.0 * i as f32).to_radians();
                        [8.0 + 5.0 * a.cos(), 8.0 + 5.0 * a.sin()]
                    })
                    .collect();
                vec![arc, vec![[13.0, 1.8], [11.3, 4.4], [8.6, 3.4]]]
            }
            Icon::ZoomOut => vec![vec![[3.5, 8.0], [12.5, 8.0]]],
            Icon::ZoomIn => vec![vec![[3.5, 8.0], [12.5, 8.0]], vec![[8.0, 3.5], [8.0, 12.5]]],
            Icon::Navigator => vec![
                vec![[3.0, 2.5], [3.0, 13.5]],
                vec![[3.0, 5.0], [6.5, 5.0]],
                vec![[3.0, 9.5], [6.5, 9.5]],
                vec![[3.0, 13.5], [6.5, 13.5]],
                vec![[9.0, 5.0], [13.5, 5.0]],
                vec![[9.0, 9.5], [13.5, 9.5]],
                vec![[9.0, 13.5], [13.5, 13.5]],
            ],
            Icon::Open => vec![vec![
                [2.0, 4.5],
                [6.0, 4.5],
                [7.5, 6.0],
                [14.0, 6.0],
                [14.0, 12.5],
                [2.0, 12.5],
                [2.0, 4.5],
            ]],
            Icon::Pin => vec![
                vec![[6.0, 2.5], [10.0, 2.5]],
                vec![
                    [7.0, 2.5],
                    [7.0, 6.5],
                    [4.5, 9.0],
                    [11.5, 9.0],
                    [9.0, 6.5],
                    [9.0, 2.5],
                ],
                vec![[8.0, 9.0], [8.0, 13.5]],
            ],
            Icon::Menu => vec![
                vec![[3.0, 4.5], [13.0, 4.5]],
                vec![[3.0, 8.0], [13.0, 8.0]],
                vec![[3.0, 11.5], [13.0, 11.5]],
            ],
        }
    }

    /// Draw the icon `size` points square, centred in `rect`.
    pub fn paint(self, painter: &egui::Painter, rect: Rect, size: f32, color: Color32) {
        let scale = size / 16.0;
        let origin = rect.center() - Vec2::splat(size / 2.0);
        let stroke = Stroke::new((1.5 * scale).max(1.0), color);
        for line in self.lines() {
            let points = line
                .iter()
                .map(|[x, y]| origin + Vec2::new(x * scale, y * scale))
                .collect::<Vec<Pos2>>();
            painter.add(Shape::line(points, stroke));
        }
    }
}

/// The height of a toolbar button: a line of button text and its padding.
pub fn height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y
}

/// A button showing `icon`, as tall as a text button: highlighted when
/// `selected`, inert unless `enabled`.
pub fn button(ui: &mut egui::Ui, icon: Icon, selected: bool, enabled: bool) -> egui::Response {
    let height = height(ui);
    let size = Vec2::new(height * 1.15, height);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        // Disabled: as an idle button, only its icon faded.
        let visuals = if enabled {
            ui.style().interact_selectable(&resp, selected)
        } else {
            ui.visuals().widgets.inactive
        };
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            visuals.corner_radius,
            fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let color = if !enabled {
            ui.visuals().weak_text_color()
        } else if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.fg_stroke.color
        };
        icon.paint(painter, rect, height * 0.55, color);
    }
    resp
}

/// The height of a `group`: a button, its frame's margin and stroke.
pub fn group_height(ui: &egui::Ui) -> f32 {
    height(ui) + 2.0 * (GROUP_MARGIN + ui.visuals().widgets.noninteractive.bg_stroke.width)
}

/// Inside a group's frame, around its buttons.
const GROUP_MARGIN: f32 = 1.0;

/// Lay out controls that belong together, `framed` as one; unframed, they
/// take the same room, so they line up with framed groups.
pub fn group<R>(ui: &mut egui::Ui, framed: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    if !framed {
        stroke.color = Color32::TRANSPARENT;
    }
    egui::Frame::new()
        .stroke(stroke)
        .corner_radius(5.0)
        .inner_margin(GROUP_MARGIN)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = if framed { 1.0 } else { 4.0 };
            ui.horizontal(add).inner
        })
        .inner
}
