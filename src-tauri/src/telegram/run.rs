// Tasks for Claude Code from the phone (Telegram `/run project task`).
//
// Off until Settings turns it on, and then only for the projects listed in
// Settings: a name and a folder each, added there and nowhere else, never from
// Telegram. One task at a time. LuLu starts the user's own Claude Code
// (`claude -p`, the task on its standard input, in the project's folder) in its
// ordinary permission mode, so every tool that needs a yes still asks through
// the island and the phone's cards (PermissionRequest reaches LuLu's hook as in
// any session), and a tool nobody answers for is refused. Nothing is ever
// started with the permission checks off. What Claude Code says in the end goes
// back to the chat; `/stop`, or half an hour, ends the task and everything it
// started.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::i18n::{t, tf};
use crate::{log, platform};

/// Projects the list may hold.
pub const MAX_PROJECTS: usize = 10;
/// Longest task, in characters.
pub const MAX_TASK: usize = 2000;
/// A task that has not finished by then is ended.
const TIMEOUT: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_millis(500);
/// What is kept of Claude Code's answer (the rest is read and dropped).
const MAX_OUTPUT: u64 = 1 << 20;
/// What goes back to the chat: the end of the answer, within Telegram's 4096.
const REPLY_CHARS: usize = 3300;

/// `claude -p` takes its prompt from standard input (so the task is never part
/// of a command line, and a task starting with `-` is no option), in the mode
/// that asks. Never a mode that skips the checks.
const ARGS: [&str; 5] = ["-p", "--permission-mode", "default", "--output-format", "text"];

/// A folder LuLu may start Claude Code in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
}

// ── The list ──────────────────────────────────────────────────────────────────

/// What the chat calls a project: 1 to 24 of a–z, 0–9, `-`, `_`.
pub fn valid_name(name: &str) -> bool {
    (1..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A plain local folder path as text: on a drive, no network or device path.
#[cfg(windows)]
pub fn plain_path(path: &str) -> bool {
    crate::voice::approval::local_drive_path(path)
}

#[cfg(not(windows))]
pub fn plain_path(path: &str) -> bool {
    path.starts_with('/') && !path.split('/').any(|part| part == "..")
}

/// The list as it may be trusted: nothing odd that a hand-edited file could add.
pub fn trusted(mut projects: Vec<Project>) -> Vec<Project> {
    let mut seen = Vec::new();
    projects.retain(|p| {
        let fine = valid_name(&p.name) && plain_path(&p.path) && !seen.contains(&p.name);
        seen.push(p.name.clone());
        fine
    });
    projects.truncate(MAX_PROJECTS);
    projects
}

/// "Add" in Settings: the project to put in the list, or what is wrong with it.
/// The folder's text is checked before the disk is touched (a network path
/// would make Windows reach out to that server), then it is resolved and must be
/// a folder; what is kept is the resolved path.
pub fn check_project(name: &str, path: &str, existing: &[Project]) -> Result<Project, String> {
    let name = name.trim().to_ascii_lowercase();
    if !valid_name(&name) {
        return Err(t("Use 1 to 24 Latin letters, digits, - or _ for the name."));
    }
    if existing.iter().any(|p| p.name == name) {
        return Err(t("That name is already in the list."));
    }
    if existing.len() >= MAX_PROJECTS {
        return Err(t("Up to 10 projects."));
    }
    let wrong = || t("Pick an existing folder on a local drive.");
    let path = path.trim().trim_matches('"');
    if !plain_path(path) {
        return Err(wrong());
    }
    let real = std::fs::canonicalize(path).ok().filter(|p| p.is_dir()).ok_or_else(wrong)?;
    let real = real.to_string_lossy();
    // A drive that is really a network share resolves to `\\?\UNC\…`: no drive letter, refused.
    let text = real.strip_prefix(r"\\?\").unwrap_or(&real).to_string();
    if !plain_path(&text) {
        return Err(wrong());
    }
    Ok(Project { name, path: text })
}

// ── Running ───────────────────────────────────────────────────────────────────

struct Active {
    project: String,
    /// `/stop` was sent: the watching thread ends the task.
    stop: bool,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// The project a task is running in, if one is.
pub fn running() -> Option<String> {
    ACTIVE.lock().unwrap().as_ref().map(|a| a.project.clone())
}

/// `/stop`: false when nothing runs. The watching thread ends the task and says so.
pub fn stop() -> bool {
    match ACTIVE.lock().unwrap().as_mut() {
        Some(active) => {
            active.stop = true;
            true
        }
        None => false,
    }
}

/// Where Claude Code's own program is: a real `.exe` (never a `.cmd`, which would
/// go through the shell), in npm's global folder or the installer's.
fn claude_exe() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let mut found: Vec<PathBuf> = Vec::new();
    if let Some(appdata) = var("APPDATA") {
        found.push(appdata.join("npm/node_modules/@anthropic-ai/claude-code/bin").join(claude_name()));
    }
    for home in [var("USERPROFILE"), var("HOME")].into_iter().flatten() {
        found.push(home.join(".local/bin").join(claude_name()));
    }
    found.extend(platform::find_on_path("claude").filter(|p| p.extension().is_none_or(|e| !e.eq_ignore_ascii_case("cmd") && !e.eq_ignore_ascii_case("bat"))));
    found.into_iter().find(|p| p.is_file())
}

fn claude_name() -> &'static str {
    if cfg!(windows) { "claude.exe" } else { "claude" }
}

/// Starts the task and returns at once; the answer comes as a message.
pub fn start(app: &AppHandle, project: &Project, task: &str) -> Result<(), String> {
    if !plain_path(&project.path) || !Path::new(&project.path).is_dir() {
        return Err(t("The project folder is missing."));
    }
    let Some(exe) = claude_exe() else { return Err(t("Claude Code was not found on this computer.")) };
    {
        let mut active = ACTIVE.lock().unwrap();
        if active.is_some() {
            return Err(t("A task is already running. Send /stop to end it."));
        }
        *active = Some(Active { project: project.name.clone(), stop: false });
    }
    let name = project.name.clone();
    let (app, project, task) = (app.clone(), project.clone(), task.to_string());
    let spawned = std::thread::Builder::new().name("lulu-run".into()).spawn(move || {
        let (outcome, stdout, stderr) = work(&exe, &project, &task);
        *ACTIVE.lock().unwrap() = None;
        log::line(format!("telegram: task in {} ended", project.name));
        super::notify(&app, &report(&project.name, &outcome, &stdout, &stderr));
    });
    if let Err(err) = spawned {
        *ACTIVE.lock().unwrap() = None;
        log::line(format!("telegram: no thread for a task: {err}"));
        return Err(t("Claude Code could not be started."));
    }
    log::line(format!("telegram: task started in {name}"));
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Done,
    Failed,
    Stopped,
    TimedOut,
}

/// Runs Claude Code to its end, a `/stop` or the time limit.
fn work(exe: &Path, project: &Project, task: &str) -> (Outcome, Vec<u8>, Vec<u8>) {
    let mut cmd = Command::new(exe);
    cmd.args(ARGS).current_dir(&project.path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    platform::no_console(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            log::line(format!("telegram: Claude Code did not start: {err}"));
            return (Outcome::Failed, Vec::new(), t("Claude Code could not be started.").into_bytes());
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(task.as_bytes());
        // Dropped here: Claude Code sees the end of the prompt.
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = (&mut pipe).take(MAX_OUTPUT).read_to_end(&mut kept);
                // Past the limit it is read and dropped, so Claude Code is never stuck writing.
                let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            }
            kept
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    let started = Instant::now();
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break if status.success() { Outcome::Done } else { Outcome::Failed },
            Ok(None) => {}
            Err(_) => break Outcome::Failed,
        }
        let stopped = ACTIVE.lock().unwrap().as_ref().is_some_and(|a| a.stop);
        if stopped || started.elapsed() > TIMEOUT {
            kill_tree(&mut child);
            break if stopped { Outcome::Stopped } else { Outcome::TimedOut };
        }
        std::thread::sleep(POLL);
    };
    let _ = child.wait();
    (outcome, out.join().unwrap_or_default(), err.join().unwrap_or_default())
}

/// Ends Claude Code and everything it started (the commands it ran).
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let mut kill = Command::new("taskkill");
        kill.args(["/T", "/F", "/PID", &child.id().to_string()]);
        platform::no_console(&mut kill);
        let _ = kill.status();
    }
    let _ = child.kill();
}

