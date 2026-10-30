// What the Telegram bot decides, without the network: pairing with one chat,
// the cards sent for permission requests, and which button press counts.
// mod.rs carries it to and from the Bot API.
//
// The rules:
//   * a chat becomes the owner by sending `/start <code>` while Settings shows
//     that code (one use, ten minutes, five wrong tries burn it);
//   * a button counts only from the owner, in the owner's chat, with the
//     request's id and the card's random nonce, and only once: the card is
//     taken by the first press, so a second press or one from an old message
//     finds nothing;
//   * the request ends once, whoever answers it: a card the computer settled
//     is edited to say how, and a press that comes after finds nothing.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::i18n::{t, tf};

/// How long the code shown in Settings may be sent.
pub const CODE_TTL_MS: u64 = 10 * 60 * 1000;
/// Wrong codes before the one shown is burnt and a new one must be asked for.
pub const MAX_CODE_TRIES: u32 = 5;
/// Telegram takes 4096 characters a message; the card's other lines fit in the rest.
pub const MAX_TARGET_CHARS: usize = 3000;
/// 32 symbols that can't be mistaken for one another, so a byte's low five bits pick one uniformly.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 8;

/// The fields of a tool's input that say what it acts on, most specific first —
/// the island's APPROVAL_FIELDS (src/island/hooks.ts).
const TARGET_FIELDS: [&str; 7] = ["command", "file_path", "path", "url", "query", "pattern", "prompt"];

/// Compares secrets in a time that does not depend on where they differ.
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A pairing code from 8 random bytes.
pub fn code_from(bytes: &[u8; CODE_LEN]) -> String {
    bytes.iter().map(|b| CODE_ALPHABET[(b & 31) as usize] as char).collect()
}

/// What a card says, read from the hook's payload before the island gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub agent: String,
    pub project: String,
    pub tool: String,
    /// The command or file; None when Settings hides it or the tool has none.
    pub target: Option<String>,
    /// Hidden on purpose (as opposed to a tool that has nothing to show).
    pub hidden: bool,
    /// The target was cut to fit: such a request can only be denied from the phone.
    pub truncated: bool,
}

