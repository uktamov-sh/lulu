// Relay server for lulu-hook.
//
// Windows: the named pipe `\\.\pipe\lulu-<sid>`, one instance per connection.
// Linux: the Unix socket `$XDG_RUNTIME_DIR/lulu.sock`. Every hook event is
// forwarded to the island as a `hook` event. `PermissionRequest` is the only one
// that keeps its connection open: it waits for the island's decision and writes
// it back on the same connection, which is how approving from the island works.
//
// Claude Code is never blocked by us. Three things guarantee it:
//   * lulu-hook gives the connection 300 ms and exits cleanly if we are closed;
//   * we only wait for a human once the island has *confirmed* the card is on
//     screen, so a paused island or a webview that is not listening costs a few
//     hundred milliseconds, not two minutes;
//   * whatever happens we drop the connection after the decision timeout, and
//     the terminal takes over.
//
// What we write back is the bare word `allow` or `deny` — or, when the request
// was Claude Code asking a question, `{"answers":{…}}` with what was picked on
// the island. Turning either into the documented hookSpecificOutput JSON is
// lulu-hook's job, so the wire format Claude Code expects lives in exactly
// one place.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

use crate::island::WINDOW_LABEL;
use crate::log;
use crate::session_window;

/// Slightly under lulu-hook's own 110 s wait, so we always answer first.
const DECISION_TIMEOUT: Duration = Duration::from_secs(108);
/// How long the island gets to say "the card is up". This is the whole of B4:
/// without it, an island that is paused, hidden behind a crashed webview or
/// simply not listening would leave Claude Code staring at a prompt nobody can
/// see for nearly two minutes.
const ACK_TIMEOUT: Duration = Duration::from_millis(800);
const MAX_PAYLOAD: usize = 1 << 20;

/// What the island can say about a permission request.
pub enum Reply {
    /// The card is on screen and a human can act on it.
    Ack,
    /// A human clicked: `allow` or `deny`.
    Decision(String),
    /// Nobody can act on it — paused, or another request already holds the card.
    Decline,
}

/// Permission requests the island has been told about.
#[derive(Default)]
pub struct Pending(pub Mutex<HashMap<String, mpsc::Sender<Reply>>>);

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// `\\.\pipe\lulu-<sid>` — must match lulu-hook's `pipe_path()` exactly.
#[cfg(windows)]
pub fn pipe_name() -> String {
    let key = crate::platform::current_user_sid()
        .unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_else(|_| "user".into()));
    format!(r"\\.\pipe\lulu-{key}")
}

#[cfg(windows)]
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let name = pipe_name();
        // first_pipe_instance also means we refuse to join a pipe somebody else
        // already owns under our name, rather than serving on top of it.
        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&name) {
            Ok(s) => s,
            Err(err) => {
                log::line(format!("cannot open the relay pipe: {err}"));
                return;
            }
        };
        loop {
            if server.connect().await.is_err() {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            // Hand the connected instance to a task and listen on a fresh one.
            let next = match ServerOptions::new().create(&name) {
                Ok(s) => s,
                Err(err) => {
                    log::line(format!("cannot reopen the relay pipe: {err}"));
                    return;
                }
            };
            let connected = std::mem::replace(&mut server, next);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { handle(app, connected).await });
        }
    });
}

#[cfg(target_os = "linux")]
pub fn start(app: AppHandle) {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    tauri::async_runtime::spawn(async move {
        let Some(path) = crate::platform::relay_socket_path() else {
            log::line("no private runtime directory ($XDG_RUNTIME_DIR) — Claude Code hooks are inactive");
            return;
        };
        // A socket file left behind by a crash answers nothing and can go. One
        // that answers belongs to a LuLu that is still running: like
        // first_pipe_instance on Windows, we refuse to serve on top of it.
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                log::line("another LuLu already serves the relay socket");
                return;
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(err) => {
                log::line(format!("cannot open the relay socket: {err}"));
                return;
            }
        };
        // The runtime directory is already 0700; this is belt and braces.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let uid = unsafe { libc::getuid() };
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            // Only the relay run by our own user may drive the island.
            if !matches!(stream.peer_cred(), Ok(c) if c.uid() == uid) {
                log::line("refused a relay connection from another user");
                continue;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move { handle(app, stream).await });
        }
    });
}

/// One accepted relay connection, whatever carries it.
trait Relay: AsyncRead + AsyncWrite + Unpin {
    /// Ends the conversation once everything has been written.
    fn finish(&mut self) {}
    /// The relay process on the other end, where the OS says.
    fn client_pid(&self) -> Option<u32> {
        None
    }
}

#[cfg(windows)]
impl Relay for NamedPipeServer {
    fn finish(&mut self) {
        let _ = self.disconnect();
    }
    fn client_pid(&self) -> Option<u32> {
        use std::os::windows::io::AsRawHandle;
        crate::platform::pipe_client_pid(self.as_raw_handle())
    }
}

