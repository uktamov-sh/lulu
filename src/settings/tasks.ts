// Settings → Tasks: a short to-do list with optional reminders (src-tauri/src/tasks.rs).
// When a task's time comes, the island shows it, and Telegram sends it to the
// phone when Telegram approvals are on and paired.

import { Bridge, onEvent, type TaskItem } from "../core/bridge";
import { h } from "../views/dom";
import { language, t } from "../i18n/i18n";

/** "29 Oct, 18:30" in LuLu's language (Intl only knows today's Uzbek Latin). */
function when(ms: number): string {
  const lang = language() === "uz-yangi" ? "uz" : language();
  try {
    return new Date(ms).toLocaleString(lang, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
  } catch {
    return new Date(ms).toLocaleString();
  }
}

export function tasksSection(): HTMLElement {
  let list: TaskItem[] = [];
  const rows = h("div", { style: "display:flex;flex-direction:column;gap:6px" });
  const text = h("input", { type: "text", placeholder: t("What to do"), maxlength: "300", style: "flex:1 1 auto;min-width:0" }) as HTMLInputElement;
  const time = h("input", { type: "datetime-local" }) as HTMLInputElement;
  const daily = h("input", { type: "checkbox" }) as HTMLInputElement;
  const add = h("button", { class: "primary", text: t("Add") }) as HTMLButtonElement;

  function draw() {
    rows.replaceChildren();
    if (!list.length) {
      rows.append(h("div", { class: "hint", text: t("No tasks yet.") }));
      return;
    }
    // Open tasks first, the soonest reminder on top.
    const sorted = [...list].sort((a, b) => Number(a.done) - Number(b.done) || (a.due ?? Infinity) - (b.due ?? Infinity));
    for (const task of sorted) {
      const done = h("input", { type: "checkbox" }) as HTMLInputElement;
      done.checked = task.done;
      done.onchange = () => void Bridge.tasksSetDone(task.id, done.checked).then(apply);
      const label = h("span", { text: task.text, style: `flex:1 1 auto;min-width:0;${task.done ? "opacity:.5;text-decoration:line-through" : ""}` });
      const meta = task.due != null
        ? h("span", { class: "hint", text: `⏰ ${when(task.due)}${task.daily ? ` · ${t("every day")}` : ""}` })
        : null;
      const remove = h("button", { class: "danger", text: t("Delete") });
      remove.onclick = () => void Bridge.tasksDelete(task.id).then(apply);
      rows.append(h("div", { class: "row", style: "gap:10px" }, done, label, ...(meta ? [meta] : []), remove));
    }
  }

  function apply(next: TaskItem[] | null) {
    if (!next) return;
    list = next;
    draw();
  }

  async function submit() {
    const value = text.value.trim();
    if (!value) return;
    const due = time.value ? new Date(time.value).getTime() : null;
    const next = await Bridge.tasksAdd(value, Number.isFinite(due) ? due : null, daily.checked);
    if (!next) return;
    text.value = "";
    time.value = "";
    daily.checked = false;
    apply(next);
  }
  add.onclick = () => void submit();
  text.onkeydown = (e: KeyboardEvent) => {
    if (e.key === "Enter") void submit();
  };

  const section = h(
    "section",
    {},
    h("h2", {}, h("span", { text: t("Tasks") })),
    h("div", { class: "hint", text: t("LuLu reminds you at the time you set, in the island and, when Telegram is connected, on your phone.") }),
    h("div", { class: "row", style: "gap:8px;flex-wrap:wrap" }, text, time,
      h("label", { style: "display:flex;gap:6px;align-items:center" }, daily, h("span", { text: t("every day") })), add),
    rows,
  );
  void Bridge.tasksList().then(apply);
  let off: (() => void) | null = null;
  void onEvent<TaskItem[]>("tasks-changed", (next) => {
    // A section drawn again (another page, a language change) leaves this one behind.
    if (!section.isConnected) {
      off?.();
      return;
    }
    apply(next);
  }).then((unlisten) => {
    off = unlisten;
  });
  return section;
}
