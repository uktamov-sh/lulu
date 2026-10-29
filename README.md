# LuLu for Windows

LuLu is a small panda who lives at the top of your screen and keeps an eye on
your AI coding agents (Claude Code, Codex, Cursor, Gemini CLI and others): she
shows what they read, edit and run, and lets you **Allow** or **Deny** their
permission requests without leaving what you are doing.

Interface languages: English, Oʻzbekcha, Özbekça (new alphabet), Русский and
ten more.

## Download

The installers are on the [Releases](https://github.com/uktamov-sh/lulu/releases)
page: `LuLu-Windows-<version>-setup.exe` (or the `.msi`). LuLu checks that page
for new versions and offers to update itself.

## Build

```
npm install
npm test                      # TypeScript tests
cargo test --workspace        # Rust tests (needs lulu-hook built once: cargo build --release -p lulu-hook)
npm run tauri build           # installers in target/release/bundle (NSIS .exe and .msi)
```

Development preview of the character: `npx vite`, then open
`http://localhost:1420/dev/panda-preview.html`.

Regenerate the art: `node scripts/gen-icons.mjs` (app icon) and
`python scripts/gen-sounds.py` (the 29 sounds) — both are drawn or synthesised
in code.

## What LuLu writes on your computer

- `%LOCALAPPDATA%\LuLu` — the hook relay (`bin\lulu-hook.exe`), the log, the history.
- `%APPDATA%\LuLu` — preferences.
- `~\.claude\settings.json` — only after your click in Settings: LuLu's hook
  entries, with a dated backup and the exact diff shown first.

No telemetry. Network requests go only to the services you configure.

## Approvals in Telegram

Settings → Agents → Telegram sends each permission request whose card is up on
the island to your own Telegram bot, with **Allow** / **Deny** buttons. Off by
default.

1. In Telegram, create a bot with @BotFather (`/newbot`) and paste its token in
   Settings. LuLu checks it once with Telegram and keeps it in the Windows
   Credential Manager — never on disk, never in the log.
2. LuLu shows a one-time code (10 minutes, five wrong tries burn it). Send
   `/start <code>` to the bot from your phone: that chat becomes the only one
   whose buttons count.

How it behaves:

- There is no LuLu server. While it is on and paired, LuLu polls the Bot API
  itself (`getUpdates`, HTTPS to `api.telegram.org`); off, nothing is sent.
- The card shows the agent, the project, the tool and the exact command or file
  — all of which goes through Telegram's servers. "Hide the command" keeps only
  the tool and the project. A command too long for one message can only be
  denied from the phone.
- Whoever answers first — the island, the terminal or the phone — decides; the
  other side is updated ("Allowed from the phone", "Answered on the computer",
  "Time ran out"). Each button carries the request's id and a random nonce and
  counts once; old or repeated presses are ignored.
- Telegram being slow or down never holds the agent: the wait stays the
  island's (108 s), and the card is only sent once the island has the request
  on screen. Questions (AskUserQuestion) stay on the computer, and so do the
  Claude desktop app's sessions unless Settings → Agents → "Claude app sessions
  ask here" is on.

## Origin and licence

LuLu is built on the open-source code of **Coucou** by Louis Raillé
(<https://github.com/Louis-CFM/coucou>, MIT License; its copyright notice is
kept in [`LICENSE`](LICENSE)). Coucou's name, its Mochi character, icon and
sounds are not used: LuLu has her own character, icon and sounds, drawn and
synthesised in this repository.

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/).

- Every release is built by GitHub Actions from the code in this repository
  ([`.github/workflows/release.yml`](.github/workflows/release.yml)); nothing
  built on a personal computer is signed.
- Committers and reviewers: [uktamov-sh](https://github.com/uktamov-sh).
  Approver of each signed release: [uktamov-sh](https://github.com/uktamov-sh).
- Privacy: LuLu sends nothing anywhere unless you turn it on. The Telegram
  approvals talk only to `api.telegram.org` with your own bot; update checks
  read this repository's Releases page; the chat sends your messages only to
  the AI service you configure. There is no telemetry.