/// The card for a permission request, or None for what the phone can't answer:
/// a question (it has options, not Allow / Deny) or a payload without a tool.
pub fn draft(payload: &Value, hide_target: bool) -> Option<Draft> {
    let tool = payload.get("tool_name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())?;
    if tool == "AskUserQuestion" {
        return None;
    }
    let agent = match payload.get("coucou_agent").and_then(Value::as_str) {
        None | Some("") | Some("claude") => "Claude Code".to_string(),
        Some(id) => crate::agents::Agent::from_id(id).map(|a| a.name().to_string()).unwrap_or_else(|| id.to_string()),
    };
    let cwd = payload.get("cwd").and_then(Value::as_str).unwrap_or_default();
    let project = cwd.split(['/', '\\']).filter(|s| !s.is_empty()).next_back().unwrap_or_default().to_string();
    let input = payload.get("tool_input");
    let full = TARGET_FIELDS
        .iter()
        .find_map(|f| input.and_then(|i| i.get(*f)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()));
    let truncated = full.is_some_and(|s| s.chars().count() > MAX_TARGET_CHARS);
    let target = if hide_target { None } else { full.map(|s| s.chars().take(MAX_TARGET_CHARS).collect()) };
    Some(Draft { agent, project, tool: clip(tool, 64), target, hidden: hide_target && full.is_some(), truncated })
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Telegram's HTML mode needs only these three escaped.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The card's text (HTML), without its buttons.
pub fn card_text(d: &Draft) -> String {
    let mut out = format!("🔐 <b>{}</b>", esc(&tf("{agent} asks for permission", &[("agent", &d.agent)])));
    if !d.project.is_empty() {
        out.push_str(&format!("\n📁 {}", esc(&d.project)));
    }
    out.push_str(&format!("\n🛠 <b>{}</b>", esc(&d.tool)));
    if let Some(target) = &d.target {
        out.push_str(&format!("\n<pre>{}</pre>", esc(target)));
    } else if d.hidden {
        out.push_str(&format!("\n<i>{}</i>", esc(&t("Command hidden in LuLu's settings"))));
    }
    if d.truncated {
        out.push_str(&format!("\n<i>{}</i>", esc(&t("Too long to show in full: allow it on the computer."))));
    }
    out
}

/// The buttons under a card. No Allow for a command the phone could not show in full.
pub fn keyboard(request_id: &str, nonce: &str, allow: bool) -> Value {
    let mut row = Vec::new();
    if allow {
        row.push(json!({ "text": format!("✅ {}", t("Allow")), "callback_data": format!("a:{request_id}:{nonce}") }));
    }
    row.push(json!({ "text": format!("❌ {}", t("Deny")), "callback_data": format!("d:{request_id}:{nonce}") }));
    json!({ "inline_keyboard": [row] })
}

/// How a request ended, for the line added under its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    PhoneAllowed,
    PhoneDenied,
    ComputerAllowed,
    ComputerDenied,
    /// Answered on the island as a question, in the terminal, or the turn moved on.
    Computer,
    TimedOut,
}

impl Ending {
    pub fn line(self) -> String {
        match self {
            Ending::PhoneAllowed => format!("✅ {}", t("Allowed from the phone")),
            Ending::PhoneDenied => format!("❌ {}", t("Denied from the phone")),
            Ending::ComputerAllowed => format!("💻 {}", t("Allowed on the computer")),
            Ending::ComputerDenied => format!("💻 {}", t("Denied on the computer")),
            Ending::Computer => format!("💻 {}", t("Answered on the computer")),
            Ending::TimedOut => format!("⌛ {}", t("Time ran out: the terminal takes over")),
        }
    }
}

struct Pairing {
    code: String,
    expires_at: u64,
    tries: u32,
}

struct Card {
    nonce: String,
    text: String,
    allow: bool,
    /// None until Telegram has said which message it is.
    message_id: Option<i64>,
    /// Settled while the message was still on its way: edited as soon as it lands.
    ending: Option<Ending>,
}

/// What a `/start` message did.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// This chat is now the owner.
    Paired,
    /// Not a pairing: a wrong code, no code shown, a group, or already paired.
    Ignored,
}

/// What the owner may ask the bot in their private chat.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Tasks,
    /// A plain to-do; empty when nothing followed the command.
    Add(String),
}

/// What a button press does.
#[derive(Debug, PartialEq, Eq)]
pub enum Press {
    /// Not the owner: nothing happens, nothing is said.
    Stranger,
    /// The owner pressed a button of a request that is over (or never was).
    Stale,
    /// The first valid press: answer the agent with it.
    Decide { request_id: String, allow: bool, message_id: Option<i64>, text: String },
}

/// An edit to make: this message gets this text and loses its buttons.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub message_id: i64,
    pub text: String,
}

/// The card's text with how it ended.
pub fn settled_text(text: &str, ending: Ending) -> String {
    format!("{text}\n\n<b>{}</b>", esc(&ending.line()))
}

#[derive(Default)]
pub struct Book {
    /// The one chat that may answer (a private chat: its id is the user's).
    pub owner: Option<i64>,
    pairing: Option<Pairing>,
    cards: HashMap<String, Card>,
}

impl Book {
    pub fn new(owner: Option<i64>) -> Book {
        Book { owner, ..Book::default() }
    }

    /// Shows `code` in Settings. Only while no chat owns the bot.
    pub fn start_pairing(&mut self, code: String, now: u64) -> bool {
        if self.owner.is_some() {
            return false;
        }
        self.pairing = Some(Pairing { code, expires_at: now + CODE_TTL_MS, tries: 0 });
        true
    }

    /// The code to show, while it can still be used.
    pub fn pairing_code(&self, now: u64) -> Option<&str> {
        self.pairing.as_ref().filter(|p| now < p.expires_at && p.tries < MAX_CODE_TRIES).map(|p| p.code.as_str())
    }

