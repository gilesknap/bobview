//! Eclipse project linked folders, as CS-Studio BOY workspaces use them.
//!
//! BOY screens name each other through the workspace project: a `.project`
//! file links folder names to real directories, so `../mks937a/gauge.opi`
//! from `proj/vacuum/x.opi` means the `mks937a` link of `proj`. A path
//! through such a project is kept as written (so `..` keeps working across
//! links), and [`locate`] finds the real file when it is opened.
//!
//! DLS modules have no `.project` on disk: the generated launcher
//! `runcss.sh` beside a module's screens lists the links, and CS-Studio
//! builds the project from them when it starts. Such a launcher is read (not
//! run) as a project of its own, in a virtual directory below the script:
//! `…/opi/opi/runcss.sh/vacuum_4-100` for project `vacuum_4-100`, which
//! cannot clash with a real file since `runcss.sh` is a file, not a folder.
//! See [`launcher_path`].

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// A project's linked folders: name and real directory.
type Links = Vec<(String, PathBuf)>;

/// Folder links of each `.project` read so far, by project directory.
static PROJECTS: LazyLock<Mutex<HashMap<PathBuf, Links>>> = LazyLock::new(Default::default);

/// The DLS CS-Studio launcher that lists a module's links.
pub const LAUNCHER: &str = "runcss.sh";

/// The projects a launcher builds: name and links.
type Projects = Vec<(String, Links)>;

/// Projects of each launcher read so far, by script.
static LAUNCHERS: LazyLock<Mutex<HashMap<PathBuf, Projects>>> = LazyLock::new(Default::default);

/// The real file for `path`: `path` itself when it exists, else the path
/// through the linked folder of the Eclipse project it lies in, else `path`
/// with `.` and `..` removed (for the error message).
pub fn locate(path: &Path) -> PathBuf {
    locate_within(path, MAX_HOPS)
}

/// Most links followed from one project into another, so that projects
/// linking to each other (or to a missing folder of their own) end.
const MAX_HOPS: usize = 8;

fn locate_within(path: &Path, hops: usize) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    let path = normalize(path);
    let mut dir = path.parent();
    while let Some(d) = dir {
        if let Some(links) = links(d)
            && let Ok(rest) = path.strip_prefix(d)
            && let Some(Component::Normal(first)) = rest.components().next()
            && let Some((_, target)) = links.iter().find(|(name, _)| first == name.as_str())
        {
            let below = rest.strip_prefix(first).unwrap_or(Path::new(""));
            let real = target.join(below);
            // A link may point into another project.
            return if real.exists() || hops == 0 {
                real
            } else {
                locate_within(&real, hops - 1)
            };
        }
        dir = d.parent();
    }
    path
}

/// The virtual path of `real` in the project at `project_dir`: the first
/// linked folder holding it, so its `..` names resolve through the project.
pub fn virtual_path(project_dir: &Path, real: &Path) -> Option<PathBuf> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let targets: Vec<(String, PathBuf)> = links(project_dir)?
        .into_iter()
        .map(|(name, target)| (name, canonical(&target)))
        .collect();
    let find = |real: &Path| {
        targets.iter().find_map(|(name, target)| {
            let rest = real.strip_prefix(target).ok()?;
            Some(project_dir.join(name).join(rest))
        })
    };
    // A symlinked screen in a linked folder keeps its own name, as in
    // CS-Studio; one elsewhere (`~/x.opi -> …/opi/x.opi`) goes by where it
    // points.
    let dir = match real.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    real.file_name()
        .and_then(|name| find(&canonical(dir).join(name)))
        .or_else(|| find(&canonical(real)))
}

/// The project that `-p DIR` names: `dir` when its `.project` links
/// folders, else the (first) project of the launcher in `dir` (or `dir`
/// itself, when that is a `runcss.sh`), else `dir` with a `.project` that
/// links nothing.
pub fn project_dir(dir: &Path) -> Option<PathBuf> {
    let own = links(dir);
    if own.as_ref().is_some_and(|l| !l.is_empty()) {
        return Some(dir.to_path_buf());
    }
    let script = if dir.file_name().is_some_and(|n| n == LAUNCHER) {
        dir.to_path_buf()
    } else {
        dir.join(LAUNCHER)
    };
    match launcher_projects(&script).into_iter().next() {
        Some((project, _)) => Some(script.join(project)),
        None => own.map(|_| dir.to_path_buf()),
    }
}