/// Dropping the stream closes it; the relay reads up to our newline first.
#[cfg(target_os = "linux")]
impl Relay for tokio::net::UnixStream {
    /// SO_PEERCRED: the process that connected, as the kernel saw it.
    fn client_pid(&self) -> Option<u32> {
        let pid = self.peer_cred().ok()?.pid()?;
        u32::try_from(pid).ok().filter(|pid| *pid > 1)
    }
}

async fn handle(app: AppHandle, mut pipe: impl Relay) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') || buf.len() > MAX_PAYLOAD {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let line = match buf.iter().position(|b| *b == b'\n') {
        Some(i) => &buf[..i],
        None => &buf[..],
    };
    let Ok(mut payload) = serde_json::from_slice::<Value>(line) else { return };
    if !payload.is_object() {
        return;
    }

    let event = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    note_session_window(&pipe, &payload, &event);
    // Counts for the weekly recap — never the command, path or prompt itself.
    crate::recap::observe(&app, &payload);

    if event != "PermissionRequest" {
        // The status line relay calls in with every Claude Code update: not log-worthy.
        if event != "StatusLine" {
            log::line(format!("hook {event}"));
        }
        let _ = app.emit_to(WINDOW_LABEL, "hook", payload);
        pipe.finish();
        return;
    }

    let id = format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let (tx, mut rx) = mpsc::channel::<Reply>(4);
    {
        let pending = app.state::<Pending>();
        pending.0.lock().unwrap().insert(id.clone(), tx);
    }
    crate::recap::note_request(&app, &id, &payload);
    // The phone's card, if Telegram approvals are on: sent once the island has it on screen.
    let mut card = crate::telegram::prepare(&app, &payload);
    // For the voice: only once the island has the card on screen (below).
    let mut voice_card = crate::telegram::describe(&payload)
        .map(|(tool, target, cwd, cut)| (id.clone(), !cut && crate::voice::approval::allow_ok(&tool, &target, &cwd)));
    payload["request_id"] = json!(id);
    log::line(format!("hook PermissionRequest id={id}"));
    let _ = app.emit_to(WINDOW_LABEL, "hook", payload);

    let on_screen = || {
        if let Some(card) = card.take() {
            crate::telegram::offer(&app, &id, card);
        }
        if let Some((id, allow_ok)) = voice_card.take() {
            WAITING.lock().unwrap().push(Waiting { id, allow_ok, shown: std::time::Instant::now() });
        }
    };
    let result = wait_for_decision(&id, &mut rx, ACK_TIMEOUT, DECISION_TIMEOUT, on_screen).await;
    app.state::<Pending>().0.lock().unwrap().remove(&id);
    WAITING.lock().unwrap().retain(|card| card.id != id);
    // However it ended, the phone's card says so (nothing to do when the phone answered it).
    if let Some(ending) = phone_ending(&result) {
        crate::telegram::settle(&app, &id, ending);
    }
    let decision = result.ok();

    // No decision: say nothing at all. lulu-hook then writes nothing to stdout
    // and Claude Code asks in the terminal, exactly as if LuLu were closed.
    if let Some(d) = decision {
        let _ = pipe.write_all(format!("{d}\n").as_bytes()).await;
        let _ = pipe.flush().await;
    }
    pipe.finish();
}

/// A card on screen that Allow / Deny can settle (not a question), as the voice
/// sees it: Allow by voice only where the request is plainly harmless
/// (voice::approval::allow_ok) and the card has been up long enough to be read.
struct Waiting {
    id: String,
    allow_ok: bool,
    shown: std::time::Instant,
}

/// Only cards the island has confirmed on screen.
static WAITING: Mutex<Vec<Waiting>> = Mutex::new(Vec::new());

/// A card must be up this long before a voice can allow it: what was said
/// can't have been about a card that has just replaced another.
const VOICE_SETTLE: Duration = Duration::from_millis(1500);

/// A card waits, and "OK LuLu, allow" may be about it.
pub fn something_waits() -> bool {
    !WAITING.lock().unwrap().is_empty()
}

/// How many cards are up on the island waiting for an answer (Telegram `/status`).
pub fn waiting_count() -> usize {
    WAITING.lock().unwrap().len()
}

/// What answering by voice came to.
#[derive(Debug, PartialEq, Eq)]
pub enum VoiceResult {
    Answered,
    /// Allow refused: not plainly harmless, or cut short.
    Risky,
    /// Allow refused: the card has only just appeared.
    TooSoon,
    Several,
    NothingWaiting,
}