    /// Whether polling has anything to wait for: an owner's presses or a pairing.
    pub fn listening(&self, now: u64) -> bool {
        self.owner.is_some() || self.pairing_code(now).is_some()
    }

    pub fn unpair(&mut self) {
        self.owner = None;
        self.pairing = None;
        self.cards.clear();
    }

    /// A text message: `/start <code>` (or `/start@bot <code>`) from a private chat pairs it.
    pub fn on_message(&mut self, chat_id: i64, private: bool, text: &str, now: u64) -> Start {
        let mut words = text.split_whitespace();
        let command = words.next().unwrap_or_default();
        if command != "/start" && !command.starts_with("/start@") {
            return Start::Ignored;
        }
        let given = words.next().unwrap_or_default().to_ascii_uppercase();
        if self.owner.is_some() || !private || given.is_empty() {
            return Start::Ignored;
        }
        if self.pairing_code(now).is_none() {
            self.pairing = None;
            return Start::Ignored;
        }
        let pairing = self.pairing.as_mut().expect("checked above");
        if constant_eq(pairing.code.as_bytes(), given.as_bytes()) {
            self.pairing = None;
            self.owner = Some(chat_id);
            return Start::Paired;
        }
        pairing.tries += 1;
        if pairing.tries >= MAX_CODE_TRIES {
            self.pairing = None;
        }
        Start::Ignored
    }

    /// `/status`, `/tasks` or `/add <text>` (also `/add@bot …`) from the owner, in
    /// their own private chat; from anyone else, or anywhere else, nothing.
    pub fn command(&self, from: i64, chat: i64, private: bool, text: &str) -> Option<Command> {
        if self.owner != Some(from) || from != chat || !private {
            return None;
        }
        let text = text.trim();
        let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let word = word.split_once('@').map_or(word, |(w, _)| w).to_ascii_lowercase();
        match word.as_str() {
            "/status" => Some(Command::Status),
            "/tasks" => Some(Command::Tasks),
            "/add" => Some(Command::Add(rest.trim().to_string())),
            _ => None,
        }
    }

    /// A card about to be sent. Returns its buttons.
    pub fn add_card(&mut self, request_id: &str, nonce: &str, draft: &Draft) -> (String, Value) {
        let text = card_text(draft);
        let allow = !draft.truncated;
        self.cards.insert(
            request_id.to_string(),
            Card { nonce: nonce.to_string(), text: text.clone(), allow, message_id: None, ending: None },
        );
        (text, keyboard(request_id, nonce, allow))
    }

    /// Telegram took the card. If the request ended meanwhile, the edit to make now.
    pub fn sent(&mut self, request_id: &str, message_id: i64) -> Option<Edit> {
        let card = self.cards.get_mut(request_id)?;
        card.message_id = Some(message_id);
        let ending = card.ending?;
        let card = self.cards.remove(request_id)?;
        Some(Edit { message_id, text: settled_text(&card.text, ending) })
    }

    /// Telegram refused the card or could not be reached: forget it.
    pub fn not_sent(&mut self, request_id: &str) {
        self.cards.remove(request_id);
    }

    /// The request ended on the computer (or ran out). The edit to make, if the
    /// message is already there; else it is made when it lands.
    pub fn settle(&mut self, request_id: &str, ending: Ending) -> Option<Edit> {
        let card = self.cards.get_mut(request_id)?;
        match card.message_id {
            Some(message_id) => {
                let card = self.cards.remove(request_id)?;
                Some(Edit { message_id, text: settled_text(&card.text, ending) })
            }
            None => {
                card.ending = Some(ending);
                None
            }
        }
    }

