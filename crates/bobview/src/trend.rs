//! Trend windows: PVs plotted against time, archived history and live
//! values, by archiveview's [`Plot`]. Opened from a widget's context menu,
//! a BOY "Graph" button, or a Data Browser or StripTool launcher. The
//! plot fetches history from the Archiver Appliance bobview was given
//! (`--archiver`), or is live only without one.

use archiveview::{ArchiverClient, Plot, View};
use eframe::egui;
use pv::PvManager;

use crate::layout::Geometry;

/// A trend window's plot, and where its window is.
pub struct Trend {
    /// Tells this window from the others.
    pub id: u64,
    plot: Plot,
    /// Where to open the window, when a layout says.
    pub place: Geometry,
    /// Where the window is, for saving a layout.
    pub geometry: Geometry,
}

impl Trend {
    /// A trend of the PVs `names`, each once, subscribed through `pvs`
    /// (tagged to repaint its window) and with history from `archiver`.
    pub fn new(
        id: u64,
        names: &[String],
        pvs: &PvManager,
        archiver: Option<ArchiverClient>,
        recent: &[String],
    ) -> Self {
        let mut plot = Plot::with_id_salt(pvs, archiver, ("bobview-trend", id));
        // The pill switches archivers, for every trend: see
        // `BobApp::set_archiver`. Not in a browser, which can't reach one.
        plot.set_archiver_switching(!cfg!(target_arch = "wasm32"));
        plot.set_recent_archivers(recent);
        plot.add_pvs(names);
        Self {
            id,
            plot,
            place: Geometry::default(),
            geometry: Geometry::default(),
        }
    }

    /// The PVs' names, as added.
    #[cfg(test)]
    pub fn names(&self) -> Vec<String> {
        self.plot.names()
    }

    /// What it shows, for a layout: its PVs, time range and value axes,
    /// but not the archiver, which is bobview's.
    pub fn view(&self) -> View {
        View {
            archiver: None,
            ..self.plot.view()
        }
    }

    /// Show `view` (from a layout), keeping bobview's archiver.
    pub fn set_view(&mut self, view: &View) {
        self.plot.set_view(&View {
            archiver: None,
            ..view.clone()
        });
    }

    /// Add the PVs `names` that it doesn't have yet.
    pub fn extend(&mut self, names: &[String]) {
        self.plot.add_pvs(names);
    }

    /// The archiver it fetches history from.
    pub fn archiver(&self) -> Option<&ArchiverClient> {
        self.plot.archiver()
    }

    /// Fetch history from `archiver` (or none), unless it does already.
    pub fn set_archiver(&mut self, archiver: Option<&ArchiverClient>, recent: &[String]) {
        if self.plot.archiver() != archiver {
            self.plot.set_archiver(archiver.cloned());
        }
        self.plot.set_recent_archivers(recent);
    }

    /// Whether the user switched archivers in its pill since last asked:
    /// [`Trend::archiver`] is the new one.
    pub fn take_archiver_change(&mut self) -> bool {
        self.plot.take_archiver_change()
    }

