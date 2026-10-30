// Approvals in Telegram (Settings → Agents → Telegram): a permission request
// whose card is up on the island is also sent, as a message with Allow / Deny
// buttons, to the user's own bot — and a press there answers the agent exactly
// as a click on the island would.
//
// Off by default. There is no server of ours: while it is on and paired, LuLu
// asks Telegram's Bot API for button presses itself (getUpdates, long polling,
// HTTPS to api.telegram.org); while it is off nothing is sent and nothing polls.
// The bot's token and the owner's chat id live in the credential store
// (secrets.rs), never on disk, never in the log; telegram.json holds only the
// switches and the bot's public name.
//
// Never in the agent's way: a card goes out only once the island has the
// request on screen (pipe.rs), the wait for a decision stays the island's
// (108 s), and whoever answers first — the island, the terminal or the phone —
// decides. The other place is told how it ended (core.rs). Telegram being down
// only means no card arrives.

mod core;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::i18n::{t, tf};
use crate::{island, log, pipe, recap, secrets};

pub use self::core::{Draft, Ending};
use self::core::{Book, Command, Press, Start};

const TOKEN_KEY: &str = "telegram-bot-token";
const OWNER_KEY: &str = "telegram-owner";
const API: &str = "https://api.telegram.org";
/// How long getUpdates holds a request open when nothing happens.
const POLL_SECONDS: u64 = 25;
/// Ceiling for one answer of the Bot API (a batch of updates is far smaller).
const MAX_ANSWER: usize = 1 << 20;

// ── Switches (telegram.json) ─────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Prefs {
    enabled: bool,
    /// Cards show the tool and the project, never the command or file.
    hide_command: bool,
    /// The bot's @name, from getMe: shown in Settings and used for its t.me link.
    bot: String,
}

fn prefs_path() -> PathBuf {
    crate::platform::config_dir().join("telegram.json")
}

fn load_prefs() -> Prefs {
    let mut p = std::fs::read(prefs_path()).ok().and_then(|b| serde_json::from_slice::<Prefs>(&b).ok()).unwrap_or_default();
    if !is_bot_name(&p.bot) {
        p.bot.clear();
    }
    p
}

fn save_prefs(prefs: &Prefs) {
    let path = prefs_path();
    let Some(dir) = path.parent() else { return };
    let temp = path.with_extension(format!("json.lulu-{}", std::process::id()));
    let written = crate::platform::ensure_private_dir(dir)
        .and_then(|()| serde_json::to_vec_pretty(prefs).map_err(std::io::Error::other))
        .and_then(|json| std::fs::write(&temp, json))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temp);
        log::line(format!("telegram: could not save telegram.json: {err}"));
    }
}

/// `123456789:AA…` — the shape @BotFather hands out. Checked before anything is sent.
fn is_token(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else { return false };
    (5..=16).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (30..=64).contains(&secret.len())
        && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A bot's username: what may go into a t.me link.
fn is_bot_name(name: &str) -> bool {
    (5..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random<const N: usize>() -> Option<[u8; N]> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes)
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Telegram {
    book: Mutex<Book>,
    prefs: Mutex<Prefs>,
    poller: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    client: reqwest::Client,
}

impl Telegram {
    pub fn load() -> Telegram {
        let owner = secrets::get_internal(OWNER_KEY).and_then(|s| s.parse::<i64>().ok());
        Telegram {
            book: Mutex::new(Book::new(owner)),
            prefs: Mutex::new(load_prefs()),
            poller: Mutex::new(None),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(POLL_SECONDS + 15))
                .build()
                .unwrap_or_default(),
        }
    }
}

/// What Settings shows.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    enabled: bool,
    hide_command: bool,
    has_token: bool,
    bot: String,
    paired: bool,
    /// The code to send the bot, while it may be used.
    code: Option<String>,
}