    /// A button press: `from` pressed it in `chat`, with `data` as its callback_data.
    pub fn on_press(&mut self, from: i64, chat: i64, data: &str) -> Press {
        if self.owner != Some(from) || from != chat {
            return Press::Stranger;
        }
        let mut parts = data.splitn(3, ':');
        let (Some(kind), Some(request_id), Some(nonce)) = (parts.next(), parts.next(), parts.next()) else {
            return Press::Stale;
        };
        let allow = match kind {
            "a" => true,
            "d" => false,
            _ => return Press::Stale,
        };
        let Some(card) = self.cards.get(request_id) else { return Press::Stale };
        if !constant_eq(card.nonce.as_bytes(), nonce.as_bytes())
            || card.ending.is_some()
            || (allow && !card.allow)
        {
            return Press::Stale;
        }
        // Taken: a request is answered once.
        let card = self.cards.remove(request_id).expect("checked above");
        Press::Decide { request_id: request_id.to_string(), allow, message_id: card.message_id, text: card.text }
    }

    #[cfg(test)]
    fn has_card(&self, request_id: &str) -> bool {
        self.cards.contains_key(request_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: i64 = 4242;

    fn payload(command: &str) -> Value {
        json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "s1",
            "cwd": "C:\\Users\\me\\projects\\shop",
            "tool_name": "Bash",
            "tool_input": { "command": command, "description": "x" },
        })
    }

    fn paired() -> Book {
        let mut book = Book::default();
        book.start_pairing("ABCD2345".into(), 0);
        assert_eq!(book.on_message(OWNER, true, "/start ABCD2345", 1), Start::Paired);
        book
    }

    #[test]
    fn a_card_shows_the_agent_project_tool_and_exact_command() {
        let d = draft(&payload("rm -rf build && echo <done>"), false).unwrap();
        assert_eq!(d.agent, "Claude Code");
        assert_eq!(d.project, "shop");
        assert_eq!(d.tool, "Bash");
        assert_eq!(d.target.as_deref(), Some("rm -rf build && echo <done>"));
        let text = card_text(&d);
        assert!(text.contains("<pre>rm -rf build &amp;&amp; echo &lt;done&gt;</pre>"), "{text}");
        assert!(text.contains("shop") && text.contains("Bash"));

        let mut codex = payload("ls");
        codex["coucou_agent"] = json!("codex");
        codex["cwd"] = json!("/home/me/api/");
        let d = draft(&codex, false).unwrap();
        assert_eq!((d.agent.as_str(), d.project.as_str()), ("Codex", "api"));

        let mut edit = payload("");
        edit["tool_name"] = json!("Edit");
        edit["tool_input"] = json!({ "file_path": "C:\\p\\src\\main.rs", "old_string": "a", "new_string": "b" });
        assert_eq!(draft(&edit, false).unwrap().target.as_deref(), Some("C:\\p\\src\\main.rs"));
    }

    #[test]
    fn hiding_the_command_keeps_the_tool_and_project_only() {
        let d = draft(&payload("curl https://secret.example/?token=abc"), true).unwrap();
        assert_eq!(d.target, None);
        assert!(d.hidden);
        let text = card_text(&d);
        assert!(!text.contains("secret.example"), "{text}");
        assert!(text.contains("Bash") && text.contains("shop"));
    }

    #[test]
    fn questions_and_payloads_without_a_tool_get_no_card() {
        let mut q = payload("");
        q["tool_name"] = json!("AskUserQuestion");
        assert!(draft(&q, false).is_none());
        assert!(draft(&json!({ "cwd": "/x" }), false).is_none());
    }

