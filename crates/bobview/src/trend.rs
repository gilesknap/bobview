//! Trend windows: PVs plotted against time, archived history and live
//! values, by archiveview's [`Plot`]. Opened from a widget's context menu,
//! a BOY "Graph" button, or a Data Browser or StripTool launcher. The
//! plot fetches history from the Archiver Appliance bobview was given
//! (`--archiver`), or is live only without one.
//!
//! A middle-click on the plot adds the PVs the primary selection names,
//! and one in its search box pastes the selection there: the plot reports
//! the click, `BobApp` reads the selection (on a thread, see
//! [`crate::selection::Reader`]) and hands the text back to
//! [`Trend::selected`].

use archiveview::plot::MiddleClick;
use archiveview::{ArchiverClient, Plot, View};
use eframe::egui;
use pv::PvManager;
use web_time::{Duration, Instant};

use crate::layout::Geometry;
use crate::stripchart::MIN_SCROLL_REPAINT;

/// A trend window's plot, and where its window is.
pub struct Trend {
    /// Tells this window from the others.
    pub id: u64,
    plot: Plot,
    /// Where to open the window, when a layout says.
    pub place: Geometry,
    /// Where the window is, for saving a layout.
    pub geometry: Geometry,
    /// The middle-click waiting for the primary selection: its number,
    /// and when it was (not egui time, which each native window counts
    /// from its own start).
    reading: Option<(u64, Instant)>,
    /// What a paste did, floating over the bottom of the plot for a while.
    notice: Option<Notice>,
    /// Text selected in the search box, and when, until it has been
    /// steady for [`PRIMARY_DEBOUNCE`] to become the primary selection.
    search_selection: Option<(String, Instant)>,
    /// The floor on the plot's own repaints, as last set: see
    /// [`Trend::ui`].
    min_repaint: Duration,
}

/// Text selected in the search box becomes the primary selection once it
/// has stayed the same this long, as in archiveview: not at every
/// shift+arrow, which over `ssh -X` costs a round trip each.
const PRIMARY_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long a middle-click waits for the primary selection, as in
/// archiveview: long enough for a slow owner (or `ssh -X`), short enough
/// that a hung one (arboard waits up to 4 s per format) gets a "Could not
/// read the selection" while the click is still fresh, rather than PVs
/// turning up long after.
pub const SELECTION_WAIT: Duration = Duration::from_secs(2);

