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
//! project (after macros and `..` are resolved), nor one that other users may
//! change (writable by others, or in a folder that is).

use std::os::unix::process::CommandExt;
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
    target(&display_dir, &dir, command, Instant::now() + TIMEOUT)
}

/// The display (a path) and macros that `command`, run in `dir` for a
/// display in `display_dir`, would open CS-Studio with; `None` if it is not
/// a launcher script in the display's folders (see [`allowed`]). The script
/// is stopped if it is still running at `deadline`.
fn target(
    display_dir: &Path,
    dir: &Path,
    command: &str,
    deadline: Instant,
) -> Option<(PathBuf, Macros)> {
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
    let real = bob::project::locate(&script);
    if others_may_write(&real) {
        log::warn!("not running '{command}': other users may change it");
        return None;
    }
    let text = std::fs::read(&real).ok()?;
    if text.len() > 1 << 20 || !text.starts_with(b"#!") || !contains(&text, b"runcss.sh") {
        return None;
    }
    log::info!("running launcher '{command}' ({})", real.display());
    let args = run(&text, args, deadline)?;
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

/// Whether users other than the owner may write to `file` or the folder
/// holding it, and so could have planted a launcher there.
fn others_may_write(file: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let writable = |p: &Path| std::fs::metadata(p).map_or(true, |m| m.mode() & 0o002 != 0);
    let dir = file.parent().filter(|d| !d.as_os_str().is_empty());
    writable(file) || writable(dir.unwrap_or(Path::new(".")))
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
///
/// This runs scripts the user did not click, so only for a display in an
/// Eclipse project (a CS-Studio workspace, as DLS screens are opened), not
/// for any `.opi` file lying around.
pub fn defaults(file: &Path) -> Option<Macros> {
    let file = without_cur_dir(file);
    let dir = parent(&file);
    if !in_project(&dir) {
        log::info!(
            "not trying launcher scripts for {}: it is not in an Eclipse project",
            file.display()
        );
        return None;
    }
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
    let end = Instant::now() + SEARCH_TIME;
    for script in scripts.iter().take(20) {
        for arg in &args {
            if Instant::now() >= end {
                log::warn!("gave up looking for the launcher of {}", file.display());
                return None;
            }
            let command = format!("./{script} {arg}");
            let deadline = end.min(Instant::now() + TIMEOUT);
            if let Some((target, macros)) = target(&dir, &dir, &command, deadline)
                && without_cur_dir(&target) == file
            {
                log::info!("macros from {command}: {macros:?}");
                return Some(macros);
            }
        }
    }
    None
}

/// Whether `dir` (as written, or where it really is) is in an Eclipse
/// project: a folder above it has a `.project`, or it is in the project of
/// a DLS launcher (`…/runcss.sh/PROJECT`).
fn in_project(dir: &Path) -> bool {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let dir = bob::project::normalize(&dir);
    let real = std::fs::canonicalize(bob::project::locate(&dir)).ok();
    std::iter::once(dir)
        .chain(real)
        .any(|d| d.ancestors().any(bob::project::is_project))
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
fn run(script: &[u8], args: &[&str], deadline: Instant) -> Option<Vec<String>> {
    // The temp dir is often mounted noexec (as on RHEL hosts), so try places
    // that usually allow running files first.
    for parent in run_dirs() {
        let Some(dir) = private_dir(&parent) else {
            continue;
        };
        let result = run_in(&dir, script, args, deadline);
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                log::debug!("cannot run a launcher in {}: {e}", parent.display());
            }
            Err(e) => {
                log::warn!("cannot run launcher script: {e}");
                return None;
            }
            Ok(args) => return args,
        }
    }
    log::warn!("cannot run launcher script: no folder allows running files");
    None
}

/// Where launcher scripts may be run: `$XDG_RUNTIME_DIR`, the user's cache
/// folder, then the temp dir.
fn run_dirs() -> Vec<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let cache = env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")));
    let mut dirs = vec![];
    dirs.extend(env("XDG_RUNTIME_DIR"));
    if let Some(cache) = cache.map(|c| c.join("bobview"))
        && std::fs::create_dir_all(&cache).is_ok()
    {
        dirs.push(cache);
    }
    dirs.push(std::env::temp_dir());
    dirs
}