/// How far above a screen its launcher may be: in the screen's own folder
/// (`opi/opi/x.opi`) or up to two above (`opi/opi/common/x.opi`).
const LAUNCHER_DEPTH: usize = 3;

/// The virtual path of the real BOY screen `real` through the projects of
/// the launcher in its folder or up to two above, as CS-Studio would open it
/// from there: `…/opi/opi/x.opi` is
/// `…/opi/opi/runcss.sh/vacuum_4-100/vacuum/x.opi`. `None` for a path that
/// is not a real file, one in no launcher's linked folders, and one in an
/// Eclipse project nearer than the launcher (a `.project` below it, or one
/// beside it that links the file; DLS modules often have an empty one).
pub fn launcher_path(real: &Path) -> Option<PathBuf> {
    if !real.is_file() {
        return None;
    }
    let abs = normalize(&std::path::absolute(real).ok()?);
    for dir in abs.ancestors().skip(1).take(LAUNCHER_DEPTH) {
        let project = dir.join(".project").is_file();
        let script = dir.join(LAUNCHER);
        if script.is_file() {
            if project && virtual_path(dir, real).is_some() {
                return None;
            }
            return launcher_projects(&script)
                .iter()
                .find_map(|(project, _)| virtual_path(&script.join(project), real));
        }
        if project {
            return None;
        }
    }
    None
}

/// Whether `dir` is a project: it holds a `.project` (which is not read),
/// or is one of a launcher's.
pub fn is_project(dir: &Path) -> bool {
    match launcher_of(dir) {
        Some(_) => links(dir).is_some(),
        None => dir.join(".project").is_file(),
    }
}

/// Whether `path` goes through a launcher's project.
pub fn through_launcher(path: &Path) -> bool {
    path.ancestors().any(|d| launcher_of(d).is_some())
}

/// The launcher whose project `dir` would be (`…/runcss.sh/PROJECT`).
fn launcher_of(dir: &Path) -> Option<&Path> {
    dir.parent()
        .filter(|s| s.file_name() == Some(LAUNCHER.as_ref()))
}

/// Forget the projects read so far, so that changed `.project` and
/// `runcss.sh` files are read again.
pub fn forget() {
    PROJECTS.lock().unwrap().clear();
    LAUNCHERS.lock().unwrap().clear();
}

/// Linked folders of `dir`'s project, by name, when `dir` holds a `.project`
/// or is a project of a launcher (`…/runcss.sh/PROJECT`).
pub fn links(dir: &Path) -> Option<Links> {
    if let Some(script) = launcher_of(dir) {
        let project = dir.file_name()?;
        return launcher_projects(script)
            .into_iter()
            .find(|(name, _)| project == name.as_str())
            .map(|(_, links)| links);
    }
    let mut projects = PROJECTS.lock().unwrap();
    if let Some(links) = projects.get(dir) {
        return Some(links.clone());
    }
    let text = std::fs::read_to_string(dir.join(".project")).ok()?;
    let links = parse(&text, dir);
    projects.insert(dir.to_path_buf(), links.clone());
    Some(links)
}

/// The projects that the launcher `script` builds, as [`parse_launcher`]
/// finds them; none when it cannot be read.
fn launcher_projects(script: &Path) -> Projects {
    let mut launchers = LAUNCHERS.lock().unwrap();
    if let Some(projects) = launchers.get(script) {
        return projects.clone();
    }
    // Only a file: `x/runcss.sh/p` for a folder `runcss.sh` is a real path.
    if !script.is_file() {
        return Vec::new();
    }
    let Some(dir) = script.parent() else {
        return Vec::new();
    };
    let projects = std::fs::read_to_string(script)
        .map(|text| parse_launcher(&text, dir))
        .unwrap_or_default();
    launchers.insert(script.to_path_buf(), projects.clone());
    projects
}

