// LuLu dances to what the computer plays (Settings → General, off by default).
//
// The computer's sound (speech::audio's loopback) is opened only while the
// option is on, the island shows and the system media controls say something
// plays — YouTube in a browser, Spotify, any player. Its beat goes to the
// island as `dance-beat` events (beat.rs); the sound itself goes nowhere.
// While the option is on, one thread sleeps on the media controls' events and
// on the island showing or hiding; with it off, nothing runs at all.

mod beat;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::island;
use crate::speech::audio::{self, Capture, Source};
use beat::{BeatDetector, BeatEvent};

const EVENT: &str = "dance-beat";
/// While listening, how often to look whether the speakers changed (headphones
/// connected, another output chosen): loopback listens to one device only.
const SPEAKERS_CHECK: Duration = Duration::from_secs(3);

/// What wakes the dance thread.
#[cfg_attr(not(windows), allow(dead_code))]
enum Wake {
    /// A player started, paused or stopped.
    Playback,
    /// A player appeared or went away.
    Sessions,
    /// The island showed or hid.
    Visible,
    Stop,
}

struct Shared {
    /// Bumped on every start and stop: a thread from an older one quits.
    generation: u64,
    on: bool,
    visible: bool,
    wake: Option<Sender<Wake>>,
}

static SHARED: LazyLock<Mutex<Shared>> =
    LazyLock::new(|| Mutex::new(Shared { generation: 0, on: false, visible: true, wake: None }));

/// The option was turned on or off (at launch and on every save).
pub fn set_enabled(app: &AppHandle, on: bool) {
    let (tx, rx) = mpsc::channel();
    let (old, generation) = {
        let mut s = SHARED.lock().unwrap();
        if s.on == on {
            return;
        }
        s.on = on;
        s.generation += 1;
        let old = s.wake.take();
        if on {
            s.wake = Some(tx.clone());
        }
        (old, s.generation)
    };
    if let Some(old) = old {
        let _ = old.send(Wake::Stop);
    }
    if on {
        let app = app.clone();
        let spawned = std::thread::Builder::new().name("lulu-dance".into()).spawn(move || run(app, generation, tx, rx));
        if let Err(err) = spawned {
            crate::log::line(format!("dance: no thread: {err}"));
        }
    }
}

/// The island showed or hid: hidden, nothing is heard.
pub fn set_visible(visible: bool) {
    let mut s = SHARED.lock().unwrap();
    s.visible = visible;
    if let Some(tx) = &s.wake {
        let _ = tx.send(Wake::Visible);
    }
}

fn current(generation: u64) -> Option<bool> {
    let s = SHARED.lock().unwrap();
    (s.on && s.generation == generation).then_some(s.visible)
}