/// The last `max` characters.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    format!("…{}", text.chars().skip(count - max + 1).collect::<String>())
}

/// What the chat is told when a task ends.
fn report(project: &str, outcome: &Outcome, stdout: &[u8], stderr: &[u8]) -> String {
    let (icon, head) = match outcome {
        Outcome::Done => ("✅", tf("Done in {project}:", &[("project", project)])),
        Outcome::Failed => ("❌", tf("Failed in {project}:", &[("project", project)])),
        Outcome::Stopped => return format!("⏹ {}", tf("Stopped: {project}", &[("project", project)])),
        Outcome::TimedOut => {
            let minutes = (TIMEOUT.as_secs() / 60).to_string();
            return format!("⌛ {}", tf("Timed out after {minutes} minutes: {project}", &[("minutes", &minutes), ("project", project)]));
        }
    };
    let said = String::from_utf8_lossy(stdout);
    let said = if said.trim().is_empty() { String::from_utf8_lossy(stderr) } else { said };
    let said = said.trim();
    let body = if said.is_empty() { t("No output.") } else { tail(said, REPLY_CHARS) };
    format!("{icon} {head}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_is_started_in_its_asking_mode_and_never_with_the_checks_off() {
        assert_eq!(ARGS, ["-p", "--permission-mode", "default", "--output-format", "text"]);
        let all = ARGS.join(" ");
        for never in ["dangerously", "bypass", "acceptEdits", "dontAsk", "--bare", "allowedTools"] {
            assert!(!all.contains(never), "{never}");
        }
    }

    #[test]
    fn project_names_are_short_and_plain() {
        for ok in ["shop", "a", "my-app_2", &"x".repeat(24)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Shop", "my app", "a/b", "a.b", "-rf ", "магазин", &"x".repeat(25), "a\nb"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn only_a_real_local_folder_goes_into_the_list() {
        let dir = std::env::temp_dir().join(format!("lulu-run-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let path = dir.to_string_lossy().to_string();

        let one = check_project(" Shop ", &format!("\"{path}\""), &[]).unwrap();
        assert_eq!(one.name, "shop");
        assert!(plain_path(&one.path) && Path::new(&one.path).is_dir());
        // Taken names, a full list, bad names.
        assert!(check_project("shop", &path, std::slice::from_ref(&one)).is_err());
        let full: Vec<Project> = (0..MAX_PROJECTS).map(|i| Project { name: format!("p{i}"), path: path.clone() }).collect();
        assert!(check_project("new", &path, &full).is_err());
        assert!(check_project("bad name", &path, &[]).is_err());
        // Not a folder, not there, not local, not plain.
        for bad in [
            file.to_string_lossy().to_string(), format!("{path}\\missing"), r"\\server\share\x".into(), r"\\?\C:\Windows".into(),
            r"\\.\C:\".into(), "relative\\dir".into(), format!("{path}\\..\\.."), "C:".into(), String::new(), format!("{path}:stream"),
        ] {
            assert!(check_project("x", &bad, &[]).is_err(), "{bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in for Claude Code: a batch file that prints its folder and what it
    /// was told on standard input, or hangs when the prompt says so.
    #[cfg(windows)]
    #[test]
    fn a_task_runs_in_its_folder_reads_its_prompt_from_stdin_and_stop_ends_it() {
        let dir = std::env::temp_dir().join(format!("lulu-run-stub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("claude-stub.cmd");
        std::fs::write(&stub, "@echo off
echo cwd=%CD%
echo args=%*
more
").unwrap();
        let project = Project { name: "stub".into(), path: dir.to_string_lossy().to_string() };

        let (outcome, stdout, _) = work(&stub, &project, "-p fix the footer");
        let said = String::from_utf8_lossy(&stdout).to_string();
        assert_eq!(outcome, Outcome::Done, "{said}");
        assert!(said.to_lowercase().contains(&format!("cwd={}", dir.to_string_lossy().to_lowercase())), "{said}");
        assert!(said.contains("args=-p --permission-mode default --output-format text"), "{said}");
        // The task came on stdin: it is not among the arguments, though it starts with a dash.
        assert!(said.contains("-p fix the footer") && !said.contains("args=-p fix"), "{said}");

        // A task that never ends is stopped, and the process it started goes with it.
        std::fs::write(&stub, "@echo off
ping -n 60 127.0.0.1 >nul
").unwrap();
        *ACTIVE.lock().unwrap() = Some(Active { project: "stub".into(), stop: false });
        let stopper = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            assert!(stop());
        });
        let started = Instant::now();
        let (outcome, _, _) = work(&stub, &project, "wait");
        stopper.join().unwrap();
        *ACTIVE.lock().unwrap() = None;
        assert_eq!(outcome, Outcome::Stopped);
        assert!(started.elapsed() < Duration::from_secs(20), "stopped after {:?}", started.elapsed());
        // Nothing runs now: a second /stop finds nothing.
        assert!(!stop());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_edited_list_is_cleaned_before_it_is_trusted() {
        let p = |name: &str, path: &str| Project { name: name.into(), path: path.into() };
        let kept = trusted(vec![
            p("shop", "C:\\work\\shop"), p("Shop2", "C:\\work\\x"), p("shop", "C:\\work\\dup"), p("net", "\\\\srv\\share"), p("ok_2", "D:/a/b"),
        ]);
        let names: Vec<_> = kept.iter().map(|p| p.name.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(names, ["shop", "ok_2"]);
        }
        let many: Vec<Project> = (0..30).map(|i| p(&format!("p{i}"), "C:\\work")).collect();
        assert!(trusted(many).len() <= MAX_PROJECTS);
    }

    #[test]
    fn the_chat_gets_the_end_of_the_answer_and_never_more_than_it_takes() {
        assert_eq!(tail("short", 100), "short");
        let long = format!("{}END", "a".repeat(10_000));
        let cut = tail(&long, REPLY_CHARS);
        assert_eq!(cut.chars().count(), REPLY_CHARS);
        assert!(cut.starts_with('…') && cut.ends_with("END"));
        let ok = report("shop", &Outcome::Done, long.as_bytes(), b"");
        assert!(ok.starts_with("✅ ") && ok.contains("shop") && ok.ends_with("END"));
        assert!(ok.chars().count() < 4096);
        // Nothing on stdout: what went to stderr tells why; nothing at all says so.
        assert!(report("shop", &Outcome::Failed, b"", b"not logged in").contains("not logged in"));
        assert!(report("shop", &Outcome::Done, b"  \n", b"").contains("No output."));
        assert!(report("shop", &Outcome::Stopped, b"partial", b"").starts_with("⏹ "));
        assert!(!report("shop", &Outcome::Stopped, b"partial", b"").contains("partial"));
        assert!(report("shop", &Outcome::TimedOut, b"", b"").contains("30"));
    }
}
