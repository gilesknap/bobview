//! The navigator: a tree, beside the main window's display, of the displays
//! it links to, the displays those link to, and so on.
//!
//! A display's links are its widgets' Open display actions, and the command
//! actions that may run a launcher script, in groups, tabs and embedded
//! displays too. They are read from the display file without building the
//! display, so no PVs are connected, on threads, when the display's node is
//! first shown: a branch is read as its parent is opened, one level ahead,
//! which shows which nodes have links of their own. A command's display is
//! found by running its script, so only when its node is opened or clicked.
//! A display that links back to one above it is shown, marked ↻, but not
//! opened again.

use std::collections::{BTreeMap, HashMap, VecDeque};

use bob::{Action, Macros, Source, Widget};
use eframe::egui;

use crate::app::Location;
use crate::screen::{MAX_DEPTH, expand_for, find_group, tab_elements};

/// What a link opens.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Display(Location),
    /// A command action: the display its launcher script opens, if any.
    Command {
        base: Option<Source>,
        directory: Option<Source>,
        command: String,
    },
}

/// A link of a display: what its widget says, and what it opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub label: String,
    pub target: Target,
    /// The label doesn't tell the link apart: it is the file name or the
    /// command, or another link's label too. The node is then called by its
    /// display's `Links::name` once that has been read.
    pub vague: bool,
}

/// A display's title and the links on it, each target once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Links {
    pub title: String,
    /// What tells this display apart from others of the same file: the
    /// parts of its title that macros fill in (a device name), else the
    /// title.
    pub name: String,
    pub links: Vec<Link>,
}

/// Read the display at `loc` for its links.
pub fn links(loc: &Location) -> Result<Links, bob::Error> {
    let display = crate::fetch::load(&loc.source)?;
    let macros = display.macros.with(&loc.macros);
    let mut all = Vec::new();
    walk(
        &display.widgets,
        &macros,
        display.source.as_ref(),
        0,
        &mut all,
    );
    let mut links: Vec<Link> = Vec::new();
    for link in all {
        let key = target_key(&link.target);
        if !links.iter().any(|l| target_key(&l.target) == key) {
            links.push(link);
        }
    }
    for i in 0..links.len() {
        let label = &links[i].label;
        let repeated = links.iter().filter(|l| l.label == *label).count() > 1;
        let named_after = match &links[i].target {
            Target::Display(loc) => *label == file_name(&loc.source),
            Target::Command { command, .. } => label.contains(command.trim()),
        };
        links[i].vague = repeated || named_after;
    }
    let title = crate::screen::title(&display, &macros);
    Ok(Links {
        name: name(&display.name, &macros).unwrap_or_else(|| title.clone()),
        title,
        links,
    })
}