    #[test]
    fn a_command_too_long_to_show_can_only_be_denied() {
        let long = "x".repeat(MAX_TARGET_CHARS + 10);
        let d = draft(&payload(&long), false).unwrap();
        assert!(d.truncated);
        assert_eq!(d.target.as_ref().unwrap().chars().count(), MAX_TARGET_CHARS);
        let mut book = paired();
        let (_, kb) = book.add_card("1-1", "n0nce", &d);
        let row = kb["inline_keyboard"][0].as_array().unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0]["callback_data"], "d:1-1:n0nce");
        // A forged Allow is refused and the card stays for the island.
        assert_eq!(book.on_press(OWNER, OWNER, "a:1-1:n0nce"), Press::Stale);
        assert!(matches!(book.on_press(OWNER, OWNER, "d:1-1:n0nce"), Press::Decide { allow: false, .. }));
    }

    #[test]
    fn callback_data_fits_telegrams_64_bytes() {
        let (_, kb) = paired().add_card("4294967295-18446744073709551615", &"f".repeat(24), &draft(&payload("ls"), false).unwrap());
        for b in kb["inline_keyboard"][0].as_array().unwrap() {
            assert!(b["callback_data"].as_str().unwrap().len() <= 64);
        }
    }

    #[test]
    fn pairing_needs_the_shown_code_from_a_private_chat() {
        let mut book = Book::default();
        // Nothing shown: no code pairs.
        assert_eq!(book.on_message(1, true, "/start ABCD2345", 0), Start::Ignored);
        assert!(book.start_pairing("ABCD2345".into(), 0));
        assert!(book.listening(0));
        assert_eq!(book.on_message(1, false, "/start ABCD2345", 1), Start::Ignored, "a group");
        assert_eq!(book.on_message(1, true, "/start", 1), Start::Ignored);
        assert_eq!(book.on_message(1, true, "hello", 1), Start::Ignored);
        assert_eq!(book.owner, None);
        // Case does not matter, the bot's name after /start may be there.
        assert_eq!(book.on_message(OWNER, true, "/start@lulu_bot abcd2345", 2), Start::Paired);
        assert_eq!(book.owner, Some(OWNER));
        // Used once: nobody else can take over, not even with the same code.
        assert_eq!(book.pairing_code(3), None);
        assert_eq!(book.on_message(7, true, "/start ABCD2345", 3), Start::Ignored);
        assert_eq!(book.owner, Some(OWNER));
        assert!(!book.start_pairing("ZZZZ2345".into(), 4), "no new code while paired");
    }

    #[test]
    fn a_code_expires_and_wrong_tries_burn_it() {
        let mut book = Book::default();
        book.start_pairing("ABCD2345".into(), 0);
        assert_eq!(book.on_message(OWNER, true, "/start ABCD2345", CODE_TTL_MS), Start::Ignored);
        assert_eq!(book.owner, None);
        assert!(!book.listening(CODE_TTL_MS));

        book.start_pairing("ABCD2345".into(), 0);
        for _ in 0..MAX_CODE_TRIES {
            assert_eq!(book.on_message(9, true, "/start WRONG234", 1), Start::Ignored);
        }
        assert_eq!(book.pairing_code(1), None);
        assert_eq!(book.on_message(OWNER, true, "/start ABCD2345", 2), Start::Ignored, "burnt");
    }

    #[test]
    fn only_the_owner_in_their_private_chat_gives_commands() {
        let book = paired();
        let ask = |from: i64, chat: i64, private: bool, text: &str| book.command(from, chat, private, text);
        assert_eq!(ask(OWNER, OWNER, true, "/status"), Some(Command::Status));
        assert_eq!(ask(OWNER, OWNER, true, "/STATUS@lulu_bot"), Some(Command::Status));
        assert_eq!(ask(OWNER, OWNER, true, " /tasks "), Some(Command::Tasks));
        assert_eq!(ask(OWNER, OWNER, true, "/add buy milk  and bread "), Some(Command::Add("buy milk  and bread".into())));
        assert_eq!(ask(OWNER, OWNER, true, "/add@lulu_bot call Ali"), Some(Command::Add("call Ali".into())));
        assert_eq!(ask(OWNER, OWNER, true, "/add"), Some(Command::Add(String::new())));
        assert_eq!(ask(OWNER, OWNER, true, "/add
second line"), Some(Command::Add("second line".into())));
        // Not commands.
        for text in ["", "status", "/statusx", "/addx milk", "hello /status", "/start ABCD2345", "/run npm test"] {
            assert_eq!(ask(OWNER, OWNER, true, text), None, "{text:?}");
        }
        // Nobody else, and nowhere else: a stranger, a group the owner is in, a forged chat.
        assert_eq!(ask(99, 99, true, "/status"), None);
        assert_eq!(ask(OWNER, -100, false, "/status"), None);
        assert_eq!(ask(OWNER, -100, true, "/status"), None);
        assert_eq!(ask(OWNER, OWNER, false, "/status"), None);
        assert_eq!(Book::default().command(OWNER, OWNER, true, "/status"), None, "unpaired");
    }

    #[test]
    fn codes_use_the_whole_alphabet_and_nothing_else() {
        let code = code_from(&[0, 1, 31, 32, 63, 200, 255, 7]);
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        assert_eq!(&code[..3], "AB9");
    }

    #[test]
    fn only_the_owner_with_the_right_nonce_decides_and_only_once() {
        let mut book = paired();
        book.add_card("7-1", "goodnonce", &draft(&payload("npm test"), false).unwrap());
        book.sent("7-1", 555);
        // Someone else, or the owner's id in another chat.
        assert_eq!(book.on_press(99, 99, "a:7-1:goodnonce"), Press::Stranger);
        assert_eq!(book.on_press(OWNER, -100, "a:7-1:goodnonce"), Press::Stranger);
        // Wrong nonce, unknown request, garbage.
        assert_eq!(book.on_press(OWNER, OWNER, "a:7-1:badnonce"), Press::Stale);
        assert_eq!(book.on_press(OWNER, OWNER, "a:7-2:goodnonce"), Press::Stale);
        assert_eq!(book.on_press(OWNER, OWNER, "x:7-1:goodnonce"), Press::Stale);
        assert_eq!(book.on_press(OWNER, OWNER, "nonsense"), Press::Stale);
        assert!(book.has_card("7-1"), "none of these touched the card");

        let Press::Decide { request_id, allow, message_id, .. } = book.on_press(OWNER, OWNER, "a:7-1:goodnonce") else {
            panic!("the owner's press decides");
        };
        assert_eq!((request_id.as_str(), allow, message_id), ("7-1", true, Some(555)));
        // A second press (a double tap, or Deny after Allow) finds nothing.
        assert_eq!(book.on_press(OWNER, OWNER, "d:7-1:goodnonce"), Press::Stale);
        // And the computer finishing afterwards has nothing to edit: the phone already did.
        assert_eq!(book.settle("7-1", Ending::ComputerAllowed), None);
    }

    #[test]
    fn the_island_answering_first_closes_the_card_and_a_late_press_finds_nothing() {
        let mut book = paired();
        book.add_card("8-1", "n", &draft(&payload("ls"), false).unwrap());
        book.sent("8-1", 77);
        let edit = book.settle("8-1", Ending::ComputerDenied).expect("the card is edited");
        assert_eq!(edit.message_id, 77);
        assert!(edit.text.contains("ls") && edit.text.ends_with(&format!("<b>{}</b>", esc(&Ending::ComputerDenied.line()))));
        assert_eq!(book.on_press(OWNER, OWNER, "a:8-1:n"), Press::Stale);
    }

    #[test]
    fn a_request_that_ends_while_its_card_is_on_its_way_is_edited_when_it_lands() {
        let mut book = paired();
        book.add_card("9-1", "n", &draft(&payload("ls"), false).unwrap());
        assert_eq!(book.settle("9-1", Ending::TimedOut), None, "no message yet");
        // No press can decide it meanwhile.
        assert_eq!(book.on_press(OWNER, OWNER, "a:9-1:n"), Press::Stale);
        let edit = book.sent("9-1", 12).expect("edited on arrival");
        assert!(edit.text.contains(&esc(&Ending::TimedOut.line())));
        assert!(!book.has_card("9-1"));
        // A card that never went: forgotten, nothing to edit.
        book.add_card("9-2", "n", &draft(&payload("ls"), false).unwrap());
        book.not_sent("9-2");
        assert_eq!(book.settle("9-2", Ending::Computer), None);
    }

    #[test]
    fn unpairing_forgets_the_owner_and_every_card() {
        let mut book = paired();
        book.add_card("1-1", "n", &draft(&payload("ls"), false).unwrap());
        book.unpair();
        assert_eq!(book.owner, None);
        assert!(!book.listening(0));
        assert_eq!(book.on_press(OWNER, OWNER, "a:1-1:n"), Press::Stranger);
    }
}