fn status(tg: &Telegram) -> Status {
    let prefs = tg.prefs.lock().unwrap().clone();
    let book = tg.book.lock().unwrap();
    Status {
        enabled: prefs.enabled,
        hide_command: prefs.hide_command,
        has_token: secrets::get_internal(TOKEN_KEY).is_some(),
        bot: prefs.bot,
        paired: book.owner.is_some(),
        code: book.pairing_code(now_ms()).map(str::to_string),
    }
}

/// Settings is told that something changed, and nothing else: a webview's global
/// listener hears events sent to any window, and the status carries the pairing
/// code. Settings asks for it through `telegram_status`, which only it may call.
fn changed(app: &AppHandle) {
    let _ = app.emit_to("settings", "telegram-changed", ());
}

/// At launch: polls only if it is on and a chat owns the bot.
pub fn start(app: &AppHandle) {
    restart_polling(app);
}

/// Stops the poller, and starts a new one when there is something to wait for.
fn restart_polling(app: &AppHandle) {
    let Some(tg) = app.try_state::<Telegram>() else { return };
    if let Some(old) = tg.poller.lock().unwrap().take() {
        old.abort();
    }
    let on = tg.prefs.lock().unwrap().enabled;
    if on && tg.book.lock().unwrap().listening(now_ms()) && secrets::get_internal(TOKEN_KEY).is_some() {
        let app2 = app.clone();
        *tg.poller.lock().unwrap() = Some(tauri::async_runtime::spawn(async move { poll(app2).await }));
    }
}

// ── Bot API ───────────────────────────────────────────────────────────────────

/// One Bot API call. The token is in the address: an error never carries the
/// address (`without_url`), so it can't reach the log or the screen.
async fn call(client: &reqwest::Client, token: &str, method: &str, body: Value) -> Result<Value, String> {
    let url = format!("{API}/bot{token}/{method}");
    let mut response = client.post(url).json(&body).send().await.map_err(|e| e.without_url().to_string())?;
    let code = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url().to_string())? {
        if bytes.len() + chunk.len() > MAX_ANSWER {
            return Err("answer too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let answer: Value = serde_json::from_slice(&bytes).map_err(|_| format!("HTTP {code}"))?;
    if answer.get("ok") == Some(&Value::Bool(true)) {
        return Ok(answer.get("result").cloned().unwrap_or(Value::Null));
    }
    let what = answer.get("description").and_then(Value::as_str).unwrap_or("refused");
    Err(format!("{} {}", code.as_u16(), what.chars().take(200).collect::<String>()))
}

/// Fire and forget: a call whose failure only goes to the log.
fn send_later(app: &AppHandle, method: &'static str, body: Value) {
    let Some(token) = secrets::get_internal(TOKEN_KEY) else { return };
    let client = app.state::<Telegram>().client.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = call(&client, &token, method, body).await {
            log::line(format!("telegram: {method} failed: {err}"));
        }
    });
}

/// The card becomes `text`, without its buttons.
fn edit(app: &AppHandle, chat: i64, message_id: i64, text: String) {
    send_later(app, "editMessageText", json!({
        "chat_id": chat, "message_id": message_id, "text": text, "parse_mode": "HTML",
        "reply_markup": { "inline_keyboard": [] },
    }));
}