    /// Whether its plot is still fetching history, or waiting to draw
    /// a PV whose data has come (snapshots wait for this).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_loading(&self) -> bool {
        self.plot.is_loading()
    }

    /// The window title: its PVs, or the first and how many more.
    pub fn title(&self) -> String {
        match self.plot.title() {
            t if t.is_empty() => "Trend".into(),
            t => format!("Trend: {t}"),
        }
    }

    /// Draw it in the space `ui` offers, and no more: in a window inside
    /// the main one (an auto-sized `egui::Window`) the plot's legend
    /// would otherwise push the window taller every frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        self.plot.ui(&mut inner);
        ui.advance_cursor_after_rect(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn titles_name_the_pvs() {
        let pvs = PvManager::new();
        let t = Trend::new(0, &names(&["A:B", "ca://A:B", " "]), &pvs, None, &[]);
        assert_eq!(t.names().len(), 1, "each PV once");
        assert_eq!(t.title(), "Trend: A:B");
        let t = Trend::new(1, &names(&["A", "B", "C"]), &pvs, None, &[]);
        assert_eq!(t.title(), "Trend: A and 2 more");
        assert_eq!(Trend::new(2, &[], &pvs, None, &[]).title(), "Trend");
    }

    #[test]
    fn extending_adds_new_pvs_after_the_others() {
        let pvs = PvManager::new();
        let mut t = Trend::new(0, &names(&["A"]), &pvs, None, &[]);
        t.extend(&names(&["ca://A", "B", "B", " ", "C"]));
        // Each PV once, in the order added, so the new ones take the next
        // colours as in a new trend of all three.
        assert_eq!(t.names(), ["A", "B", "C"]);
        assert_eq!(t.title(), "Trend: A and 2 more");
    }

    /// The longest line the trend draws, in points, and how wide it is,
    /// once it has stopped loading: drawn headless, as a snapshot is.
    fn longest_line(archiver: Option<ArchiverClient>) -> (usize, f32) {
        use std::time::{Duration, Instant};
        let pvs = PvManager::new();
        // Archived by the mock, and served by no IOC: what it draws is
        // history, not a live tail.
        let mut t = Trend::new(0, &names(&["MOCK:SINE"]), &pvs, archiver, &[]);
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 500.0));
        let begin = Instant::now();
        let mut frames = 0;
        let shapes = loop {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(begin.elapsed().as_secs_f64()),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| t.ui(ui));
            out.textures_delta.clear();
            frames += 1;
            // A few frames after it has loaded, for the data to be drawn.
            if frames > 3 && !t.is_loading() {
                break out.shapes;
            }
            assert!(
                begin.elapsed() < Duration::from_secs(30),
                "the trend was still loading after 30s"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        fn walk(shape: &egui::Shape, best: &mut (usize, f32)) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, best)),
                egui::Shape::Path(p) if p.points.len() > best.0 => {
                    let (lo, hi) = p.points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| {
                        (lo.min(q.x), hi.max(q.x))
                    });
                    *best = (p.points.len(), hi - lo);
                }
                _ => {}
            }
        }
        let mut best = (0, 0.0);
        shapes.iter().for_each(|s| walk(&s.shape, &mut best));
        best
    }

    #[test]
    fn shows_history_from_the_archiver() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let archiver = ArchiverClient::new(&mock.url()).unwrap();
        // MOCK:SINE has a 10 minute period, sampled every second: the
        // default hour of it is a long wavy line across the plot.
        let (points, width) = longest_line(Some(archiver));
        assert!(
            points > 100,
            "history drawn with {points} points (is http_proxy sending 127.0.0.1 to a \
             proxy? set no_proxy=127.0.0.1,localhost)"
        );
        assert!(width > 400.0, "history drawn {width} points wide");
        // Without the archiver the longest line is the plot's furniture
        // (an icon), so the line above is history.
        let (points, _) = longest_line(None);
        assert!(points < 50, "{points} points with no archiver");
    }

    #[test]
    fn views_leave_the_archiver_to_bobview() {
        let pvs = PvManager::new();
        let ours = ArchiverClient::new("http://ours:17668").unwrap();
        let mut t = Trend::new(0, &names(&["A"]), &pvs, Some(ours.clone()), &[]);
        assert_eq!(t.view().archiver, None, "not saved in layouts");
        t.set_view(&View {
            archiver: Some("http://theirs:17668/retrieval".into()),
            ..t.view()
        });
        assert_eq!(t.archiver(), Some(&ours));
        // Set again, it keeps the one it has (no refetch).
        t.set_archiver(Some(&ours), &[]);
        assert_eq!(t.archiver(), Some(&ours));
        t.set_archiver(None, &[]);
        assert_eq!(t.archiver(), None);
    }
}
