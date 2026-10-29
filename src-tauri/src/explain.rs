// "LuLu's brain": one plain line about a permission request — what the agent
// wants and whether it is routine (✅), worth a look (⚠️) or risky (⛔) — from
// the AI chosen in Settings → Chat, in LuLu's language. Asked by the island
// for each card only while Settings → Agents → "Explain requests" is on (off by
// default): the tool, the project folder and the command go to that AI.
//
// Each request is a conversation of its own (a fresh `Chat`): nothing of the
// island's chat goes with it, and nothing of it stays.

use tauri::State;

use crate::chat::{self, Chat};
use crate::Shared;

/// Longest command or path sent, in characters.
const MAX_DETAIL: usize = 1500;
/// Longest line kept, in characters.
const MAX_LINE: usize = 160;

fn prompt(tool: &str, detail: &str, cwd: &str, language: &str) -> String {
    let detail: String = detail.chars().take(MAX_DETAIL).collect();
    format!(
        "An AI coding agent asks permission to use a tool on my computer.\n\
Tool: {tool}\nProject folder: {cwd}\nDetails: {detail}\n\n\
In {language}, in one sentence of at most 15 words, tell a non-programmer what it wants to do. \
Start with ✅ if it is routine and safe, ⚠️ if it is worth a look, ⛔ if it could destroy data, \
leak secrets or reach the internet in a risky way. Plain text only, no Markdown, nothing else."
    )
}

/// The first line of an answer, without Markdown marks, at most MAX_LINE characters.
fn tidy(answer: &str) -> String {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches(|c| c == '*' || c == '_' || c == '`' || c == '"');
    let mut out: String = line.chars().take(MAX_LINE).collect();
    if line.chars().count() > MAX_LINE {
        out.push('…');
    }
    out
}

#[tauri::command]
pub async fn explain_request(
    shared: State<'_, Shared>,
    tool: String,
    detail: String,
    cwd: String,
    language: String,
) -> Result<String, String> {
    let settings = shared.settings.lock().unwrap().clone();
    if !settings.explain_requests {
        return Err("off".into());
    }
    let provider = if settings.chat_provider.is_empty() { chat::ANTHROPIC.to_string() } else { settings.chat_provider.clone() };
    let model = chat::model_for(&settings, &provider);
    let reply = chat::send_for_phone(
        &settings,
        &Chat::default(),
        &provider,
        &model,
        prompt(&tool, &detail, &cwd, &language),
        |_| {},
    )
    .await?;
    let line = tidy(&reply.text);
    if line.is_empty() {
        return Err("empty".into());
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_language_and_caps_the_command() {
        let long = "x".repeat(5000);
        let p = prompt("Bash", &long, "C:\\p", "Oʻzbekcha");
        assert!(p.contains("In Oʻzbekcha,"));
        assert!(p.contains("Tool: Bash"));
        assert!(p.len() < 2500);
    }

    #[test]
    fn only_the_first_line_is_kept_without_markdown_and_capped() {
        assert_eq!(tidy("\n**✅ Runs the tests.**\nMore text"), "✅ Runs the tests.");
        let long = "y".repeat(400);
        let line = tidy(&long);
        assert_eq!(line.chars().count(), MAX_LINE + 1);
        assert!(line.ends_with('…'));
        assert_eq!(tidy("  \n "), "");
    }
}