/// The words of a display's `name` that macros fill in, expanded: a
/// device name, as in `Ion Pump - $(device)`; None if there are none.
fn name(name: &str, macros: &Macros) -> Option<String> {
    let words: Vec<String> = name
        .split_whitespace()
        .filter(|w| w.contains("$(") || w.contains("${"))
        .map(|w| bob::expand(w, &|n| macros.get(n).map(str::to_string)))
        .filter(|w| !w.contains("$(") && !w.contains("${") && !w.trim().is_empty())
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Add the links of `widgets`, in a display at `base`, to `out`.
fn walk(
    widgets: &[Widget],
    parent: &Macros,
    base: Option<&Source>,
    depth: u32,
    out: &mut Vec<Link>,
) {
    for w in widgets {
        let macros = parent.with(&w.macros());
        let actions = w.actions();
        for action in &actions {
            if let Some(target) = target(w, &macros, base, action) {
                let label = label(w, &macros, action, actions.len(), &target);
                out.push(Link {
                    label,
                    target,
                    vague: false,
                });
            }
        }
        let prop = |name: &str| expand_for(w, &macros, w.text(name).unwrap_or(""));
        match w.kind.as_str() {
            "group" => walk(&w.children, &macros, base, depth, out),
            "tabs" => {
                for tab in tab_elements(w) {
                    let children: Vec<Widget> = tab
                        .child("children")
                        .map(|c| {
                            c.children
                                .iter()
                                .filter(|e| e.name == "widget")
                                .map(|e| Widget::from_element(e.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    walk(&children, &macros, base, depth, out);
                }
            }
            "embedded" => embed(
                &prop("file"),
                &prop("group_name"),
                &macros,
                base,
                depth,
                out,
            ),
            // Every tab, as each is a display a click away.
            "navtabs" => {
                for tab in tab_elements(w) {
                    let text =
                        |name: &str| expand_for(w, &macros, tab.child_text(name).unwrap_or(""));
                    let own = tab.child("macros").map(Macros::from_element);
                    let macros = macros.with(&own.unwrap_or_default());
                    embed(
                        &text("file"),
                        &text("group_name"),
                        &macros,
                        base,
                        depth,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Add the links of the display (or its group) that a widget embeds.
fn embed(
    file: &str,
    group: &str,
    macros: &Macros,
    base: Option<&Source>,
    depth: u32,
    out: &mut Vec<Link>,
) {
    if file.trim().is_empty() || depth >= MAX_DEPTH {
        return;
    }
    let Ok(display) = crate::fetch::load(&resolve(base, file)) else {
        return;
    };
    let macros = display.macros.with(macros);
    let widgets = if group.trim().is_empty() {
        &display.widgets[..]
    } else {
        find_group(&display.widgets, group).map_or(&[][..], |g| &g.children[..])
    };
    walk(widgets, &macros, display.source.as_ref(), depth + 1, out);
}

/// A file named by a widget of the display at `base`.
fn resolve(base: Option<&Source>, file: &str) -> Source {
    match base {
        Some(base) => base.resolve(file),
        None => Source::parse(file),
    }
}

/// What `action` of widget `w` opens, as clicking it would (see
/// `widgets::run_action`): a display, or a command that may.
fn target(w: &Widget, macros: &Macros, base: Option<&Source>, action: &Action) -> Option<Target> {
    match action {
        Action::OpenDisplay {
            file, macros: own, ..
        } => {
            let own = Macros::from_pairs(
                own.iter()
                    .map(|(k, v)| (k.to_string(), expand_for(w, macros, v))),
            );
            let macros = macros.with(&own);
            let file = expand_for(w, &macros, file);
            (!file.trim().is_empty()).then(|| {
                Target::Display(Location {
                    source: resolve(base, &file),
                    macros,
                })
            })
        }
        // A browser runs no commands.
        Action::Command {
            command, directory, ..
        } if !cfg!(target_arch = "wasm32") => {
            let command = expand_for(w, macros, command);
            // A StripTool launcher opens a trend, not a display.
            if command.trim().is_empty() || bob::strip_tool_pvs(&command).is_some() {
                return None;
            }
            let directory = expand_for(w, macros, directory);
            let directory = (!directory.trim().is_empty()).then(|| resolve(base, &directory));
            Some(Target::Command {
                base: base.cloned(),
                directory,
                command,
            })
        }
        _ => None,
    }
}

/// What a link is called: its widget's text, when that is its only action,
/// else the action's description, else the widget's text, else the file or
/// command.
fn label(w: &Widget, macros: &Macros, action: &Action, actions: usize, target: &Target) -> String {
    let one_line = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let own = ["text", "label"]
        .iter()
        .find_map(|p| w.text(p))
        .filter(|t| !t.contains("$(actions)"))
        .map(|t| one_line(&expand_for(w, macros, t)))
        .filter(|t| !t.is_empty());
    let description =
        Some(one_line(&expand_for(w, macros, action.description()))).filter(|t| !t.is_empty());
    let label = if actions == 1 {
        own.or(description)
    } else {
        description.or(own)
    };
    label.unwrap_or_else(|| match target {
        Target::Display(loc) => file_name(&loc.source),
        Target::Command { command, .. } => one_line(command),
    })
}

/// A display's file name without its directory or extension.
fn file_name(source: &Source) -> String {
    let text = source.to_string();
    let base = text.rsplit('/').next().unwrap_or("");
    base.rsplit_once('.')
        .map_or(base, |(stem, _)| stem)
        .to_string()
}

/// What tells displays apart: the file and the effective macros, as
/// `Location::same_display` compares them.
pub fn key(loc: &Location) -> String {
    let macros: BTreeMap<&str, &str> = loc.macros.iter().collect();
    format!("{}\0{macros:?}", loc.source)
}

/// What tells links' targets apart.
fn target_key(target: &Target) -> String {
    match target {
        Target::Display(loc) => key(loc),
        Target::Command {
            base,
            directory,
            command,
        } => format!("command\0{base:?}\0{directory:?}\0{command}"),
    }
}

/// Reading a display, or finding the display a command opens.
enum Job {
    Display(String, Location),
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "a browser runs no commands")
    )]
    Command(String, Option<Source>, Option<Source>, String),
}

/// A job's result, by its key.
enum Done {
    Display(String, Result<Links, String>),
    Command(String, Option<Location>),
}

/// Displays read at once.
#[cfg(not(target_arch = "wasm32"))]
const THREADS: usize = 4;

/// What clicking a node asks for, and whether ctrl or shift was held.
pub struct Clicked {
    pub target: Target,
    pub ctrl: bool,
    pub shift: bool,
}

/// The navigator's state: whether it is shown, its filter, and the links
/// read so far.
pub struct Navigator {
    pub open: bool,
    filter: String,
    /// Each display's links by `key`, or why they couldn't be read; None
    /// while being read.
    displays: HashMap<String, Option<Result<Links, String>>>,
    /// The display each command opens, by `target_key`; None while its
    /// script runs.
    commands: HashMap<String, Option<Option<Location>>>,
    queue: VecDeque<Job>,
    running: usize,
    done: (
        std::sync::mpsc::Sender<Done>,
        std::sync::mpsc::Receiver<Done>,
    ),
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            open: false,
            filter: String::new(),
            displays: HashMap::new(),
            commands: HashMap::new(),
            queue: VecDeque::new(),
            running: 0,
            done: std::sync::mpsc::channel(),
        }
    }
}

impl Navigator {
    /// The tree of links from `root`, with the displays shown in windows
    /// (`here`) in bold; returns the node clicked, if any.
    pub fn ui(&mut self, ui: &mut egui::Ui, root: &Location, here: &[Location]) -> Option<Clicked> {
        self.poll();
        let here: Vec<String> = here.iter().map(key).collect();
        let mut clicked = None;
        ui.horizontal(|ui| {
            if ui
                .button("⟳")
                .on_hover_text("Read the displays again")
                .clicked()
            {
                self.displays.clear();
                self.commands.clear();
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("🔍 Filter")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.separator();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            if self.filter.trim().is_empty() {
                let target = Target::Display(root.clone());
                let label = match self.displays.get(&key(root)) {
                    Some(Some(Ok(links))) => links.title.clone(),
                    _ => file_name(&root.source),
                };
                let link = Link {
                    label,
                    target,
                    vague: false,
                };
                self.node(ui, &link, &mut Vec::new(), &here, &mut clicked);
            } else {
                self.matches(ui, &here, &mut clicked);
            }
        });
        self.start(ui.ctx());
        clicked
    }

    /// One node and, while it is open, its links.
    fn node(
        &mut self,
        ui: &mut egui::Ui,
        link: &Link,
        above: &mut Vec<String>,
        here: &[String],
        clicked: &mut Option<Clicked>,
    ) {
        // A command's display, once its script has said.
        let (loc, command) = match &link.target {
            Target::Display(loc) => (Some(loc.clone()), None),
            Target::Command { .. } => {
                let k = target_key(&link.target);
                let found = self.commands.get(&k).cloned();
                (found.clone().flatten().flatten(), Some((k, found)))
            }
        };
        let k = loc.as_ref().map(key);
        let cycle = k.as_ref().is_some_and(|k| above.contains(k));
        let read = match (&loc, &k) {
            (Some(loc), Some(k)) if !cycle => self.read(k, loc),
            _ => None,
        };
        let mut text = match &read {
            Some(Some(Ok(links))) if link.vague => links.name.clone(),
            _ => link.label.clone(),
        };
        let mut tip = match &loc {
            Some(loc) => describe(loc),
            None => match &link.target {
                Target::Command { command, .. } => format!("command: {command}"),
                Target::Display(_) => String::new(),
            },
        };
        // Whether it has links to show, or may have.
        let mut branch = false;
        match (&command, &read) {
            (Some((_, None)), _) => branch = true,
            (Some((_, Some(None))), _) => text.push_str(" …"),
            (Some((_, Some(Some(None)))), _) => {
                text = format!("⚠ {text}");
                tip.push_str("\nopens no display");
            }
            _ if cycle => text = format!("↻ {text}"),
            (_, Some(None)) => text.push_str(" …"),
            (_, Some(Some(Err(e)))) => {
                text = format!("⚠ {text}");
                tip = format!("{tip}\n{e}");
            }
            (_, Some(Some(Ok(links)))) => {
                branch = !links.links.is_empty();
                if links.title != text && !links.title.is_empty() {
                    tip = format!("{}\n{tip}", links.title);
                }
            }
            _ => {}
        }
        let shown = k.as_ref().is_some_and(|k| here.contains(k));
        let mut rich = egui::RichText::new(text);
        if shown {
            rich = rich.strong();
        }
        let target = match &loc {
            Some(loc) => Target::Display(loc.clone()),
            None => link.target.clone(),
        };
        let mut row = |ui: &mut egui::Ui| {
            let resp = ui.selectable_label(shown, rich.clone());
            let resp = if tip.is_empty() {
                resp
            } else {
                resp.on_hover_text(&tip)
            };
            if resp.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                *clicked = Some(Clicked {
                    target: target.clone(),
                    ctrl: modifiers.command,
                    shift: modifiers.shift,
                });
            }
        };
        if !branch {
            // Lined up with the nodes that have an arrow.
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().indent);
                row(ui);
            });
            return;
        }
        let id = ui.make_persistent_id(target_key(&link.target));
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            above.is_empty(),
        );
        state.show_header(ui, row).body(|ui| match (&loc, &k) {
            (Some(_), Some(k)) => {
                let Some(Some(Ok(links))) = self.displays.get(k) else {
                    return;
                };
                let links = links.links.clone();
                above.push(k.clone());
                for child in &links {
                    self.node(ui, child, above, here, clicked);
                }
                above.pop();
            }
            // Opened: find the command's display.
            _ => {
                if let (
                    Some((ck, None)),
                    Target::Command {
                        base,
                        directory,
                        command,
                    },
                ) = (&command, &link.target)
                {
                    self.commands.insert(ck.clone(), None);
                    self.queue.push_back(Job::Command(
                        ck.clone(),
                        base.clone(),
                        directory.clone(),
                        command.clone(),
                    ));
                }
                ui.weak("…");
            }
        });
    }

    /// The links read so far whose label or file has the filter in it.
    fn matches(&mut self, ui: &mut egui::Ui, here: &[String], clicked: &mut Option<Clicked>) {
        let filter = self.filter.trim().to_lowercase();
        let mut found: Vec<(String, Link)> = Vec::new();
        for links in self.displays.values().flatten().flatten() {
            for link in &links.links {
                let target = match &link.target {
                    Target::Command { .. } => match self.commands.get(&target_key(&link.target)) {
                        Some(Some(Some(loc))) => Target::Display(loc.clone()),
                        _ => link.target.clone(),
                    },
                    t => t.clone(),
                };
                let file = match &target {
                    Target::Display(loc) => loc.source.to_string(),
                    Target::Command { command, .. } => command.clone(),
                };
                let k = target_key(&target);
                let read = match self.displays.get(&k) {
                    Some(Some(Ok(read))) => Some(read),
                    _ => None,
                };
                let label = match read {
                    Some(read) if link.vague => read.name.clone(),
                    _ => link.label.clone(),
                };
                let title = read.map_or("", |r| r.title.as_str());
                let hit = [label.as_str(), title, &file]
                    .iter()
                    .any(|t| t.to_lowercase().contains(&filter));
                if hit && !found.iter().any(|(fk, _)| *fk == k) {
                    let vague = false;
                    found.push((
                        k,
                        Link {
                            label,
                            target,
                            vague,
                        },
                    ));
                }
            }
        }
        found.sort_by_key(|a| a.1.label.to_lowercase());
        if found.is_empty() {
            ui.weak("No match");
        }
        for (k, link) in &found {
            let tip = match &link.target {
                Target::Display(loc) => describe(loc),
                Target::Command { command, .. } => format!("command: {command}"),
            };
            let resp = ui
                .selectable_label(here.contains(k), &link.label)
                .on_hover_text(tip);
            if resp.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                *clicked = Some(Clicked {
                    target: link.target.clone(),
                    ctrl: modifiers.command,
                    shift: modifiers.shift,
                });
            }
        }
        ui.separator();
        ui.weak("Searches the displays read so far: open branches to read more.");
    }

    /// The links of the display at `loc`, asking for them the first time.
    fn read(&mut self, k: &str, loc: &Location) -> Option<Option<Result<Links, String>>> {
        if !self.displays.contains_key(k) {
            self.displays.insert(k.to_string(), None);
            self.queue
                .push_back(Job::Display(k.to_string(), loc.clone()));
        }
        self.displays.get(k).cloned()
    }

    /// Take in the jobs done.
    fn poll(&mut self) {
        while let Ok(done) = self.done.1.try_recv() {
            self.running = self.running.saturating_sub(1);
            match done {
                Done::Display(k, links) => {
                    self.displays.insert(k, Some(links));
                }
                Done::Command(k, loc) => {
                    self.commands.insert(k, Some(loc));
                }
            }
        }
    }

    /// Start the jobs waiting, a few at a time, on threads.
    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, ctx: &egui::Context) {
        while self.running < THREADS
            && let Some(job) = self.queue.pop_front()
        {
            self.running += 1;
            let (tx, ctx) = (self.done.0.clone(), ctx.clone());
            std::thread::spawn(move || {
                tx.send(run(job)).ok();
                ctx.request_repaint();
            });
        }
    }

    /// Read the displays waiting, a few a frame; one still being fetched
    /// is tried again.
    #[cfg(target_arch = "wasm32")]
    fn start(&mut self, ctx: &egui::Context) {
        for _ in 0..4 {
            let Some(job) = self.queue.pop_front() else {
                return;
            };
            let done = match &job {
                Job::Display(k, loc) => match links(loc) {
                    Err(bob::Error::Fetch(_, why)) if why == crate::fetch::LOADING => {
                        self.queue.push_back(job);
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        return;
                    }
                    links => Done::Display(k.clone(), links.map_err(|e| e.to_string())),
                },
                Job::Command(..) => run(job),
            };
            self.running += 1;
            self.done.0.send(done).ok();
        }
        ctx.request_repaint();
    }
}

