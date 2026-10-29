// Dev harness: LuLu in every state side by side, in a plain browser.
// Not part of the app bundle.

import { BotEngine, hexToRGB } from "../src/mochi/engine";
import type { BotEmoteName, BotStateName } from "../src/core/layout";
import type { Outfit } from "../src/mochi/wardrobe";

interface Cell {
  label: string;
  state?: BotStateName;
  emote?: BotEmoteName;
  mini?: string;
  light?: boolean;
  look?: boolean;
  size?: number;
  outfit?: Outfit;
}

const CELLS: Cell[] = [
  { label: "Тюбетейка (муж.)", state: "idle", outfit: "doppi", look: true },
  { label: "Тюбетейка (жен.)", state: "idle", outfit: "doppiGirl", look: true },
  { label: "Бандит", state: "idle", outfit: "bandit", look: true },
  { label: "Шапка", state: "idle", outfit: "beanie" },
  { label: "Корона", state: "idle", outfit: "crown" },
  { label: "Очки", state: "idle", outfit: "sunglasses" },
  { label: "Шляпа ведьмы", state: "idle", outfit: "witchHat" },
  { label: "Шарф", state: "idle", outfit: "scarf" },
  { label: "Покой", state: "idle", size: 220 },
  { label: "Следит за курсором", state: "idle", look: true },
  { label: "Работает", state: "working" },
  { label: "Ждёт разрешения", state: "approval" },
  { label: "Вопрос", state: "question" },
  { label: "Готово", state: "finished" },
  { label: "Ошибка", state: "error" },
  { label: "Спит", state: "sleeping" },
  { label: "Любовь", state: "idle", emote: "love" },
  { label: "Гордость", state: "idle", emote: "proud" },
  { label: "Голова кружится", state: "dizzy" },
  { label: "На светлом фоне", state: "idle", light: true },
  { label: "Мини: GitHub", mini: "#8B5CF6" },
  { label: "Мини: Stripe", mini: "#635BFF" },
  { label: "Мини: Vercel", mini: "#F5F5F5" },
];

const grid = document.getElementById("grid")!;
const items: { bot: BotEngine; ctx: CanvasRenderingContext2D; size: number; cell: Cell }[] = [];
const dpr = window.devicePixelRatio || 1;

for (const cell of CELLS) {
  const size = cell.size ?? (cell.mini ? 110 : 170);
  const fig = document.createElement("figure");
  if (cell.light) fig.classList.add("light");
  const canvas = document.createElement("canvas");
  canvas.width = size * dpr;
  canvas.height = size * dpr;
  canvas.style.width = `${size}px`;
  canvas.style.height = `${size}px`;
  const cap = document.createElement("figcaption");
  cap.textContent = cell.label;
  fig.append(canvas, cap);
  grid.append(fig);

  const bot = new BotEngine();
  if (cell.mini) {
    bot.isMini = true;
    bot.bodyColor = hexToRGB(cell.mini);
  }
  if (cell.state) bot.setState(cell.state, true);
  if (cell.emote) bot.setPermanentEmote(cell.emote);
  if (cell.outfit) bot.setOutfit(cell.outfit, false);
  items.push({ bot, ctx: canvas.getContext("2d")!, size, cell });
}

// For poking at from the console: window.lulu[i].bot
(window as unknown as { lulu: typeof items }).lulu = items;

let last = performance.now();
function loop(t: number) {
  const dt = Math.min(0.05, (t - last) / 1000);
  last = t;
  for (const { bot, ctx, size, cell } of items) {
    if (cell.look) {
      const a = t / 1400;
      bot.lookX = Math.cos(a) * 0.9;
      bot.lookY = Math.sin(a * 1.3) * 0.6;
    }
    bot.update(dt);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size, size);
    bot.draw(ctx, size, size);
  }
  requestAnimationFrame(loop);
}
requestAnimationFrame(loop);
