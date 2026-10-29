// Whether the user is at the computer: no mouse or key for longer than
// Settings' absence interval (180 s by default) and LuLu dozes off on the
// island, as MaoMao's pet does. Windows says when input last came
// (GetLastInputInfo): when, never what.
//
// One thread asks every 5 s and tells the island only when the answer changes
// (`user-away` event, true or false).

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::island::WINDOW_LABEL;

const EVENT: &str = "user-away";
const EVERY: Duration = Duration::from_secs(5);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("lulu-presence".into()).spawn(move || {
        let mut last = false;
        loop {
            let limit = app
                .try_state::<crate::Shared>()
                .map(|s| s.settings.lock().unwrap().absence_interval)
                .unwrap_or(180.0);
            let now = idle_seconds().is_some_and(|idle| is_away(idle, limit));
            if now != last {
                last = now;
                let _ = app.emit_to(WINDOW_LABEL, EVENT, now);
            }
            std::thread::sleep(EVERY);
        }
    });
    if let Err(err) = spawned {
        crate::log::line(format!("presence: no thread: {err}"));
    }
}

/// Seconds since the last mouse move or key press, on this session.
#[cfg(windows)]
fn idle_seconds() -> Option<f64> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // Both wrap every 49.7 days; the difference doesn't.
    let ms = unsafe { GetTickCount() }.wrapping_sub(info.dwTime);
    Some(ms as f64 / 1000.0)
}

#[cfg(not(windows))]
fn idle_seconds() -> Option<f64> {
    None
}

/// Away once idle for longer than the interval; an interval of 0 or less never.
fn is_away(idle: f64, limit: f64) -> bool {
    limit > 0.0 && idle > limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_only_past_the_interval_and_never_when_it_is_off() {
        assert!(!is_away(179.0, 180.0));
        assert!(!is_away(180.0, 180.0));
        assert!(is_away(181.0, 180.0));
        assert!(!is_away(10_000.0, 0.0));
    }
}