/// Opens and closes the computer's sound as the island and the players change.
fn run(app: AppHandle, generation: u64, tx: Sender<Wake>, rx: Receiver<Wake>) {
    let Some(mut media) = media::Watch::start(&tx) else { return };
    let mut capture: Option<Capture> = None;
    let mut listening_on: Option<String> = None;
    while let Some(visible) = current(generation) {
        let want = visible && media.playing();
        if want && capture.is_none() {
            listening_on = audio::default_output_name();
            capture = listen(&app);
        } else if !want && capture.is_some() {
            stop(&app, &mut capture);
        }

        // Events come in bursts (a new song moves everything): one look each.
        let mut rebind = false;
        let mut quit = false;
        let first = if capture.is_some() {
            match rx.recv_timeout(SPEAKERS_CHECK) {
                Ok(wake) => wake,
                Err(RecvTimeoutError::Timeout) => {
                    // Other speakers or headphones now: listen to them instead.
                    if audio::default_output_name() != listening_on {
                        crate::log::line("dance: the speakers changed, listening to the new ones");
                        stop(&app, &mut capture);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            let Ok(wake) = rx.recv() else { break };
            wake
        };
        for wake in std::iter::once(first).chain(rx.try_iter()) {
            match wake {
                Wake::Playback | Wake::Visible => {}
                Wake::Sessions => rebind = true,
                Wake::Stop => quit = true,
            }
        }
        if quit {
            break;
        }
        if rebind {
            media.rebind(&tx);
        }
    }
    stop(&app, &mut capture);
}

fn listen(app: &AppHandle) -> Option<Capture> {
    let app = app.clone();
    let mut detector = BeatDetector::default();
    let sink = move |samples: &[f32]| {
        detector.feed(samples, |event| {
            let _ = app.emit_to(island::WINDOW_LABEL, EVENT, event);
            // The desktop LuLu dances in step too.
            let _ = app.emit_to(crate::desktop::LABEL, EVENT, event);
        })
    };
    match Capture::start(Source::System, sink) {
        Ok(capture) => Some(capture),
        Err(err) => {
            crate::log::line(format!("dance: the computer's sound is unavailable: {err}"));
            None
        }
    }
}

/// Closes the sound and tells the island the beat is gone.
fn stop(app: &AppHandle, capture: &mut Option<Capture>) {
    if let Some(mut c) = capture.take() {
        c.stop();
        let gone = BeatEvent { bpm: 0.0, phase: 0.0, strength: 0.0, beat: false };
        let _ = app.emit_to(island::WINDOW_LABEL, EVENT, gone);
        let _ = app.emit_to(crate::desktop::LABEL, EVENT, gone);
    }
}

/// Whether a player plays, from Windows' media controls (every app that shows
/// in the volume flyout's media card).
#[cfg(windows)]
mod media {
    use super::Wake;
    use std::sync::mpsc::Sender;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    pub struct Watch {
        manager: Manager,
        sessions_token: Option<i64>,
        bound: Vec<(Session, i64)>,
    }

    impl Watch {
        pub fn start(tx: &Sender<Wake>) -> Option<Watch> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let manager = match Manager::RequestAsync().and_then(|op| op.get()) {
                Ok(m) => m,
                Err(err) => {
                    crate::log::line(format!("dance: no media sessions: {err}"));
                    return None;
                }
            };
            let sessions_token = {
                let tx = tx.clone();
                manager
                    .SessionsChanged(&TypedEventHandler::new(move |_, _| {
                        let _ = tx.send(Wake::Sessions);
                        Ok(())
                    }))
                    .ok()
            };
            let mut watch = Watch { manager, sessions_token, bound: Vec::new() };
            watch.rebind(tx);
            Some(watch)
        }

        /// Listens to every player's start and pause again.
        pub fn rebind(&mut self, tx: &Sender<Wake>) {
            self.unbind();
            let Ok(sessions) = self.manager.GetSessions() else { return };
            for i in 0..sessions.Size().unwrap_or(0) {
                let Ok(session) = sessions.GetAt(i) else { continue };
                let tx = tx.clone();
                let handler = TypedEventHandler::new(move |_, _| {
                    let _ = tx.send(Wake::Playback);
                    Ok(())
                });
                if let Ok(token) = session.PlaybackInfoChanged(&handler) {
                    self.bound.push((session, token));
                }
            }
        }

        pub fn playing(&self) -> bool {
            let Ok(sessions) = self.manager.GetSessions() else { return false };
            (0..sessions.Size().unwrap_or(0)).filter_map(|i| sessions.GetAt(i).ok()).any(|s| {
                s.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).is_ok_and(|status| status == Status::Playing)
            })
        }

        fn unbind(&mut self) {
            for (session, token) in self.bound.drain(..) {
                let _ = session.RemovePlaybackInfoChanged(token);
            }
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.unbind();
            if let Some(token) = self.sessions_token.take() {
                let _ = self.manager.RemoveSessionsChanged(token);
            }
        }
    }
}

/// Linux has no system-wide media controls here yet: the option is not offered
/// (src/settings/main.ts), and nothing is ever heard.
#[cfg(not(windows))]
mod media {
    use super::Wake;
    use std::sync::mpsc::Sender;

    pub struct Watch;

    impl Watch {
        pub fn start(_tx: &Sender<Wake>) -> Option<Watch> {
            None
        }

        pub fn rebind(&mut self, _tx: &Sender<Wake>) {}

        pub fn playing(&self) -> bool {
            false
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Listens to the computer for 20 s and prints each beat: run by hand with
    /// music playing (`cargo test -p lulu --lib dance::tests -- --ignored --nocapture`).
    #[test]
    #[ignore = "needs music playing on this computer"]
    fn hears_the_music_playing_now() {
        let (tx, _rx) = mpsc::channel();
        let media = media::Watch::start(&tx).expect("the media controls");
        println!("something plays: {}", media.playing());
        let start = Instant::now();
        let mut detector = BeatDetector::default();
        let capture = Capture::start(Source::System, move |samples| {
            detector.feed(samples, |e| {
                if e.beat || e.bpm == 0.0 {
                    println!("{:6.2} s  {:5.1} BPM  strength {:.2}", start.elapsed().as_secs_f64(), e.bpm, e.strength);
                }
            })
        })
        .expect("the computer's sound");
        std::thread::sleep(Duration::from_secs(20));
        drop(capture);
    }
}
