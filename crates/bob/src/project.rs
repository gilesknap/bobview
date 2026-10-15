//! Eclipse project linked folders, as CS-Studio BOY workspaces use them.
//!
//! BOY screens name each other through the workspace project: a `.project`
//! file links folder names to real directories, so `../mks937a/gauge.opi`
//! from `proj/vacuum/x.opi` means the `mks937a` link of `proj`. A path
//! through such a project is kept as written (so `..` keeps working across
//! links), and [`locate`] finds the real file when it is opened.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// A project's linked folders: name and real directory.
type Links = Vec<(String, PathBuf)>;

/// Folder links of each `.project` read so far, by project directory.
static PROJECTS: LazyLock<Mutex<HashMap<PathBuf, Links>>> = LazyLock::new(Default::default);

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
    let real = std::fs::canonicalize(real).unwrap_or_else(|_| real.to_path_buf());
    links(project_dir)?.iter().find_map(|(name, target)| {
        let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.clone());
        let rest = real.strip_prefix(&target).ok()?;
        Some(project_dir.join(name).join(rest))
    })
}

/// Linked folders of `dir`'s project, by name, when `dir` holds a `.project`.
pub fn links(dir: &Path) -> Option<Links> {
    let mut projects = PROJECTS.lock().unwrap();
    if let Some(links) = projects.get(dir) {
        return Some(links.clone());
    }
    let text = std::fs::read_to_string(dir.join(".project")).ok()?;
    let links = parse(&text);
    projects.insert(dir.to_path_buf(), links.clone());
    Some(links)
}

/// `<link><name>…</name><location>…</location></link>` entries of a
/// `.project` (or `<locationURI>file:…`).
fn parse(text: &str) -> Links {
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
            let location = child("location").or_else(|| {
                let uri = child("locationURI")?;
                Some(
                    uri.strip_prefix("file://")
                        .or(uri.strip_prefix("file:"))
                        .unwrap_or(uri),
                )
            })?;
            Some((name.to_string(), PathBuf::from(location)))
        })
        .collect()
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
    fn normalizes_lexically() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("../x")), PathBuf::from("../x"));
    }
}