/// Do a job.
fn run(job: Job) -> Done {
    match job {
        Job::Display(k, loc) => Done::Display(k, links(&loc).map_err(|e| e.to_string())),
        #[cfg(not(target_arch = "wasm32"))]
        Job::Command(k, base, directory, command) => {
            let loc = crate::launch::command_target(base.as_ref(), directory, &command).map(
                |(file, macros)| Location {
                    source: Source::File(file),
                    macros,
                },
            );
            Done::Command(k, loc)
        }
        #[cfg(target_arch = "wasm32")]
        Job::Command(k, ..) => Done::Command(k, None),
    }
}

/// A display's file and macros, for a node's tooltip.
fn describe(loc: &Location) -> String {
    let macros: Vec<String> = loc
        .macros
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if macros.is_empty() {
        loc.source.to_string()
    } else {
        format!("{}\n{}", loc.source, macros.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, key, links};
    use crate::app::Location;
    use bob::{Macros, Source};

    /// Print the navigator's nodes below the display at $TREE_DUMP (with
    /// macros $TREE_MACROS, as `P=X,R=Y`), to check their labels:
    /// `TREE_DUMP=… cargo test -p bobview print_links -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_links() {
        let path = std::env::var("TREE_DUMP").unwrap();
        let pairs = std::env::var("TREE_MACROS").unwrap_or_default();
        let macros = pairs.split(',').filter_map(|p| p.split_once('='));
        let loc = Location {
            source: Source::File(path.into()),
            macros: Macros::from_pairs(macros),
        };
        let found = links(&loc).unwrap();
        println!("{} ({})", found.title, found.name);
        for l in &found.links {
            match &l.target {
                Target::Display(loc) => {
                    let read = links(loc);
                    let label = match &read {
                        Ok(read) if l.vague => read.name.clone(),
                        _ => l.label.clone(),
                    };
                    let title = read.map_or_else(|e| e.to_string(), |r| r.title);
                    println!("  {label}  [{title}]");
                }
                Target::Command { command, .. } => println!("  {}  [command {command}]", l.label),
            }
        }
    }

    fn button(text: Option<&str>, actions: &str) -> String {
        let text = text.map_or(String::new(), |t| format!("<text>{t}</text>"));
        format!(r#"<widget type="action_button">{text}<actions>{actions}</actions></widget>"#)
    }

    fn open(file: &str, macros: &str, description: &str) -> String {
        format!(
            r#"<action type="open_display"><file>{file}</file><macros>{macros}</macros><description>{description}</description></action>"#
        )
    }

    fn display(name: &str, widgets: &[String]) -> String {
        format!(
            r#"<display version="2.0.0"><name>{name}</name><macros><P>X</P></macros>{}</display>"#,
            widgets.concat()
        )
    }

    #[test]
    fn reads_a_displays_links_as_its_buttons_open_them() {
        let dir = std::env::temp_dir().join(format!("bobview-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let top = display(
            "Top $(P)",
            &[
                // The button's text, with the action's macros.
                button(Some("Go to B"), &open("b.bob", "<R>$(P)1</R>", "Open")),
                // The same display and macros again: one link.
                button(None, &open("b.bob", "<R>X1</R>", "Again")),
                // A menu: each action's description, else the menu's text.
                button(
                    Some("Menu"),
                    &[open("c.bob", "", "First"), open("b.bob", "", "")].concat(),
                ),
                // In a group, and a file named with a macro.
                format!(
                    r#"<widget type="group"><name>g</name>{}</widget>"#,
                    button(None, &open("$(P)/d.bob", "", ""))
                ),
                // An embedded display's links, which may lead back.
                r#"<widget type="embedded"><file>e.bob</file></widget>"#.to_string(),
                button(
                    Some("Launch"),
                    r#"<action type="command"><command>./go</command></action>"#,
                ),
            ],
        );
        std::fs::write(dir.join("top.bob"), top).unwrap();
        std::fs::write(
            dir.join("e.bob"),
            display("E", &[button(Some("Home"), &open("top.bob", "", ""))]),
        )
        .unwrap();

        let at = |file: &str, pairs: &[(&str, &str)]| Location {
            source: Source::File(dir.join(file)),
            macros: Macros::from_pairs(pairs.iter().copied()),
        };
        let found = links(&at("top.bob", &[])).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(found.title, "Top X");
        let shown: Vec<(&str, String)> = found
            .links
            .iter()
            .map(|l| {
                let target = match &l.target {
                    Target::Display(loc) => key(loc),
                    Target::Command { command, .. } => command.clone(),
                };
                (l.label.as_str(), target)
            })
            .collect();
        let expected = vec![
            ("Go to B", key(&at("b.bob", &[("P", "X"), ("R", "X1")]))),
            ("First", key(&at("c.bob", &[("P", "X")]))),
            ("Menu", key(&at("b.bob", &[("P", "X")]))),
            ("d", key(&at("X/d.bob", &[("P", "X")]))),
            ("Home", key(&at("top.bob", &[("P", "X")]))),
            ("Launch", "./go".to_string()),
        ];
        assert_eq!(shown, expected);
        // Called after the file: vague, so named by its display once read.
        let vague: Vec<bool> = found.links.iter().map(|l| l.vague).collect();
        assert_eq!(vague, [false, false, false, true, false, false]);
    }

    #[test]
    fn names_a_display_by_what_macros_fill_in_its_title() {
        use super::name;
        let macros = Macros::from_pairs([("dom", "LB"), ("device", "LB-VA-IONP-04"), ("id", "02")]);
        assert_eq!(
            name("Ion Pump Controller - $(device)", &macros).as_deref(),
            Some("LB-VA-IONP-04")
        );
        assert_eq!(
            name("Gauge - $(dom)-VA-GAUGE-$(id)", &macros).as_deref(),
            Some("LB-VA-GAUGE-02")
        );
        // Nothing filled in, or a macro not set: the title instead.
        assert_eq!(name("Linac Vacuum", &macros), None);
        assert_eq!(name("Space $(nope)", &macros), None);
    }
}