/// "OK LuLu, allow / deny" (voice/approval.rs): the one card waiting is
/// answered like a button press; the island and the phone are told.
pub fn voice_answer(app: &AppHandle, allow: bool) -> VoiceResult {
    let id = {
        let waiting = WAITING.lock().unwrap();
        match waiting.as_slice() {
            [] => return VoiceResult::NothingWaiting,
            [card] => {
                if allow && !card.allow_ok {
                    return VoiceResult::Risky;
                }
                if allow && card.shown.elapsed() < VOICE_SETTLE {
                    return VoiceResult::TooSoon;
                }
                card.id.clone()
            }
            _ => return VoiceResult::Several,
        }
    };
    let word = if allow { "allow" } else { "deny" };
    if !answer(app, &id, word) {
        return VoiceResult::NothingWaiting;
    }
    log::line(format!("voice: id={id} {word}"));
    crate::recap::record_decision(app, &id, word);
    let _ = app.emit_to(WINDOW_LABEL, "voice-decided", json!({ "requestId": id, "allow": allow }));
    VoiceResult::Answered
}

/// Finds, once per session, the window it runs in — see session_window.rs.
///
/// Only while the session is unknown, so the process snapshot is not taken on
/// every event. The relay must still be running for its parents to be found:
/// a permission request always is (it waits for us), a quick event may already
/// have exited, and then a later event of the session tries again.
fn note_session_window(pipe: &impl Relay, payload: &Value, event: &str) {
    let Some(session) = payload.get("session_id").and_then(Value::as_str) else { return };
    if event == "SessionEnd" {
        session_window::forget(session);
        return;
    }
    if session_window::known(session) {
        return;
    }
    let Some(relay) = pipe.client_pid() else { return };
    let ancestors = crate::platform::process_ancestors(relay);
    // No ancestors: the relay was already gone, so try again next time. Some,
    // but none with a window (a classic console): settled, VS Code it is.
    if !ancestors.is_empty() {
        session_window::remember(session, crate::platform::window_owners(&ancestors));
    }
}

/// Why a request ended without a decision.
#[derive(Debug, PartialEq, Eq)]
enum NoDecision {
    /// The island never had it on screen.
    NotShown,
    /// The card went without an answer: the turn moved on, or it was answered in the terminal.
    Released,
    TimedOut,
}

/// What the phone's card says about how the request ended. None: it never had one.
fn phone_ending(result: &Result<String, NoDecision>) -> Option<crate::telegram::Ending> {
    use crate::telegram::Ending;
    Some(match result {
        Ok(d) if d == "allow" || d == "always" => Ending::ComputerAllowed,
        Ok(d) if d == "deny" => Ending::ComputerDenied,
        Ok(_) | Err(NoDecision::Released) => Ending::Computer,
        Err(NoDecision::TimedOut) => Ending::TimedOut,
        Err(NoDecision::NotShown) => return None,
    })
}

/// Two waits: a short one for "the card is up", then the long one for a human.
/// `on_screen` runs once the island has confirmed the card, before the long one.
async fn wait_for_decision(
    id: &str,
    rx: &mut mpsc::Receiver<Reply>,
    ack_timeout: Duration,
    decision_timeout: Duration,
    on_screen: impl FnOnce(),
) -> Result<String, NoDecision> {
    match tokio::time::timeout(ack_timeout, rx.recv()).await {
        Ok(Some(Reply::Ack)) => {}
        // A click that beats the ack is still a click.
        Ok(Some(Reply::Decision(d))) => {
            log::line(format!("hook id={id} answered {}", loggable(&d)));
            return Ok(d);
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} not shown — terminal takes over"));
            return Err(NoDecision::NotShown);
        }
        Ok(None) => return Err(NoDecision::NotShown),
        Err(_) => {
            log::line(format!("hook id={id} island never acknowledged — terminal takes over"));
            return Err(NoDecision::NotShown);
        }
    }
    on_screen();

    match tokio::time::timeout(decision_timeout, rx.recv()).await {
        Ok(Some(Reply::Decision(d))) => {
            log::line(format!("hook id={id} answered {}", loggable(&d)));
            Ok(d)
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} released without a decision"));
            Err(NoDecision::Released)
        }
        _ => {
            log::line(format!("hook id={id} timed out — terminal takes over"));
            Err(NoDecision::TimedOut)
        }
    }
}

/// The log says a question was answered, never with what.
fn loggable(decision: &str) -> &str {
    if decision.starts_with('{') { "a question" } else { decision }
}

/// Whether the reply reached a request that was still waiting. A decision takes
/// the request out of `Pending`, so of two answers (the island and the phone)
/// only the first one finds it.
fn send(app: &AppHandle, request_id: &str, reply: Reply, keep: bool) -> bool {
    send_in(&app.state::<Pending>(), request_id, reply, keep)
}