/// [`run`] in `dir`: an error if the launcher cannot be started, `Ok(None)`
/// if it did not pass `runcss.sh` anything.
fn run_in(
    dir: &Path,
    script: &[u8],
    args: &[&str],
    deadline: Instant,
) -> std::io::Result<Option<Vec<String>>> {
    // A process forked while a file is open for writing holds it open until
    // it calls exec, and until then the file cannot be run ("text file
    // busy"). So launchers are not started while another's files are being
    // written.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let launcher = dir.join("launcher");
    write_executable(&launcher, script)?;
    write_executable(&dir.join("runcss.sh"), STUB.as_bytes())?;
    // The stand-in first on the PATH, so a bare `runcss.sh` finds it too.
    let mut path = std::ffi::OsString::from(dir);
    if let Some(p) = std::env::var_os("PATH").filter(|p| !p.is_empty()) {
        path.push(":");
        path.push(p);
    }
    let mut command = Command::new(&launcher);
    command
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        // Its own process group, so that it can be stopped with whatever
        // it started.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = loop {
        match command.spawn() {
            // Busy, as above, through a process forked elsewhere in bobview.
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => break result?,
        }
    };
    drop(writing);
    loop {
        match child.try_wait()? {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                // SAFETY: kill(2) on the group this child leads; it is not
                // yet reaped, so the group id cannot have been reused.
                unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
                // Reaped on a thread: a process stuck on a hung file
                // system ignores even SIGKILL until the file system answers.
                std::thread::spawn(move || child.wait());
                log::warn!("launcher script took too long");
                return Ok(None);
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let Ok(out) = std::fs::read(dir.join("args")) else {
        return Ok(None);
    };
    Ok(Some(
        out.split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    ))
}

/// A new directory in `parent` that only this user can use: made here
/// (never one that already exists, which another user could have planted)
/// with mode 0700, under a name nobody can predict.
fn private_dir(parent: &Path) -> Option<PathBuf> {
    use std::hash::{BuildHasher, RandomState};
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..16 {
        let n = RandomState::new().hash_one((std::process::id(), Instant::now()));
        let dir = parent.join(format!("bobview-launch-{n:016x}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Some(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                log::debug!("cannot make a directory to run a launcher in: {e}");
                return None;
            }
        }
    }
    None
}

/// Write a new executable file at `path`; fails if it already exists.
fn write_executable(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)?
        .write_all(data)
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
        // A display opened directly gets the macros its launcher gives it,
        // only in an Eclipse project.
        assert!(defaults(&dir.join("srCell04Vacuum.opi")).is_none());
        std::fs::write(dir.join(".project"), "<projectDescription/>").unwrap();
        let macros = defaults(&dir.join("srCell04Vacuum.opi")).unwrap();
        assert_eq!(macros.get("pcell"), Some("03"));
        assert!(defaults(&dir.join("srCell05Vacuum.opi")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_display_beside_runcss_gets_its_launcher_macros() {
        let dir = tmp("runcss");
        write_executable(&dir.join("srCellVacuum"), LAUNCHER.as_bytes()).unwrap();
        // The module's own launcher, which lists the project's links.
        std::fs::write(
            dir.join("runcss.sh"),
            "#!/bin/bash\nproject=vac_${version}\nmodule=vac\nlinks=\"${SCRIPT_DIR}=${project}/${module}\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("srCell04Vacuum.opi"), "<display/>").unwrap();
        let file = bob::project::launcher_path(&dir.join("srCell04Vacuum.opi")).unwrap();
        assert_eq!(file, dir.join("runcss.sh/vac_dev/vac/srCell04Vacuum.opi"));
        let macros = defaults(&file).unwrap();
        assert_eq!(macros.get("cell"), Some("04"));
        // Its command actions run launchers beside it, and name displays
        // through the project.
        let display = Source::File(file);
        let (target, _) = command_target(Some(&display), None, "./srCellVacuum 04").unwrap();
        assert_eq!(target, dir.join("runcss.sh/vac_dev/vac/srCell04Vacuum.opi"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bare_runcss_finds_the_stand_in() {
        let dir = tmp("bare");
        let script = "#!/bin/sh\nrunCSS=runcss.sh\nexec $runCSS a.opi -m x=1\n";
        write_executable(&dir.join("go"), script.as_bytes()).unwrap();
        let display = Source::File(dir.join("x.opi"));
        let (file, macros) = command_target(Some(&display), None, "./go").unwrap();
        assert_eq!((file, macros.get("x")), (dir.join("a.opi"), Some("1")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn runs_no_script_others_may_change() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("writable");
        let marker = dir.join("ran");
        marking_launcher(&dir, &marker);
        let display = Source::File(dir.join("x.opi"));
        let mode = |p: &Path, m| std::fs::set_permissions(p, PermissionsExt::from_mode(m));
        mode(&dir.join("launch"), 0o777).unwrap();
        assert!(command_target(Some(&display), None, "./launch").is_none());
        mode(&dir.join("launch"), 0o700).unwrap();
        mode(&dir, 0o777).unwrap();
        assert!(command_target(Some(&display), None, "./launch").is_none());
        assert!(!marker.exists());
        mode(&dir, 0o755).unwrap();
        assert!(command_target(Some(&display), None, "./launch").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stops_a_slow_launcher_and_what_it_started() {
        let dir = tmp("slow");
        let marker = dir.join("late");
        let script = format!(
            "#!/bin/sh\n(sleep 1; touch '{}') &\nsleep 10\nruncss.sh x.opi\n",
            marker.display()
        );
        let start = Instant::now();
        let deadline = start + Duration::from_millis(300);
        assert_eq!(run(script.as_bytes(), &[], deadline), None);
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            !marker.exists(),
            "the script's background job was not stopped"
        );
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
