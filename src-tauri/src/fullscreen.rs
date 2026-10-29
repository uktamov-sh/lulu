// A full-screen app (a game, a video, a presentation) is in front: the island
// steps aside, as MaoMao's pet does. Windows says so through
// SHQueryUserNotificationState, the same signal it uses to hold its own
// notifications; nothing about the app (its title, its window) is read.
//
// One thread asks every 1.5 s and tells the island only when the answer
// changes (`fullscreen` event, true or false). The island decides what to hide:
// a card waiting for an answer stays (src/island/island.ts).

use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::island::WINDOW_LABEL;

const EVENT: &str = "fullscreen";
const EVERY: Duration = Duration::from_millis(1500);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("lulu-fullscreen".into()).spawn(move || {
        let mut last = false;
        loop {
            let now = in_front();
            if now != last {
                last = now;
                let _ = app.emit_to(WINDOW_LABEL, EVENT, now);
            }
            std::thread::sleep(EVERY);
        }
    });
    if let Err(err) = spawned {
        crate::log::line(format!("fullscreen: no thread: {err}"));
    }
}

/// Whether Windows reports a full-screen app, a Direct3D game or presentation mode.
#[cfg(windows)]
fn in_front() -> bool {
    use windows::Win32::UI::Shell::SHQueryUserNotificationState;
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => is_full_screen(state.0),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn in_front() -> bool {
    false
}

/// QUNS_BUSY (a full-screen app), QUNS_RUNNING_D3D_FULL_SCREEN (a game),
/// QUNS_PRESENTATION_MODE (Windows' presentation settings).
#[cfg_attr(not(windows), allow(dead_code))]
fn is_full_screen(state: i32) -> bool {
    matches!(state, 2..=4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_full_screen_app_a_game_or_a_presentation_count() {
        // 1 not present (locked), 2 busy, 3 Direct3D, 4 presentation, 5 accepts
        // notifications, 6 quiet time, 7 an app on the Store's full screen.
        assert!(!is_full_screen(1));
        assert!(is_full_screen(2));
        assert!(is_full_screen(3));
        assert!(is_full_screen(4));
        assert!(!is_full_screen(5));
        assert!(!is_full_screen(6));
    }
}
