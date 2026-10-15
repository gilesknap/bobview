//! DLS CS-Studio launcher scripts, run by `.opi` command actions.
//!
//! DLS BOY screens open many others (such as the storage ring cell screens)
//! with a command like `./srCellVacuum 04`: a shell script that works out a
//! display and its macros and ends with `runcss.sh DISPLAY -m MACROS`, which
//! starts CS-Studio. bobview runs a copy of such a script next to a stand-in
//! `runcss.sh` that only records its arguments, and opens that display
//! itself. Other commands are not run.
//!
//! Nothing is run for a display fetched over HTTP, nor any script that does
//! not lie beside or under the display or in a linked folder of its Eclipse
//! project (after macros and `..` are resolved).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bob::{Macros, Source};

/// Longest a launcher script may take to name its display.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Longest [`defaults`] spends trying launchers before giving up.
const SEARCH_TIME: Duration = Duration::from_secs(10);

/// The stand-in for `runcss.sh`: writes its arguments, NUL separated.
const STUB: &str = "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$(dirname \"$0\")/args\"\n";

/// The display and macros that a command action of the display at `base`
/// opens, run in `directory` (the display's own when `None`); `None` for a
/// display that is not a local file, a directory or script outside the
/// display's own folders, or a command that is not a launcher script.
pub fn command_target(
    base: Option<&Source>,
    directory: Option<Source>,
    command: &str,
) -> Option<(PathBuf, Macros)> {
    let Some(Source::File(display)) = base else {
        log::warn!("not running '{command}': only a local display may run commands");
        return None;
    };
    let display_dir = parent(display);
    let dir = match directory {
        None => display_dir.clone(),
        Some(Source::File(d)) if allowed(&display_dir, &d) => d,
        Some(d) => {
            log::warn!("not running '{command}' in {d}: outside the display's folders");
            return None;
        }
    };
    target(&display_dir, &dir, command)
}

/// The display (a path) and macros that `command`, run in `dir` for a
/// display in `display_dir`, would open CS-Studio with; `None` if it is not
/// a launcher script in the display's folders (see [`allowed`]).
fn target(display_dir: &Path, dir: &Path, command: &str) -> Option<(PathBuf, Macros)> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let (program, args) = words.split_first()?;
    // Only a script named by a path, as `./srCellVacuum`, not one on $PATH.
    if !program.contains('/') {
        return None;
    }
    let script = dir.join(program);
    if !allowed(display_dir, &script) {
        log::warn!("not running '{command}': outside the display's folders");
        return None;
    }
    let text = std::fs::read(bob::project::locate(&script)).ok()?;
    if text.len() > 1 << 20 || !text.starts_with(b"#!") || !contains(&text, b"runcss.sh") {
        return None;
    }
    let args = run(&text, args)?;
    let (display, rest) = args.split_first()?;
    let macros = rest
        .windows(2)
        .find(|w| w[0] == "-m")
        .map(|w| parse_macros(&w[1]))
        .unwrap_or_default();
    let dir = script.parent().unwrap_or(dir);
    Some((without_cur_dir(&dir.join(display)), macros))
}

/// Whether `path` (a file or directory, possibly through project links) is
/// a real local path beside or under `display_dir`, or under a linked folder
/// of the Eclipse project the display lies in.
fn allowed(display_dir: &Path, path: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(bob::project::locate(path)) else {
        return false;
    };
    roots(display_dir).iter().any(|root| real.starts_with(root))
}

/// The real directories a display in `display_dir` may run scripts from:
/// its own, and the linked folders of the nearest `.project` above it (as
/// written, through links, and as found on disk).
fn roots(display_dir: &Path) -> Vec<PathBuf> {
    let dir = std::path::absolute(display_dir).unwrap_or_else(|_| display_dir.to_path_buf());
    let dir = bob::project::normalize(&dir);
    let Ok(real) = std::fs::canonicalize(bob::project::locate(&dir)) else {
        return Vec::new();
    };
    let mut roots = vec![real.clone()];
    for start in [&dir, &real] {
        if let Some(links) = start.ancestors().find_map(bob::project::links) {
            roots.extend(
                links
                    .iter()
                    .filter_map(|(_, target)| std::fs::canonicalize(target).ok()),
            );
        }
    }
    roots
}

/// The directory holding `file`, `.` for a bare file name.
fn parent(file: &Path) -> PathBuf {
    match file.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `path` without its `.` components.
fn without_cur_dir(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| *c != std::path::Component::CurDir)
        .collect()
}