fn send_in(pending: &Pending, request_id: &str, reply: Reply, keep: bool) -> bool {
    let sender = {
        let mut map = pending.0.lock().unwrap();
        if keep { map.get(request_id).cloned() } else { map.remove(request_id) }
    };
    match sender {
        Some(tx) => tx.try_send(reply).is_ok(),
        None => {
            log::line(format!("reply for id={request_id} — no pending request"));
            false
        }
    }
}

/// The island has the card on screen; the long wait may begin.
pub fn acknowledge(app: &AppHandle, request_id: &str) {
    send(app, request_id, Reply::Ack, true);
}

/// Nobody can act on this one — paused, or another card already holds the view.
pub fn decline(app: &AppHandle, request_id: &str) {
    log::line(format!("decline id={request_id}"));
    send(app, request_id, Reply::Decline, false);
}

/// Called by the island's Allow / Deny buttons (and the phone's). Only ever a
/// bare word: turning it into Claude Code's JSON is lulu-hook's job. False when
/// the request had already been answered or was over.
pub fn answer(app: &AppHandle, request_id: &str, decision: &str) -> bool {
    // "always" goes on to lulu-hook as it is: it asks Claude Code to keep the rule
    // (reply::always), and only does so for a command the hook itself found plain.
    let word = match decision {
        "allow" => "allow",
        "always" => "always",
        _ => "deny",
    };
    log::line(format!("decision id={request_id} {word}"));
    send(app, request_id, Reply::Decision(word.to_string()), false)
}

/// Called when an option is picked for a question Claude Code asked. `answers`
/// maps each question's text to the chosen label, which is the shape
/// AskUserQuestion takes them in.
pub fn answer_question(app: &AppHandle, request_id: &str, answers: &HashMap<String, serde_json::Value>) {
    log::line(format!("decision id={request_id} answered a question"));
    // One line: the relay reads up to the first newline.
    let line = json!({ "answers": answers }).to_string();
    send(app, request_id, Reply::Decision(line), false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::Ending;

    const SHORT: Duration = Duration::from_millis(50);

    fn pending_with(id: &str) -> (Pending, mpsc::Receiver<Reply>) {
        let (tx, rx) = mpsc::channel(4);
        let pending = Pending::default();
        pending.0.lock().unwrap().insert(id.into(), tx);
        (pending, rx)
    }

    #[tokio::test]
    async fn the_first_answer_wins_whether_island_or_phone() {
        let (pending, mut rx) = pending_with("r1");
        assert!(send_in(&pending, "r1", Reply::Ack, true));
        // The phone answers first; the island's click a moment later finds nothing.
        assert!(send_in(&pending, "r1", Reply::Decision("allow".into()), false));
        assert!(!send_in(&pending, "r1", Reply::Decision("deny".into()), false));
        let mut shown = false;
        let got = wait_for_decision("r1", &mut rx, SHORT, SHORT, || shown = true).await;
        assert_eq!(got, Ok("allow".to_string()));
        assert!(shown, "the card went to the phone once the island had it");
        assert_eq!(phone_ending(&got), Some(Ending::ComputerAllowed));
    }

    #[tokio::test]
    async fn the_phone_gets_nothing_for_a_request_the_island_never_showed() {
        let (pending, mut rx) = pending_with("r2");
        assert!(send_in(&pending, "r2", Reply::Decline, false));
        let mut shown = false;
        let got = wait_for_decision("r2", &mut rx, SHORT, SHORT, || shown = true).await;
        assert_eq!(got, Err(NoDecision::NotShown));
        assert!(!shown);
        assert_eq!(phone_ending(&got), None);

        // No ack in time: the same, after the short wait only.
        let (_pending, mut rx) = pending_with("r3");
        let started = std::time::Instant::now();
        let got = wait_for_decision("r3", &mut rx, SHORT, Duration::from_secs(60), || shown = true).await;
        assert_eq!(got, Err(NoDecision::NotShown));
        assert!(!shown && started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn nobody_answering_times_out_and_the_card_says_how_it_ended() {
        let (pending, mut rx) = pending_with("r4");
        send_in(&pending, "r4", Reply::Ack, true);
        let got = wait_for_decision("r4", &mut rx, SHORT, SHORT, || {}).await;
        assert_eq!(got, Err(NoDecision::TimedOut));
        assert_eq!(phone_ending(&got), Some(Ending::TimedOut));

        let (pending, mut rx) = pending_with("r5");
        send_in(&pending, "r5", Reply::Ack, true);
        send_in(&pending, "r5", Reply::Decline, false);
        let got = wait_for_decision("r5", &mut rx, SHORT, SHORT, || {}).await;
        assert_eq!(phone_ending(&got), Some(Ending::Computer));
        assert_eq!(phone_ending(&Ok("deny".into())), Some(Ending::ComputerDenied));
        assert_eq!(phone_ending(&Ok(r#"{"answers":{}}"#.into())), Some(Ending::Computer));
    }
}