async fn poll(app: AppHandle) {
    let mut offset: i64 = 0;
    let mut backoff = 2u64;
    let mut failing = false;
    loop {
        let (client, token) = {
            let Some(tg) = app.try_state::<Telegram>() else { return };
            if !tg.prefs.lock().unwrap().enabled || !tg.book.lock().unwrap().listening(now_ms()) {
                return;
            }
            let Some(token) = secrets::get_internal(TOKEN_KEY) else { return };
            (tg.client.clone(), token)
        };
        let body = json!({ "offset": offset, "timeout": POLL_SECONDS, "allowed_updates": ["message", "callback_query"] });
        match call(&client, &token, "getUpdates", body).await {
            Ok(Value::Array(updates)) => {
                if failing {
                    log::line("telegram: reachable again");
                }
                failing = false;
                backoff = 2;
                for update in updates {
                    if let Some(id) = update.get("update_id").and_then(Value::as_i64) {
                        offset = offset.max(id + 1);
                    }
                    on_update(&app, &update);
                }
            }
            Ok(_) => {}
            Err(err) => {
                // Once per outage, not every few seconds.
                if !failing {
                    log::line(format!("telegram: cannot poll: {err}"));
                }
                failing = true;
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

fn on_update(app: &AppHandle, update: &Value) {
    let tg = app.state::<Telegram>();
    if let Some(message) = update.get("message") {
        let chat = message.pointer("/chat/id").and_then(Value::as_i64).unwrap_or_default();
        let private = message.pointer("/chat/type").and_then(Value::as_str) == Some("private");
        let text = message.get("text").and_then(Value::as_str).unwrap_or_default();
        let started = tg.book.lock().unwrap().on_message(chat, private, text, now_ms());
        if started == Start::Paired {
            if let Err(err) = secrets::set_internal(OWNER_KEY, &chat.to_string()) {
                log::line(format!("telegram: could not store the owner: {err}"));
            }
            log::line("telegram: paired");
            send_later(app, "sendMessage", json!({
                "chat_id": chat,
                "text": format!("🐼 {}", t("LuLu is connected. Your agents' permission requests will arrive here.")),
            }));
            changed(app);
        } else {
            let from = message.pointer("/from/id").and_then(Value::as_i64).unwrap_or_default();
            let command = tg.book.lock().unwrap().command(from, chat, private, text);
            if let Some(command) = command {
                run_command(app, chat, command);
            }
        }
        return;
    }
    let Some(press) = update.get("callback_query") else { return };
    let query_id = press.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let from = press.pointer("/from/id").and_then(Value::as_i64).unwrap_or_default();
    let chat = press.pointer("/message/chat/id").and_then(Value::as_i64).unwrap_or_default();
    let shown_id = press.pointer("/message/message_id").and_then(Value::as_i64);
    let data = press.get("data").and_then(Value::as_str).unwrap_or_default();
    let outcome = tg.book.lock().unwrap().on_press(from, chat, data);
    let toast = match outcome {
        Press::Stranger => None,
        Press::Stale => {
            // An old card (LuLu restarted, or already answered): its buttons go.
            if let Some(id) = shown_id {
                send_later(app, "editMessageReplyMarkup", json!({
                    "chat_id": chat, "message_id": id, "reply_markup": { "inline_keyboard": [] },
                }));
            }
            Some(t("This request is already closed."))
        }
        Press::Decide { request_id, allow, message_id, text } => {
            let word = if allow { "allow" } else { "deny" };
            // Only if the request is still waiting: the island may have been quicker.
            let ending = if pipe::answer(app, &request_id, word) {
                log::line(format!("telegram: decision id={request_id} {word}"));
                recap::record_decision(app, &request_id, word);
                let _ = app.emit_to(island::WINDOW_LABEL, "telegram-decided", json!({ "requestId": request_id, "allow": allow }));
                if allow { Ending::PhoneAllowed } else { Ending::PhoneDenied }
            } else {
                Ending::Computer
            };
            if let Some(id) = message_id.or(shown_id) {
                edit(app, chat, id, self::core::settled_text(&text, ending));
            }
            Some(ending.line())
        }
    };
    let mut body = json!({ "callback_query_id": query_id });
    if let Some(text) = toast {
        body["text"] = json!(text);
    }
    send_later(app, "answerCallbackQuery", body);
}

/// Most open tasks listed in one message, and the characters Telegram takes.
const MAX_LISTED: usize = 20;
const MAX_REPLY: usize = 3800;

/// The owner asked for `/status`, `/tasks` or `/add`: a plain message back.
fn run_command(app: &AppHandle, chat: i64, command: Command) {
    let open = crate::tasks::open_tasks(app);
    let text = match command {
        Command::Status => format!(
            "🐼 {}
{}
{}",
            t("LuLu is running."),
            tf("Waiting for permission: {count}", &[("count", &pipe::waiting_count().to_string())]),
            tf("Open tasks: {count}", &[("count", &open.len().to_string())]),
        ),
        Command::Tasks if open.is_empty() => t("No open tasks."),
        Command::Tasks => {
            let mut out = t("Open tasks:");
            for task in open.iter().take(MAX_LISTED) {
                out.push_str("
• ");
                out.push_str(&task.text);
            }
            out.chars().take(MAX_REPLY).collect()
        }
        Command::Add(text) if text.is_empty() => t("Write the task after the command: /add buy milk"),
        Command::Add(text) => match crate::tasks::add_from_phone(app, &text) {
            Some(added) => {
                log::line("telegram: task added from the phone");
                tf("Added: {task}", &[("task", &added)])
            }
            None => t("Could not add the task."),
        },
    };
    send_later(app, "sendMessage", json!({ "chat_id": chat, "text": text }));
}

// ── Called by pipe.rs ─────────────────────────────────────────────────────────

/// The card a permission request would get, if Telegram approvals are on and
/// paired. Read before the payload goes to the island.
pub fn prepare(app: &AppHandle, payload: &Value) -> Option<Draft> {
    let tg = app.try_state::<Telegram>()?;
    let hide = {
        let prefs = tg.prefs.lock().unwrap();
        if !prefs.enabled {
            return None;
        }
        prefs.hide_command
    };
    tg.book.lock().unwrap().owner?;
    self::core::draft(payload, hide)
}

/// What a waiting request asks for — the tool, its command or file, the
/// project folder, and whether the target was cut short — for the voice's look
/// at risk (voice::approval::allow_ok). None for a question: those have
/// options, not Allow / Deny.
pub fn describe(payload: &Value) -> Option<(String, String, String, bool)> {
    let draft = self::core::draft(payload, false)?;
    let cwd = payload.get("cwd").and_then(Value::as_str).unwrap_or_default().to_string();
    Some((draft.tool, draft.target.unwrap_or_default(), cwd, draft.truncated))
}

/// A plain message to the paired chat (a task's reminder, tasks.rs), when
/// Telegram is on and paired; otherwise nothing. Never waits.
pub fn notify(app: &AppHandle, text: &str) {
    let Some(tg) = app.try_state::<Telegram>() else { return };
    if !tg.prefs.lock().unwrap().enabled {
        return;
    }
    let Some(owner) = tg.book.lock().unwrap().owner else { return };
    let Some(token) = secrets::get_internal(TOKEN_KEY) else { return };
    let client = tg.client.clone();
    let body = json!({ "chat_id": owner, "text": text });
    tauri::async_runtime::spawn(async move {
        match call(&client, &token, "sendMessage", body).await {
            Ok(_) => log::line("telegram: reminder sent"),
            Err(err) => log::line(format!("telegram: reminder failed: {err}")),
        }
    });
}

/// The island has the request on screen: the phone gets its card. Never waits.
pub fn offer(app: &AppHandle, request_id: &str, draft: Draft) {
    let Some(tg) = app.try_state::<Telegram>() else { return };
    let (Some(token), Some(bytes)) = (secrets::get_internal(TOKEN_KEY), random::<12>()) else { return };
    let nonce: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let (owner, text, keyboard) = {
        let mut book = tg.book.lock().unwrap();
        let Some(owner) = book.owner else { return };
        let (text, keyboard) = book.add_card(request_id, &nonce, &draft);
        (owner, text, keyboard)
    };
    let client = tg.client.clone();
    let app = app.clone();
    let request_id = request_id.to_string();
    tauri::async_runtime::spawn(async move {
        let body = json!({ "chat_id": owner, "text": text, "parse_mode": "HTML", "reply_markup": keyboard });
        let sent = call(&client, &token, "sendMessage", body).await;
        let tg = app.state::<Telegram>();
        match sent.as_ref().ok().and_then(|m| m.get("message_id")).and_then(Value::as_i64) {
            Some(message_id) => {
                log::line(format!("telegram: card sent id={request_id}"));
                let late = tg.book.lock().unwrap().sent(&request_id, message_id);
                if let Some(e) = late {
                    edit(&app, owner, e.message_id, e.text);
                }
            }
            None => {
                tg.book.lock().unwrap().not_sent(&request_id);
                let why = sent.err().unwrap_or_else(|| "no message id".into());
                log::line(format!("telegram: card not sent id={request_id}: {why}"));
            }
        }
    });
}

/// The request is over, however it ended: its card, if it has one, says so.
pub fn settle(app: &AppHandle, request_id: &str, ending: Ending) {
    let Some(tg) = app.try_state::<Telegram>() else { return };
    let (owner, done) = {
        let mut book = tg.book.lock().unwrap();
        (book.owner, book.settle(request_id, ending))
    };
    if let (Some(owner), Some(e)) = (owner, done) {
        edit(app, owner, e.message_id, e.text);
    }
}

// ── Settings ──────────────────────────────────────────────────────────────────

/// Every command here is the settings window's alone: the island shows text
/// that comes from elsewhere (chat, WhatsApp, agents), so it may neither read
/// the pairing code nor connect, switch or point the bot anywhere.
fn only_settings(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err("only the settings window may change Telegram approvals".into())
    }
}

#[tauri::command]
pub fn telegram_status(window: WebviewWindow, tg: State<Telegram>) -> Result<Status, String> {
    only_settings(&window)?;
    Ok(status(&tg))
}

#[tauri::command]
pub fn telegram_set_options(window: WebviewWindow, app: AppHandle, tg: State<Telegram>, enabled: bool, hide_command: bool) -> Result<Status, String> {
    only_settings(&window)?;
    {
        let mut prefs = tg.prefs.lock().unwrap();
        prefs.enabled = enabled;
        prefs.hide_command = hide_command;
        save_prefs(&prefs);
    }
    restart_polling(&app);
    changed(&app);
    Ok(status(&tg))
}

/// "Connect": checks the token with Telegram (getMe), keeps it, and shows a
/// pairing code. A new bot means a new owner.
#[tauri::command]
pub async fn telegram_connect(window: WebviewWindow, app: AppHandle, token: String) -> Result<Status, String> {
    only_settings(&window)?;
    let token = token.trim().to_string();
    if !is_token(&token) {
        return Err(t("This doesn't look like a bot token from @BotFather."));
    }
    let client = app.state::<Telegram>().client.clone();
    let me = call(&client, &token, "getMe", json!({})).await.map_err(|err| {
        if err.starts_with("401") || err.starts_with("404") {
            t("Telegram refused this token.")
        } else {
            tf("Telegram can't be reached: {error}", &[("error", &err)])
        }
    })?;
    let bot = me.get("username").and_then(Value::as_str).filter(|n| is_bot_name(n)).unwrap_or_default().to_string();
    secrets::set_internal(TOKEN_KEY, &token)?;
    let _ = secrets::set_internal(OWNER_KEY, "");
    let tg = app.state::<Telegram>();
    {
        let mut prefs = tg.prefs.lock().unwrap();
        prefs.bot = bot;
        prefs.enabled = true;
        save_prefs(&prefs);
    }
    tg.book.lock().unwrap().unpair();
    log::line("telegram: bot token stored");
    new_code(&tg)?;
    restart_polling(&app);
    changed(&app);
    Ok(status(&tg))
}

fn new_code(tg: &Telegram) -> Result<(), String> {
    let bytes = random::<{ self::core::CODE_LEN }>().ok_or("no secure random source")?;
    tg.book.lock().unwrap().start_pairing(self::core::code_from(&bytes), now_ms());
    Ok(())
}

/// A fresh pairing code, while no chat owns the bot.
#[tauri::command]
pub fn telegram_new_code(window: WebviewWindow, app: AppHandle, tg: State<Telegram>) -> Result<Status, String> {
    only_settings(&window)?;
    if secrets::get_internal(TOKEN_KEY).is_none() {
        return Err(t("Connect a bot first."));
    }
    new_code(&tg)?;
    restart_polling(&app);
    changed(&app);
    Ok(status(&tg))
}

/// Forgets the bot, its token and the paired chat. Polling stops.
#[tauri::command]
pub fn telegram_disconnect(window: WebviewWindow, app: AppHandle, tg: State<Telegram>) -> Result<Status, String> {
    only_settings(&window)?;
    let _ = secrets::set_internal(TOKEN_KEY, "");
    let _ = secrets::set_internal(OWNER_KEY, "");
    tg.book.lock().unwrap().unpair();
    {
        let mut prefs = tg.prefs.lock().unwrap();
        prefs.enabled = false;
        prefs.bot.clear();
        save_prefs(&prefs);
    }
    log::line("telegram: disconnected");
    restart_polling(&app);
    changed(&app);
    Ok(status(&tg))
}

/// Opens the bot in Telegram with the pairing code filled in (`/start <code>`).
#[tauri::command]
pub fn telegram_open_bot(window: WebviewWindow, tg: State<Telegram>) -> Result<(), String> {
    only_settings(&window)?;
    let bot = tg.prefs.lock().unwrap().bot.clone();
    let code = tg.book.lock().unwrap().pairing_code(now_ms()).map(str::to_string);
    if !is_bot_name(&bot) {
        return Ok(());
    }
    let url = match code {
        Some(code) => format!("https://t.me/{bot}?start={code}"),
        None => format!("https://t.me/{bot}"),
    };
    crate::platform::open_url(&url);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_botfather_shaped_tokens_are_sent_anywhere() {
        assert!(is_token("123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        assert!(is_token("7012345678:AAF-x_yz0123456789abcdefghijklmnopq"));
        for bad in ["", "123456789", ":AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw", "abc:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw",
            "123456789:short", "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDs/w", "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw\n"] {
            assert!(!is_token(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_token_and_owner_are_internal_secrets_the_island_cannot_reach() {
        let ours: Vec<_> = secrets::INTERNAL_KEYS.iter().filter(|k| k.starts_with("telegram-")).collect();
        assert_eq!(ours, [&TOKEN_KEY, &OWNER_KEY]);
        for key in [TOKEN_KEY, OWNER_KEY] {
            assert!(secrets::is_internal(key));
            assert!(!secrets::KNOWN_KEYS.contains(&key), "{key} must not be reachable from a webview");
        }
    }

    /// Reads this file: every command checks that the settings window called it.
    #[test]
    fn every_command_is_the_settings_windows_alone() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let mut seen = 0;
        for chunk in source.split("#[tauri::command]\n").skip(1) {
            let name = chunk.split("fn ").nth(1).and_then(|r| r.split('(').next()).expect("a command has a name");
            let body = &chunk[..chunk.find("\n}\n").expect("a command ends")];
            assert!(body.contains("window: WebviewWindow") && body.contains("only_settings(&window)?"), "{name} does not check the calling window");
            seen += 1;
        }
        assert_eq!(seen, 6);
        // Events reach every webview's global listener: none may carry the status (its pairing code).
        for line in source.lines().filter(|l| l.contains(".emit")) {
            assert!(!line.contains("status("), "an event carries the status: {line}");
        }
    }

    #[test]
    fn only_a_plain_bot_name_goes_into_a_link() {
        assert!(is_bot_name("lulu_approvals_bot"));
        assert!(!is_bot_name("evil.example/x"));
        assert!(!is_bot_name("a"));
        assert!(!is_bot_name("bot?start=x"));
    }

    #[test]
    fn the_switches_default_to_off_and_hold_no_secret() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs::default());
        assert!(!p.enabled && !p.hide_command);
        let json = serde_json::to_string(&Prefs { enabled: true, hide_command: true, bot: "lulu_bot".into() }).unwrap();
        assert_eq!(json, r#"{"enabled":true,"hideCommand":true,"bot":"lulu_bot"}"#);
    }
}