/// The macros a launcher script beside the `.opi` at `file` would open it
/// with, for a display opened directly (as `linacVacuum.opi`, which needs
/// `linacVacuum.sh`'s `dom=LI`). Tries each launcher in its directory, the
/// one named `<stem>.sh` first, with no arguments and with each number in
/// the file name, bare and with an `r` (`srCellVacuum 05`, `05r` for
/// `SR05C_rack.opi`), and takes the first that opens `file`.
pub fn defaults(file: &Path) -> Option<Macros> {
    let file = without_cur_dir(file);
    let dir = parent(&file);
    let name = file.file_name()?.to_str()?;
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut scripts: Vec<String> = std::fs::read_dir(bob::project::locate(&dir))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| (n.ends_with(".sh") || !n.contains('.')) && n != "runcss.sh")
        .collect();
    scripts.sort_by_key(|n| (*n != format!("{stem}.sh"), n.clone()));
    let mut args = vec![String::new()];
    for n in numbers(stem) {
        args.push(format!("{n}r"));
        args.push(n);
    }
    let start = Instant::now();
    for script in scripts.iter().take(20) {
        for arg in &args {
            if start.elapsed() > SEARCH_TIME {
                log::warn!("gave up looking for the launcher of {}", file.display());
                return None;
            }
            let command = format!("./{script} {arg}");
            log::debug!("trying {command}");
            if let Some((target, macros)) = target(&dir, &dir, &command)
                && without_cur_dir(&target) == file
            {
                log::info!("macros from {command}: {macros:?}");
                return Some(macros);
            }
        }
    }
    None
}

