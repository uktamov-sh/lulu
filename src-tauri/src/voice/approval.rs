// "OK LuLu, allow" / "OK LuLu, deny": answering the permission card that is
// waiting, by voice (Settings → Agents → Answer permission requests by voice,
// off by default).
//
// Only what follows « OK LuLu » is read, only while a card waits, and only a
// short sentence made of the one word: "allow", "yes, allow it", "deny". A
// longer sentence, or one with both words in it, goes to the assistant as any
// other question. Allow is refused for a command that looks destructive, for
// one cut short, and when several cards wait (pipe::voice_answer): those need
// the buttons. Deny always works. The audio never leaves the computer; what is
// written here is only a word.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
}

/// Longest sentence taken as an answer, in words.
const MAX_WORDS: usize = 4;

/// Whole words (Cyrillic, Latin) of the sentence, lower case, no punctuation.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == 'ʻ' || c == 'ʼ'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

const ALLOW_STEMS: &[&str] = &["разреш", "одобр", "allow", "approve", "ruxsat", "tasdiq"];
const DENY_STEMS: &[&str] = &["отказ", "отклон", "запрет", "deny", "denied", "reject", "refuse", "rad"];
const ALLOW_WORDS: &[&str] = &["да", "yes", "ha"];
const DENY_WORDS: &[&str] = &["нет", "no", "yoq", "yo'q"];

/// What the sentence answers, if it is only that.
pub fn verdict(text: &str) -> Option<Verdict> {
    let words = words(text);
    if words.is_empty() || words.len() > MAX_WORDS {
        return None;
    }
    let hits = |stems: &[&str], exact: &[&str]| {
        words.iter().any(|w| stems.iter().any(|s| w.starts_with(s)) || exact.contains(&w.as_str()))
    };
    // "rad" is a stem of the Uzbek "rad etish" but also a prefix of other words:
    // only the word itself counts.
    let deny_stem = |w: &String| DENY_STEMS.iter().any(|s| if *s == "rad" { w == "rad" } else { w.starts_with(s) });
    let allow = hits(ALLOW_STEMS, ALLOW_WORDS);
    let deny = words.iter().any(deny_stem) || words.iter().any(|w| DENY_WORDS.contains(&w.as_str()));
    match (allow, deny) {
        (true, false) => Some(Verdict::Allow),
        (false, true) => Some(Verdict::Deny),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_sentences_with_one_of_the_words_answer() {
        assert_eq!(verdict("разрешить"), Some(Verdict::Allow));
        assert_eq!(verdict("Да, разрешаю."), Some(Verdict::Allow));
        assert_eq!(verdict("allow it"), Some(Verdict::Allow));
        assert_eq!(verdict("Yes."), Some(Verdict::Allow));
        assert_eq!(verdict("ruxsat beraman"), Some(Verdict::Allow));
        assert_eq!(verdict("отказать"), Some(Verdict::Deny));
        assert_eq!(verdict("Нет, отклонить!"), Some(Verdict::Deny));
        assert_eq!(verdict("deny"), Some(Verdict::Deny));
        assert_eq!(verdict("rad etish"), Some(Verdict::Deny));
    }

    #[test]
    fn long_mixed_or_unrelated_sentences_are_questions_for_the_assistant() {
        assert_eq!(verdict(""), None);
        assert_eq!(verdict("what is the weather"), None);
        assert_eq!(verdict("разреши мне, пожалуйста, открыть почту"), None);
        assert_eq!(verdict("разрешить или отказать"), None);
        assert_eq!(verdict("allow and deny"), None);
        assert_eq!(verdict("radio"), None);
        assert_eq!(verdict("нетрудно"), None);
        assert_eq!(verdict("data"), None);
    }
}