/// A short message over the plot.
struct Notice {
    text: String,
    error: bool,
    /// When it goes (egui time), from when it is first drawn.
    until: Option<f64>,
    secs: f64,
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
            reading: None,
            notice: None,
            search_selection: None,
            min_repaint: Duration::ZERO,
        }
    }

    /// Whether a middle-click is waiting for the primary selection.
    #[cfg(test)]
    pub fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// The notice's text, and whether it is an error.
    #[cfg(test)]
    pub fn notice(&self) -> Option<(&str, bool)> {
        self.notice.as_ref().map(|n| (n.text.as_str(), n.error))
    }

    /// The floor on the plot's own repaints, as last drawn.
    #[cfg(test)]
    pub fn min_repaint(&self) -> Duration {
        self.min_repaint
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

    /// Where a middle-click on it since last asked landed, if there was
    /// one: the caller reads the primary selection for it.
    pub fn take_middle_click(&mut self) -> Option<MiddleClick> {
        self.plot.take_middle_click()
    }

    /// Text selected in its search box that has been steady since before
    /// `now` less [`PRIMARY_DEBOUNCE`], once, to make the primary
    /// selection; and when to ask again, while some is waiting.
    pub fn take_search_selection(&mut self, now: Instant) -> (Option<String>, Option<Duration>) {
        if let Some(text) = self.plot.take_search_selection() {
            self.search_selection = Some((text, now));
        }
        match &self.search_selection {
            Some((_, since)) if now.duration_since(*since) >= PRIMARY_DEBOUNCE => {
                (self.search_selection.take().map(|(t, _)| t), None)
            }
            Some((_, since)) => (None, Some(PRIMARY_DEBOUNCE - now.duration_since(*since))),
            None => (None, None),
        }
    }

    /// The middle-click numbered `n`, at `now`, is waiting for the
    /// selection (an earlier one's answer is then ignored).
    pub fn reading(&mut self, n: u64, now: Instant) {
        self.reading = Some((n, now));
    }

    /// The primary selection's `text` for the middle-click numbered `n` on
    /// `click`: add the PVs it names, or paste it into the search box. An
    /// answer for an older click, or one that came too late, is dropped.
    pub fn selected(&mut self, n: u64, click: MiddleClick, text: Option<String>) {
        if self.reading.is_none_or(|(r, _)| r != n) {
            return;
        }
        self.reading = None;
        let text = text.unwrap_or_default();
        match click {
            // Says what it added in `ui`, from `take_pasted`.
            MiddleClick::Plot => {
                self.plot.paste_pvs(&text);
            }
            MiddleClick::Search => self.plot.paste_into_search(&text),
        }
    }

    /// Give up on a middle-click that has waited [`SELECTION_WAIT`] by
    /// `now` for the selection (its owner may have hung), and say so;
    /// whether it did.
    pub fn time_out_reading(&mut self, now: Instant) -> bool {
        let late = self
            .reading
            .is_some_and(|(_, since)| now.duration_since(since) >= SELECTION_WAIT);
        if late {
            self.reading = None;
            self.notify("Could not read the selection", true, 4.0);
        }
        late
    }

    fn notify(&mut self, text: impl Into<String>, error: bool, secs: f64) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            until: None,
            secs,
        });
    }

    /// Draw it in the space `ui` offers, and no more: in a window inside
    /// the main one (an auto-sized `egui::Window`) the plot's legend
    /// would otherwise push the window taller every frame.
    ///
    /// Where windows are embedded (the browser, snapshots) the plot is
    /// in the root viewport, so each repaint it schedules itself redraws
    /// the whole display: those are held to a strip chart's
    /// [`MIN_SCROLL_REPAINT`] there. In its own native window it keeps
    /// its own pace. Set as it is drawn, so it holds for every trend
    /// however it was opened, and whenever the embedding was decided.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let min_repaint = if ui.ctx().embed_viewports() {
            Duration::from_secs_f64(MIN_SCROLL_REPAINT)
        } else {
            Duration::ZERO
        };
        if min_repaint != self.min_repaint {
            self.min_repaint = min_repaint;
            self.plot.set_min_repaint_interval(min_repaint);
        }
        let rect = ui.available_rect_before_wrap();
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        self.plot.ui(&mut inner);
        // A paste: a middle-click's, or Ctrl+V's.
        if let Some(pasted) = self.plot.take_pasted() {
            let error = pasted.added.is_empty() && pasted.present.is_empty();
            self.notify(pasted.message(), error, 3.0);
        }
        self.toast(ui, rect);
        ui.advance_cursor_after_rect(rect);
    }

    /// The notice, over the bottom middle of `rect`, until it expires.
    fn toast(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some(notice) = &mut self.notice else {
            return;
        };
        let now = ui.input(|i| i.time);
        let until = *notice.until.get_or_insert(now + notice.secs);
        if now > until {
            self.notice = None;
            return;
        }
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64(until - now));
        let color = if notice.error {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().text_color()
        };
        let text = egui::RichText::new(&notice.text).color(color);
        // In the trend's own layer, over the plot drawn before it: a trend
        // window behind another (where windows are embedded) keeps its
        // notice behind too.
        let layout = egui::Layout::bottom_up(egui::Align::Center);
        let mut over = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
        over.add_space(56.0);
        egui::Frame::popup(over.style())
            .corner_radius(16.0)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(&mut over, |ui| ui.label(text));
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
    fn the_selection_answers_only_the_latest_middle_click_in_time() {
        let pvs = PvManager::new();
        let mut t = Trend::new(0, &names(&["A"]), &pvs, None, &[]);
        let start = Instant::now();
        t.reading(1, start);
        t.reading(2, start);
        // An earlier click's answer is dropped.
        t.selected(1, MiddleClick::Plot, Some("OLD:PV".into()));
        assert_eq!(t.names(), ["A"]);
        assert!(t.is_reading());
        t.selected(2, MiddleClick::Plot, Some("B:C".into()));
        assert_eq!(t.names(), ["A", "B:C"]);
        assert!(!t.is_reading());
        // Too late: given up on, with a notice, and the answer dropped.
        t.reading(3, start);
        assert!(!t.time_out_reading(start + Duration::from_millis(500)));
        assert!(t.time_out_reading(start + SELECTION_WAIT));
        let (text, error) = t.notice().unwrap();
        assert!(error && text.contains("Could not read"), "{text}");
        t.selected(3, MiddleClick::Plot, Some("LATE:PV".into()));
        assert_eq!(t.names(), ["A", "B:C"]);
        // Into the search box: pasted there, not added.
        t.reading(4, start);
        t.selected(4, MiddleClick::Search, Some("D:E".into()));
        assert_eq!(t.names(), ["A", "B:C"]);
    }

    /// Draw `t` for a pass of `ctx`.
    fn draw(ctx: &egui::Context, t: &mut Trend) {
        let mut out = ctx.run_ui(Default::default(), |ui| t.ui(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn embedded_trends_hold_their_repaints_to_the_strip_charts() {
        let pvs = PvManager::new();
        let mut t = Trend::new(0, &names(&["A"]), &pvs, None, &[]);
        assert_eq!(t.min_repaint(), Duration::ZERO);
        // In the root viewport (the browser, snapshots).
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        draw(&ctx, &mut t);
        assert_eq!(t.min_repaint().as_secs_f64(), MIN_SCROLL_REPAINT);
        // In a native window of its own: its own pace.
        ctx.set_embed_viewports(false);
        draw(&ctx, &mut t);
        assert_eq!(t.min_repaint(), Duration::ZERO);
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