/// The runs of digits in `text`, with a `-N` suffix kept (`02-1`).
fn numbers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars().chain([' ']) {
        if c.is_ascii_digit() || (c == '-' && !cur.is_empty() && !cur.contains('-')) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(cur.trim_end_matches('-').to_string());
            cur.clear();
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Run a copy of `script` with `args` beside the stand-in `runcss.sh`, and
/// return the arguments it passed that.
fn run(script: &[u8], args: &[&str]) -> Option<Vec<String>> {
    let dir = private_dir()?;
    let result = (|| {
        let launcher = dir.join("launcher");
        write_executable(&launcher, script)?;
        write_executable(&dir.join("runcss.sh"), STUB.as_bytes())?;
        let mut child = Command::new(&launcher)
            .args(args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let start = Instant::now();
        loop {
            match child.try_wait().ok()? {
                Some(_) => break,
                None if start.elapsed() > TIMEOUT => {
                    // Not waited for: a process stuck on a hung file system
                    // ignores even SIGKILL.
                    let _ = child.kill();
                    log::warn!("launcher script took too long");
                    return None;
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        let out = std::fs::read(dir.join("args")).ok()?;
        Some(
            out.split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect(),
        )
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// A new directory in the temp dir that only this user can use: made here
/// (never one that already exists, which another user could have planted)
/// with mode 0700, under a name nobody can predict.
fn private_dir() -> Option<PathBuf> {
    use std::hash::{BuildHasher, RandomState};
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..16 {
        let n = RandomState::new().hash_one((std::process::id(), Instant::now()));
        let dir = std::env::temp_dir().join(format!("bobview-launch-{n:016x}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Some(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                log::warn!("cannot make a directory to run a launcher in: {e}");
                return None;
            }
        }
    }
    None
}

/// Write a new executable file at `path`; fails if it already exists.
fn write_executable(path: &Path, data: &[u8]) -> Option<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .ok()?
        .write_all(data)
        .ok()
}

/// `a=1,b=2` as macros.
fn parse_macros(text: &str) -> Macros {
    Macros::from_pairs(text.split(',').filter_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        Some((k.trim().to_string(), v.trim().to_string()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_file_names() {
        assert_eq!(numbers("SR05C_rack"), vec!["05"]);
        assert_eq!(numbers("srDDBACell02-1Vacuum"), vec!["02-1"]);
        assert_eq!(numbers("linacVacuum"), Vec::<String>::new());
    }

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bobview-launch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const LAUNCHER: &str = r#"#!/bin/sh
SRC=$(cd $(dirname "$0"); pwd)
case $1 in
   04 ) MACROS="cell=04,pcell=03"; OPI=srCell04Vacuum.opi ;;
esac
exec "${SRC}/runcss.sh" $OPI -m $MACROS "$@"
"#;

    #[test]
    fn finds_the_display_a_launcher_opens() {
        let dir = tmp("test");
        write_executable(&dir.join("srCellVacuum"), LAUNCHER.as_bytes()).unwrap();
        write_executable(&dir.join("other"), b"#!/bin/sh\necho hi\n").unwrap();
        let display = Source::File(dir.join("x.opi"));
        let target = |c: &str| command_target(Some(&display), None, c);
        let (file, macros) = target("./srCellVacuum 04").unwrap();
        assert_eq!(file, dir.join("srCell04Vacuum.opi"));
        assert_eq!(
            (macros.get("cell"), macros.get("pcell")),
            (Some("04"), Some("03"))
        );
        assert!(target("./other 04").is_none());
        assert!(target("ls").is_none());
        assert!(target("./missing").is_none());
        // A display opened directly gets the macros its launcher gives it.
        let macros = defaults(&dir.join("srCell04Vacuum.opi")).unwrap();
        assert_eq!(macros.get("pcell"), Some("03"));
        assert!(defaults(&dir.join("srCell05Vacuum.opi")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A launcher that would leave `marker` behind if it were run.
    fn marking_launcher(dir: &Path, marker: &Path) {
        let script = format!(
            "#!/bin/sh\ntouch '{}'\nexec \"$(dirname \"$0\")/runcss.sh\" x.opi\n",
            marker.display()
        );
        write_executable(&dir.join("launch"), script.as_bytes()).unwrap();
    }

    #[test]
    fn runs_nothing_for_a_display_from_the_web() {
        let dir = tmp("web");
        let marker = dir.join("ran");
        marking_launcher(&dir, &marker);
        let web = Source::Url("https://example.com/opi/x.opi".into());
        let local_dir = Some(Source::File(dir.clone()));
        let file_url = Some(Source::parse(&format!("file://{}", dir.display())));
        for directory in [None, local_dir, file_url] {
            assert!(command_target(Some(&web), directory.clone(), "./launch").is_none());
            let path = format!("{}/launch", dir.display());
            assert!(command_target(Some(&web), directory, &path).is_none());
        }
        assert!(command_target(None, None, "./launch").is_none());
        assert!(!marker.exists());
        // Nor in a directory on the web, for a local display.
        let display = Source::File(dir.join("x.opi"));
        let web_dir = Some(Source::Url("https://example.com/opi".into()));
        assert!(command_target(Some(&display), web_dir, "./launch").is_none());
        assert!(!marker.exists());
        // The same launcher does run beside the local display.
        assert!(command_target(Some(&display), None, "./launch").is_some());
        assert!(marker.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn runs_only_scripts_in_the_display_folders() {
        let outside = tmp("outside");
        let marker = outside.join("ran");
        marking_launcher(&outside, &marker);
        let proj = tmp("proj");
        std::fs::create_dir_all(proj.join("app/sub")).unwrap();
        let display = Source::File(proj.join("app/x.opi"));
        let run = |dir: Option<Source>, c: &str| command_target(Some(&display), dir, c);
        let escape = format!("../../{}/launch", outside.file_name().unwrap().display());
        let absolute = format!("{}/launch", outside.display());
        assert!(run(None, &absolute).is_none());
        assert!(run(None, &escape).is_none());
        // As a macro such as `$(dir)/launch` would expand.
        let expanded = bob::expand("$(dir)/launch", &|_| Some(outside.display().to_string()));
        assert!(run(None, &expanded).is_none());
        assert!(run(Some(Source::File(outside.clone())), "./launch").is_none());
        assert!(run(Some(Source::parse("/tmp")), "./launch").is_none());
        assert!(!marker.exists());
        // Under the display, or in a linked folder of its project, is fine.
        marking_launcher(&proj.join("app/sub"), &marker);
        assert!(run(None, "./sub/launch").is_some());
        assert!(run(Some(Source::File(proj.join("app/sub"))), "./launch").is_some());
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(
            proj.join(".project"),
            format!(
                "<projectDescription><linkedResources>
                <link><name>other</name><location>{}</location></link>
                </linkedResources></projectDescription>",
                outside.display()
            ),
        )
        .unwrap();
        assert!(run(None, "../other/launch").is_some());
        assert!(marker.exists());
        std::fs::remove_dir_all(&outside).unwrap();
        std::fs::remove_dir_all(&proj).unwrap();
    }
}
