// The island chat's memory of earlier conversations (Settings → Chat →
// Remember earlier conversations, off by default), as MaoMao keeps one: a short
// note about the user — facts, preferences, ongoing work — rewritten by the
// chat's AI each time a conversation ends ("New chat"), at most MAX_CHARS long.
//
// The note is %APPDATA%\LuLu\memory.txt, nowhere else. It rides along with the
// first question of each new island conversation only: never with the phone's
// chat, never with a permission request's explanation. "Forget" deletes it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::chat::{self, Chat};
use crate::settings::Settings;

/// Longest note kept, in characters.
const MAX_CHARS: usize = 1200;
/// Longest conversation sent to be folded in, in characters.
const MAX_TRANSCRIPT: usize = 12_000;

static ON: AtomicBool = AtomicBool::new(false);

fn path() -> PathBuf {
    crate::settings::config_dir().join("memory.txt")
}

/// The option was turned on or off (at launch and on every save).
pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// The note, when the option is on and there is one.
pub fn remembered() -> Option<String> {
    if !ON.load(Ordering::Relaxed) {
        return None;
    }
    std::fs::read_to_string(path()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The first question of a new conversation, with the note in front of it.
pub fn with_note(note: &str, query: &str) -> String {
    format!("What you remember about me from earlier conversations (use it when it helps, don't recite it):\n{note}\n\n{query}")
}

/// "user: …" / "assistant: …" lines of a conversation, the newest kept when long.
fn transcript(plain: &[Value]) -> String {
    let lines: Vec<String> = plain
        .iter()
        .filter_map(|m| Some(format!("{}: {}", m.get("role")?.as_str()?, m.get("content")?.as_str()?)))
        .collect();
    let text = lines.join("\n");
    let count = text.chars().count();
    if count <= MAX_TRANSCRIPT {
        return text;
    }
    text.chars().skip(count - MAX_TRANSCRIPT).collect()
}

fn fold_prompt(note: &str, conversation: &str) -> String {
    format!(
        "You keep a short private note about the user of a desktop assistant, to remember across conversations.\n\
Current note:\n{}\n\nConversation that just ended:\n{conversation}\n\n\
Rewrite the note with what is worth remembering: who the user is, their preferences, ongoing projects and decisions. \
Drop what is no longer true. Never keep passwords, keys, tokens, card or account numbers, or health details. \
At most {MAX_CHARS} characters, in the user's language, plain text, short lines. Answer with the note only.",
        if note.is_empty() { "(empty)" } else { note }
    )
}

/// A conversation ended: the chat's AI folds it into the note. Quietly does
/// nothing when the option is off, the conversation is empty or the AI fails.
pub async fn fold_in(settings: Settings, plain: Vec<Value>) {
    if !ON.load(Ordering::Relaxed) || plain.len() < 2 {
        return;
    }
    let note = std::fs::read_to_string(path()).unwrap_or_default();
    let provider = if settings.chat_provider.is_empty() { chat::ANTHROPIC.to_string() } else { settings.chat_provider.clone() };
    let model = chat::model_for(&settings, &provider);
    let prompt = fold_prompt(note.trim(), &transcript(&plain));
    match chat::send_for_phone(&settings, &Chat::default(), &provider, &model, prompt, |_| {}).await {
        Ok(reply) => {
            let text: String = reply.text.trim().chars().take(MAX_CHARS).collect();
            if text.is_empty() {
                return;
            }
            let dir = crate::settings::config_dir();
            if let Err(err) = crate::platform::ensure_private_dir(&dir).and_then(|_| std::fs::write(path(), text)) {
                crate::log::line(format!("memory: could not save: {err}"));
            }
        }
        Err(err) => crate::log::line(format!("memory: not updated: {}", err.chars().take(120).collect::<String>())),
    }
}

/// What LuLu remembers, for Settings to show; empty when nothing.
#[tauri::command]
pub fn memory_text() -> String {
    std::fs::read_to_string(path()).unwrap_or_default()
}

/// "Forget": the note is deleted.
#[tauri::command]
pub fn memory_forget() {
    let _ = std::fs::remove_file(path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_transcript_keeps_roles_and_the_newest_text() {
        let plain = vec![json!({ "role": "user", "content": "hi" }), json!({ "role": "assistant", "content": "hello" })];
        assert_eq!(transcript(&plain), "user: hi\nassistant: hello");
        let long = vec![json!({ "role": "user", "content": "x".repeat(20_000) + "END" })];
        let t = transcript(&long);
        assert_eq!(t.chars().count(), MAX_TRANSCRIPT);
        assert!(t.ends_with("END"));
    }

    #[test]
    fn the_prompt_forbids_secrets_and_the_note_rides_in_front_of_the_question() {
        let p = fold_prompt("", "user: my card is 4111");
        assert!(p.contains("(empty)"));
        assert!(p.contains("Never keep passwords"));
        let q = with_note("Name: Shams", "What next?");
        assert!(q.starts_with("What you remember"));
        assert!(q.ends_with("\n\nWhat next?"));
    }
}