/// The linked folders that the launcher `text`, in `dir`, passes CS-Studio
/// as `links="REAL=/PROJECT/NAME,…"`, by project, in order. The script is
/// not run: its plain `name=value` lines are evaluated, with `SCRIPT_DIR`
/// as `dir`, `TOP` as `dir/../../..` and `version` from
/// `$TOP/configure/VERSION` (else `dev`), as the generated scripts set them.
fn parse_launcher(text: &str, dir: &Path) -> Projects {
    let dir = normalize(&std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
    let top = dir.join("../../..");
    let version = std::fs::read_to_string(top.join("configure/VERSION"))
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "dev".into());
    let mut vars: HashMap<String, String> = HashMap::from([
        ("SCRIPT_DIR".into(), dir.display().to_string()),
        ("TOP".into(), top.display().to_string()),
        ("version".into(), version),
    ]);
    let text = text.replace("\\\n", "");
    for line in text.lines() {
        let Some((name, value)) = line.trim().split_once('=') else {
            continue;
        };
        let plain = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        // Command substitutions are not run, and the variables set above
        // keep their values.
        let fixed = ["SCRIPT_DIR", "TOP", "version"].contains(&name);
        if !plain || fixed || value.contains("$(") || value.contains('`') {
            continue;
        }
        let value = match value.strip_prefix('\'') {
            Some(v) => v.strip_suffix('\'').unwrap_or(v).to_string(),
            None => {
                let v = value
                    .strip_prefix('"')
                    .map_or(value, |v| v.strip_suffix('"').unwrap_or(v));
                expand_vars(v, &vars)
            }
        };
        vars.insert(name.to_string(), value);
    }
    let mut projects = Projects::new();
    for entry in vars.get("links").map_or("", String::as_str).split(',') {
        let Some((real, virt)) = entry.trim().split_once('=') else {
            continue;
        };
        let Some((project, name)) = virt.trim_start_matches('/').split_once('/') else {
            continue;
        };
        let name = name.trim_end_matches('/');
        let real = PathBuf::from(real);
        if project.is_empty() || name.is_empty() || name.contains('/') || !real.is_absolute() {
            continue;
        }
        let link = (name.to_string(), real);
        match projects.iter_mut().find(|(p, _)| p == project) {
            Some((_, links)) => links.push(link),
            None => projects.push((project.to_string(), vec![link])),
        }
    }
    projects
}

/// `text` with `$name` and `${name}` replaced by their `vars` (empty when
/// unset), as the shell would in double quotes.
fn expand_vars(text: &str, vars: &HashMap<String, String>) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, next) = match after.strip_prefix('{') {
            Some(braced) => match braced.split_once('}') {
                Some((name, next)) => (name, next),
                None => ("", after),
            },
            None => {
                let end = after.find(|c| !word(c)).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(vars.get(name).map_or("", String::as_str));
        }
        rest = next;
    }
    out.push_str(rest);
    out
}

/// `<link><name>…</name><location>…</location></link>` entries of the
/// `.project` in `dir` (or `<locationURI>`), resolved by [`location`].
fn parse(text: &str, dir: &Path) -> Links {
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("link"))
        .filter_map(|link| {
            let child = |name: &str| {
                link.children()
                    .find(|c| c.has_tag_name(name))
                    .and_then(|c| c.text())
                    .map(str::trim)
            };
            let name = child("name")?;
            let location = child("location").or_else(|| child("locationURI"))?;
            Some((name.to_string(), location_path(location, dir)?))
        })
        .collect()
}

/// The directory a link's location (or location URI) names, for the project
/// in `dir`: an absolute path, a `file:` URI, or one relative to the project
/// as `PROJECT_LOC/x` or `PARENT-2-PROJECT_LOC/x`. `None` for anything else
/// (`virtual:`, other variables or a bare relative path), which would
/// otherwise be taken relative to the working directory.
fn location_path(location: &str, dir: &Path) -> Option<PathBuf> {
    let project_relative = |rest: &str| {
        let (n, rest) = match rest.strip_prefix("PARENT-") {
            Some(r) => {
                let (n, r) = r.split_once("-PROJECT_LOC")?;
                (n.parse::<usize>().ok()?, r)
            }
            None => (0, rest.strip_prefix("PROJECT_LOC")?),
        };
        let rest = rest.strip_prefix('/').or(rest.is_empty().then_some(rest))?;
        // `absolute` keeps `..`, so `../proj` needs normalising before `parent`.
        let mut base = normalize(&std::path::absolute(dir).ok()?);
        for _ in 0..n {
            base = base.parent()?.to_path_buf();
        }
        Some(base.join(percent_decode(rest)))
    };
    let path = if let Some(uri) = location.strip_prefix("file:") {
        // file:/x, file:///x, or file://localhost/x.
        let path = match uri.strip_prefix("//") {
            Some(rest) => &rest[rest.find('/')?..],
            None => uri,
        };
        PathBuf::from(percent_decode(path))
    } else if let Some(path) = project_relative(location) {
        path
    } else {
        PathBuf::from(location)
    };
    path.is_absolute().then_some(path)
}

