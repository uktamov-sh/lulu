// Settings → Agents → Telegram: permission requests on the phone, through the
// user's own bot (src-tauri/src/telegram). Off until turned on. The token goes
// straight to the credential store through Rust and never comes back here; the
// pairing code is shown until a chat sends it to the bot, and only that chat
// may answer afterwards.

import { Bridge, onEvent, type TelegramStatus } from "../core/bridge";
import { h } from "../views/dom";
import { t } from "../i18n/i18n";

type Toggle = (on: boolean, onChange: (v: boolean) => void) => HTMLElement;

const TELEGRAM_BLUE = "#2AABEE";

export function telegramSection(toggle: Toggle, keyStore: string): HTMLElement {
  let view: TelegramStatus | null = null;
  let busy = false;
  let error = "";

  const enableSwitch = toggle(false, (on) => void setOptions({ enabled: on }));
  const hideSwitch = toggle(false, (on) => void setOptions({ hideCommand: on }));
  const body = h("div", { style: "display:flex;flex-direction:column;gap:10px" });
  const steps = h("div", { style: "display:flex;flex-direction:column;gap:10px" });
  const tokenInput = h("input", {
    type: "password", placeholder: t("Bot token"), autocomplete: "off", spellcheck: "false",
    style: "flex:1 1 auto;min-width:0",
  }) as HTMLInputElement;
  const connectBtn = h("button", { class: "primary", text: t("Connect") }) as HTMLButtonElement;

  const switchState = (el: HTMLElement, on: boolean) => {
    el.classList.toggle("on", on);
    el.setAttribute("aria-pressed", String(on));
  };

  async function setOptions(change: Partial<{ enabled: boolean; hideCommand: boolean }>) {
    const v = view;
    if (!v) return;
    const next = { enabled: v.enabled, hideCommand: v.hideCommand, ...change };
    apply(await Bridge.telegramSetOptions(next.enabled, next.hideCommand));
  }

  /** Runs a step that may fail with a message from Rust, shown under the steps. */
  async function attempt(step: () => Promise<TelegramStatus>) {
    busy = true;
    error = "";
    draw();
    try {
      apply(await step());
    } catch (err) {
      error = String(err).replace(/^Error:\s*/, "");
    }
    busy = false;
    draw();
  }

  function connect() {
    const token = tokenInput.value.trim();
    if (!token || busy) return;
    void attempt(async () => {
      const status = await Bridge.telegramConnect(token);
      tokenInput.value = "";
      return status;
    });
  }
  connectBtn.addEventListener("click", connect);
  tokenInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") connect();
  });

  const disconnectBtn = () => h("button", {
    class: "danger", text: t("Disconnect"),
    onclick: () => void Bridge.telegramDisconnect().then(apply),
  });

  function draw() {
    const v = view;
    if (!v) return;
    switchState(enableSwitch, v.enabled);
    switchState(hideSwitch, v.hideCommand);
    body.style.display = v.enabled ? "" : "none";
    const bot = v.bot ? `@${v.bot}` : t("your bot");
    const parts: HTMLElement[] = [];

    if (v.paired) {
      parts.push(h("div", { class: "row", style: "gap:8px" },
        h("i", { class: "dot", style: "background:#22c55e" }),
        h("span", { style: "font-size:12.5px", text: t("Connected to {bot}. Only your chat can answer.", { bot }) }),
        h("span", { class: "spacer" }),
        disconnectBtn(),
      ));
    } else if (!v.hasToken) {
      connectBtn.disabled = busy;
      connectBtn.textContent = busy ? t("Checking the token…") : t("Connect");
      parts.push(
        h("div", { class: "hint", text: t("1. In Telegram, open @BotFather, send /newbot and copy the token it gives you.") }),
        h("div", { class: "row" }, tokenInput, connectBtn),
        h("div", { class: "hint", text: t("The token is kept in {store}, never on disk.", { store: keyStore }) }),
      );
    } else if (v.code) {
      parts.push(
        h("div", { class: "hint", text: t("2. From your phone, send this to {bot}:", { bot }) }),
        h("div", { class: "row", style: "gap:8px" },
          h("code", { style: "font-size:15px;letter-spacing:1px;user-select:all", text: `/start ${v.code}` }),
          h("span", { class: "spacer" }),
          h("button", { class: "primary", text: t("Open in Telegram"), onclick: () => void Bridge.telegramOpenBot() }),
        ),
        h("div", { class: "hint", text: t("The code works once, for 10 minutes. Only the chat that sends it can answer.") }),
        h("div", { class: "row" }, h("span", { class: "spacer" }), disconnectBtn()),
      );
    } else {
      parts.push(h("div", { class: "row", style: "gap:8px" },
        h("span", { style: "font-size:12.5px", text: t("2. Get a code and send it to {bot} from your phone.", { bot }) }),
        h("span", { class: "spacer" }),
        h("button", { class: "primary", text: t("New code"), onclick: () => void attempt(Bridge.telegramNewCode) }),
        disconnectBtn(),
      ));
    }
    if (error) parts.push(h("div", { class: "notice err", text: error }));
    steps.replaceChildren(...parts);
  }

  body.append(
    steps,
    h("div", { class: "row" }, h("label", { text: t("Hide the command: show only the tool and the project") }), hideSwitch),
  );

  const section = h(
    "section",
    {},
    h("h2", {}, h("i", { class: "dot", style: `background:${TELEGRAM_BLUE}` }), h("span", { text: "Telegram" })),
    h("div", { class: "hint", text: t("Allow or deny your agents' requests from your phone, through your own Telegram bot.") }),
    h("div", { class: "row" }, h("label", { text: t("Send permission requests to Telegram") }), enableSwitch),
    h("div", { class: "hint", style: "color:#F5B83D", text: t("Each card goes through Telegram's servers: the agent, the project, the tool and the exact command or file. LuLu has no server of its own and only talks to Telegram while this is on.") }),
    body,
  );
  body.style.display = "none";

  function apply(v: TelegramStatus | null) {
    if (!v) return;
    view = v;
    draw();
  }
  void Bridge.telegramStatus().then(apply);
  let off: (() => void) | null = null;
  // The event says only that something changed (a phone paired): the status, with
  // its pairing code, is asked for through the command only this window may call.
  void onEvent<null>("telegram-changed", () => {
    // A section drawn again (another page, a language change) leaves this one behind.
    if (!section.isConnected && view) {
      off?.();
      return;
    }
    void Bridge.telegramStatus().then(apply);
  }).then((unlisten) => {
    off = unlisten;
  });
  return section;
}
