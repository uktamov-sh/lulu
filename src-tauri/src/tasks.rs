// Tasks and reminders (Settings → Tasks), as MaoMao's pet keeps them: a short
// list, each task with an optional time and a daily repeat. When a time comes,
// the island shows it (`reminder` event) and, when Telegram approvals are on
// and paired, the phone gets it as a message (telegram::notify).
//
// The list is %APPDATA%\LuLu\tasks.json, nowhere else. One thread looks at it
// every 20 s; nothing runs while no task has a time.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::island::WINDOW_LABEL;

const EVERY: Duration = Duration::from_secs(20);
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Longest task text kept, in characters.
const MAX_TEXT: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: u64,
    pub text: String,
    /// When to remind, Unix ms; none for a plain to-do.
    pub due: Option<i64>,
    /// Again every day at the same time.
    pub daily: bool,
    pub done: bool,
}

#[derive(Default)]
pub struct Tasks(Mutex<Vec<Task>>);

fn path() -> PathBuf {
    crate::settings::config_dir().join("tasks.json")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn load() -> Vec<Task> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(list: &[Task]) {
    let dir = crate::settings::config_dir();
    let written = crate::platform::ensure_private_dir(&dir).and_then(|_| {
        let tmp = dir.join("tasks.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default())?;
        std::fs::rename(&tmp, path())
    });
    if let Err(err) = written {
        crate::log::line(format!("tasks: could not save: {err}"));
    }
}

pub fn setup(app: &AppHandle) {
    app.manage(Tasks(Mutex::new(load())));
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("lulu-tasks".into()).spawn(move || loop {
        std::thread::sleep(EVERY);
        remind(&app);
    });
    if let Err(err) = spawned {
        crate::log::line(format!("tasks: no thread: {err}"));
    }
}

/// The tasks whose time has come: the island shows each, the phone gets it.
fn remind(app: &AppHandle) {
    let Some(tasks) = app.try_state::<Tasks>() else { return };
    let due = {
        let mut list = tasks.0.lock().unwrap();
        let due = take_due(&mut list, now_ms());
        if !due.is_empty() {
            save(&list);
        }
        due
    };
    for task in due {
        let _ = app.emit_to(WINDOW_LABEL, "reminder", &task.text);
        crate::telegram::notify(app, &format!("⏰ {}", task.text));
    }
}

/// Takes the reminders due at `now` out of the list: a daily task moves to its
/// next time (today's if it is still ahead, else tomorrow's), any other loses
/// its time and stays as a to-do. Done tasks never remind.
fn take_due(list: &mut [Task], now: i64) -> Vec<Task> {
    let mut out = Vec::new();
    for task in list.iter_mut().filter(|t| !t.done) {
        let Some(due) = task.due.filter(|d| *d <= now) else { continue };
        out.push(task.clone());
        task.due = if task.daily {
            let mut next = due + DAY_MS;
            while next <= now {
                next += DAY_MS;
            }
            Some(next)
        } else {
            None
        };
    }
    out
}

fn changed(app: &AppHandle, list: &[Task]) -> Vec<Task> {
    save(list);
    let _ = app.emit("tasks-changed", list);
    list.to_vec()
}

#[tauri::command]
pub fn tasks_list(tasks: State<Tasks>) -> Vec<Task> {
    tasks.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn tasks_add(app: AppHandle, tasks: State<Tasks>, text: String, due: Option<i64>, daily: bool) -> Result<Vec<Task>, String> {
    add(&app, &tasks, text, due, daily)
}

fn add(app: &AppHandle, tasks: &Tasks, text: String, due: Option<i64>, daily: bool) -> Result<Vec<Task>, String> {
    let text: String = text.trim().chars().take(MAX_TEXT).collect();
    if text.is_empty() {
        return Err("empty".into());
    }
    let mut list = tasks.0.lock().unwrap();
    let id = list.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    list.push(Task { id, text, due, daily: daily && due.is_some(), done: false });
    Ok(changed(app, &list))
}

/// The to-dos not yet done, for the Telegram bot's `/tasks` and `/status`.
pub fn open_tasks(app: &AppHandle) -> Vec<Task> {
    let Some(tasks) = app.try_state::<Tasks>() else { return Vec::new() };
    let list = tasks.0.lock().unwrap();
    list.iter().filter(|t| !t.done).cloned().collect()
}

/// A plain to-do from the owner's phone (Telegram `/add`): the text as kept.
pub fn add_from_phone(app: &AppHandle, text: &str) -> Option<String> {
    let tasks = app.try_state::<Tasks>()?;
    let list = add(app, &tasks, text.to_string(), None, false).ok()?;
    list.last().map(|t| t.text.clone())
}

#[tauri::command]
pub fn tasks_set_done(app: AppHandle, tasks: State<Tasks>, id: u64, done: bool) -> Vec<Task> {
    let mut list = tasks.0.lock().unwrap();
    if let Some(t) = list.iter_mut().find(|t| t.id == id) {
        t.done = done;
    }
    changed(&app, &list)
}

#[tauri::command]
pub fn tasks_delete(app: AppHandle, tasks: State<Tasks>, id: u64) -> Vec<Task> {
    let mut list = tasks.0.lock().unwrap();
    list.retain(|t| t.id != id);
    changed(&app, &list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, due: Option<i64>, daily: bool, done: bool) -> Task {
        Task { id, text: format!("t{id}"), due, daily, done }
    }

    #[test]
    fn due_reminders_come_out_once_and_daily_ones_move_to_their_next_time() {
        let now = 10 * DAY_MS + 5_000;
        let mut list = vec![
            task(1, Some(now - 1_000), false, false),
            task(2, Some(now + 60_000), false, false),
            task(3, Some(now - 3 * DAY_MS), true, false),
            task(4, Some(now - 1_000), false, true),
            task(5, None, false, false),
        ];
        let due: Vec<u64> = take_due(&mut list, now).iter().map(|t| t.id).collect();
        assert_eq!(due, vec![1, 3]);
        assert_eq!(list[0].due, None, "a one-off keeps no time");
        assert_eq!(list[1].due, Some(now + 60_000));
        let next = list[2].due.unwrap();
        assert!(next > now && next <= now + DAY_MS, "the daily one is due within the next day");
        assert_eq!((next - (now - 3 * DAY_MS)) % DAY_MS, 0, "at the same time of day");
        assert!(take_due(&mut list, now).is_empty(), "nothing twice");
    }
}