/// `text` with `%xx` escapes decoded, as in a URI.
fn percent_decode(text: &str) -> std::ffi::OsString {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .filter(|h| bytes[i] == b'%' && h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    #[cfg(unix)]
    return std::os::unix::ffi::OsStringExt::from_vec(out);
    // Elsewhere (Windows, and the browser, which has no files anyway) paths
    // are text.
    #[cfg(not(unix))]
    return String::from_utf8_lossy(&out).into_owned().into();
}

/// `path` with `.` and `..` removed, without looking at the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bob-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_files_through_linked_folders() {
        let real = tmp("real");
        std::fs::write(real.join("x.opi"), "<display/>").unwrap();
        let proj = tmp("proj");
        std::fs::write(
            proj.join(".project"),
            format!(
                "<projectDescription><linkedResources>
                <link><name>a</name><type>2</type><location>{}</location></link>
                <link><name>u</name><type>2</type><locationURI>file:{}</locationURI></link>
                </linkedResources></projectDescription>",
                real.display(),
                real.display()
            ),
        )
        .unwrap();
        let x = real.join("x.opi");
        assert_eq!(locate(&proj.join("a/x.opi")), x);
        assert_eq!(locate(&proj.join("b/../a/x.opi")), x);
        assert_eq!(locate(&proj.join("u/x.opi")), x);
        assert_eq!(locate(&proj.join("a/./sub/../x.opi")), x);
        // Existing files and unknown links are left alone.
        assert_eq!(locate(&x), x);
        assert_eq!(locate(&proj.join("c/../q.opi")), proj.join("q.opi"));
        assert_eq!(virtual_path(&proj, &x), Some(proj.join("a/x.opi")));
        assert_eq!(virtual_path(&proj, Path::new("/elsewhere/y.opi")), None);
    }

    #[test]
    fn link_cycles_end() {
        let proj = tmp("cycle");
        std::fs::write(
            proj.join(".project"),
            format!(
                "<projectDescription><linkedResources>
                <link><name>a</name><location>{}</location></link>
                </linkedResources></projectDescription>",
                proj.join("a").display()
            ),
        )
        .unwrap();
        assert_eq!(locate(&proj.join("a/x.opi")), proj.join("a/x.opi"));
    }

    #[test]
    fn link_locations_resolve_against_the_project() {
        let dir = Path::new("/w/proj");
        let loc = |l: &str| location_path(l, dir);
        assert_eq!(loc("/abs/opi"), Some(PathBuf::from("/abs/opi")));
        assert_eq!(loc("file:/a%20b/opi"), Some(PathBuf::from("/a b/opi")));
        assert_eq!(loc("file:///a/opi"), Some(PathBuf::from("/a/opi")));
        assert_eq!(loc("file://localhost/a"), Some(PathBuf::from("/a")));
        assert_eq!(loc("PROJECT_LOC/sub"), Some(PathBuf::from("/w/proj/sub")));
        assert_eq!(
            loc("PARENT-1-PROJECT_LOC/other%20x"),
            Some(PathBuf::from("/w/other x"))
        );
        // A relative project path with `..` climbs from its normalised form.
        let up = normalize(&std::path::absolute("..").unwrap());
        assert_eq!(
            location_path("PARENT-2-PROJECT_LOC/x", Path::new("../proj")),
            Some(up.parent().unwrap().join("x"))
        );
        for bad in [
            "virtual:/virtual",
            "WORKSPACE_LOC/x",
            "rel/dir",
            "PARENT-9-PROJECT_LOC/x",
        ] {
            assert_eq!(loc(bad), None, "{bad}");
        }
    }

    /// A DLS module's launcher, as generated, with the lines that matter.
    const RUNCSS: &str = r#"#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
TOP=${SCRIPT_DIR}/../../..
CSS_RUN_SCRIPT=$(configure-ioc s -p CSS-gui)
opifile="$1"
if [[ -z $opifile || $opifile == "-h" ]]; then
    exit 1
fi
if [ -e "${TOP}/configure/VERSION" ] ; then
    version="$(cat "${TOP}"/configure/VERSION)"
else
    version="dev"
fi
project=vac_${version}
module=vac
launch_opi=/${project}/${module}/$opifile

links="${SCRIPT_DIR}=${project}/${module},\
GAUGE=/${project}/gauge/,\
/nowhere=/${project}/sub/dir,\
relative/dir=/${project}/rel,\
OTHER=/elsewhere/other"

$CSS_RUN_SCRIPT -o "${launch_opi}" -s -l "$links" "$@"
"#;

    /// A module `top` with its screens (and launcher) in `top/vacApp/opi/opi`,
    /// and a module `gauge` it links to.
    fn module(name: &str, version: Option<&str>) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmp(name);
        let top = root.join("top");
        let opi = top.join("vacApp/opi/opi");
        let gauge = root.join("gauge/opi");
        std::fs::create_dir_all(opi.join("common")).unwrap();
        std::fs::create_dir_all(&gauge).unwrap();
        if let Some(v) = version {
            std::fs::create_dir_all(top.join("configure")).unwrap();
            std::fs::write(top.join("configure/VERSION"), format!("{v}\n")).unwrap();
        }
        let script = RUNCSS
            .replace("GAUGE", &gauge.display().to_string())
            .replace("OTHER", &root.display().to_string());
        std::fs::write(opi.join(LAUNCHER), script).unwrap();
        for f in [
            opi.join("x.opi"),
            opi.join("common/c.opi"),
            gauge.join("g.opi"),
        ] {
            std::fs::write(f, "<display/>").unwrap();
        }
        (root, opi, gauge)
    }

    #[test]
    fn a_launcher_is_a_project() {
        let (root, opi, gauge) = module("runcss", Some("1-2"));
        let proj = opi.join("runcss.sh/vac_1-2");
        let x = proj.join("vac/x.opi");
        assert_eq!(launcher_path(&opi.join("x.opi")), Some(x.clone()));
        assert_eq!(
            launcher_path(&opi.join("common/c.opi")),
            Some(proj.join("vac/common/c.opi"))
        );
        assert_eq!(launcher_path(&gauge.join("g.opi")), None);
        assert_eq!(launcher_path(&x), None);
        // Links between the modules resolve through the virtual project.
        assert_eq!(locate(&x), opi.join("x.opi"));
        assert_eq!(
            locate(&proj.join("vac/../gauge/g.opi")),
            gauge.join("g.opi")
        );
        assert_eq!(locate(&proj.join("vac/common/../x.opi")), opi.join("x.opi"));
        // Only links of this project with a usable name and real folder.
        let names: Vec<String> = links(&proj).unwrap().into_iter().map(|l| l.0).collect();
        assert_eq!(names, ["vac", "gauge"]);
        assert_eq!(
            links(&opi.join("runcss.sh/elsewhere")).unwrap()[0],
            ("other".to_string(), root.clone())
        );
        assert_eq!(links(&opi.join("runcss.sh/vac_dev")), None);
        // `-p` takes the module's folder or its launcher.
        assert_eq!(project_dir(&opi), Some(proj.clone()));
        assert_eq!(project_dir(&opi.join(LAUNCHER)), Some(proj.clone()));
        assert_eq!(project_dir(&gauge), None);
        assert_eq!(
            virtual_path(&proj, &gauge.join("g.opi")),
            Some(proj.join("gauge/g.opi"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_a_generated_launcher() {
        // vacuum 4-100's, verbatim.
        let text = include_str!("testdata/runcss.sh");
        let dir = Path::new("/nowhere/vacuum/4-100/vacuumApp/opi/opi");
        let projects = parse_launcher(text, dir);
        let [(project, links)] = projects.as_slice() else {
            panic!("{projects:?}")
        };
        assert_eq!(project, "vacuum_dev");
        assert_eq!(links.len(), 14);
        assert_eq!(links[0], ("vacuum".to_string(), dir.to_path_buf()));
        let support = Path::new("/dls_sw/prod/R3.14.12.7/support");
        assert_eq!(
            links[2],
            (
                "mks937a".to_string(),
                support.join("mks937a/2-90/mks937aApp/opi/opi")
            )
        );
        assert_eq!(links[13].0, "devIocStats");
    }

    #[test]
    fn only_boy_screens_go_through_a_launcher() {
        let (root, opi, _) = module("runcss-bob", Some("1-2"));
        let boy = r#"<display typeId="org.csstudio.opibuilder.Display" version="1.0.0"/>"#;
        std::fs::write(opi.join("x.opi"), boy).unwrap();
        std::fs::write(opi.join("y.bob"), r#"<display version="2.0.0"/>"#).unwrap();
        let source = |p: &Path| crate::Display::load(p).unwrap().source;
        let x = opi.join("runcss.sh/vac_1-2/vac/x.opi");
        assert_eq!(source(&opi.join("x.opi")), Some(crate::Source::File(x)));
        let y = opi.join("y.bob");
        assert_eq!(source(&y), Some(crate::Source::File(y)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinked_screens_map_by_name_or_by_target() {
        let (root, opi, _) = module("runcss-links", Some("1-2"));
        let proj = opi.join("runcss.sh/vac_1-2");
        // In a linked folder, a link keeps its own name.
        std::os::unix::fs::symlink("x.opi", opi.join("y.opi")).unwrap();
        assert_eq!(
            launcher_path(&opi.join("y.opi")),
            Some(proj.join("vac/y.opi"))
        );
        // Elsewhere, it goes by where it points.
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(opi.join("x.opi"), home.join("z.opi")).unwrap();
        assert_eq!(
            virtual_path(&proj, &home.join("z.opi")),
            Some(proj.join("vac/x.opi"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_project_file_beside_a_launcher_wins_only_with_links() {
        let (root, opi, gauge) = module("runcss-empty", Some("1-2"));
        let proj = opi.join("runcss.sh/vac_1-2");
        // An empty one, as DLS modules often have.
        std::fs::write(opi.join(".project"), "<projectDescription/>").unwrap();
        assert_eq!(
            launcher_path(&opi.join("x.opi")),
            Some(proj.join("vac/x.opi"))
        );
        assert_eq!(project_dir(&opi), Some(proj));
        // One linking the file wins.
        let linking = format!(
            "<projectDescription><linkedResources>
            <link><name>me</name><location>{}</location></link>
            </linkedResources></projectDescription>",
            opi.display()
        );
        std::fs::write(opi.join(".project"), linking).unwrap();
        forget();
        assert_eq!(launcher_path(&opi.join("x.opi")), None);
        assert_eq!(project_dir(&opi), Some(opi.clone()));
        // An empty `.project` and no launcher is still a project.
        std::fs::write(gauge.join(".project"), "<projectDescription/>").unwrap();
        assert_eq!(project_dir(&gauge), Some(gauge.clone()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_launcher_is_near_the_screen() {
        let (root, opi, _) = module("runcss-deep", Some("1-2"));
        let deep = opi.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("d.opi"), "<display/>").unwrap();
        std::fs::write(opi.join("a/b/e.opi"), "<display/>").unwrap();
        assert_eq!(launcher_path(&deep.join("d.opi")), None);
        assert_eq!(
            launcher_path(&opi.join("a/b/e.opi")),
            Some(opi.join("runcss.sh/vac_1-2/vac/a/b/e.opi"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn forget_reads_launchers_again() {
        let (root, opi, _) = module("runcss-forget", Some("1-2"));
        assert!(project_dir(&opi).is_some());
        let text = std::fs::read_to_string(opi.join(LAUNCHER)).unwrap();
        std::fs::write(
            opi.join(LAUNCHER),
            text.replace("project=vac_", "project=new_"),
        )
        .unwrap();
        // (Not checked before `forget`: other tests forget too.)
        forget();
        assert_eq!(project_dir(&opi), Some(opi.join("runcss.sh/new_1-2")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_launcher_without_a_version_is_dev() {
        let (root, opi, _) = module("runcss-dev", None);
        assert_eq!(
            launcher_path(&opi.join("x.opi")),
            Some(opi.join("runcss.sh/vac_dev/vac/x.opi"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_nearer_project_file_wins_over_a_launcher() {
        let (root, opi, _) = module("runcss-project", Some("1-2"));
        std::fs::write(opi.join("common/.project"), "<projectDescription/>").unwrap();
        assert_eq!(launcher_path(&opi.join("common/c.opi")), None);
        // One further away than the launcher does not.
        std::fs::write(root.join(".project"), "<projectDescription/>").unwrap();
        assert!(launcher_path(&opi.join("x.opi")).is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn expands_shell_variables() {
        let vars = HashMap::from([("a".to_string(), "1".to_string())]);
        let e = |t: &str| expand_vars(t, &vars);
        assert_eq!(e("x${a}y$a/$b.$"), "x1y1/.$");
        assert_eq!(e("${a"), "${a");
    }

    #[test]
    fn normalizes_lexically() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("../x")), PathBuf::from("../x"));
    }
}
