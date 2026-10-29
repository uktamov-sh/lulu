// LuLu's wardrobe — the pure logic, port of NotchBuddy/Sources/LuLuKit/LuLuWardrobe.swift.
// What LuLu wears is picked in the wardrobe view (right-click on LuLu, or the
// tray menu) and stored in the preferences as `mochiOutfit`. The drawing lives
// in ./outfits.ts.

import { N_, labels, t } from "../i18n/i18n";

/**
 * Every outfit, plus "auto" (dress for the season), "shuffle" (change by
 * herself every SHUFFLE_MS) and "none". Order of the wardrobe.
 */
export const OUTFIT_SELECTIONS = [
  "auto", "shuffle", "none", "doppi", "doppiGirl", "bandit", "partyHat", "beanie", "crown",
  "sunglasses", "roundGlasses", "bow", "scarf", "witchHat", "pumpkin", "santaHat", "bunnyEars",
] as const;

/** What can be stored in the preferences. The raw values are the Mac's, keep them stable. */
export type OutfitSelection = (typeof OUTFIT_SELECTIONS)[number];

/** What LuLu actually wears: a selection with "auto" and "shuffle" resolved. */
export type Outfit = Exclude<OutfitSelection, "auto" | "shuffle">;

export const DEFAULT_OUTFIT: OutfitSelection = "auto";

// ── User-visible strings ──────────────────────────────────────────────────────

/** The English names, the keys of their translations (src/i18n). */
export const OUTFIT_KEYS: Record<OutfitSelection, string> = {
  auto: N_("Auto (seasons)"),
  shuffle: N_("Shuffle (changes on its own)"),
  none: N_("None"),
  doppi: N_("Doppi"),
  doppiGirl: N_("Girl's doppi"),
  bandit: N_("Bandit"),
  partyHat: N_("Party hat"),
  beanie: N_("Beanie"),
  crown: N_("Crown"),
  sunglasses: N_("Sunglasses"),
  roundGlasses: N_("Round glasses"),
  bow: N_("Bow"),
  scarf: N_("Scarf"),
  witchHat: N_("Witch hat"),
  pumpkin: N_("Pumpkin"),
  santaHat: N_("Santa hat"),
  bunnyEars: N_("Bunny ears"),
};

/** In the current language: every read goes through `t()`. */
export const OUTFIT_LABELS: Record<OutfitSelection, string> = labels(OUTFIT_KEYS);

export const WARDROBE_STRINGS = {
  get title() { return t("Wardrobe"); },
  get autoBadge() { return t("AUTO"); },
  get shuffleBadge() { return t("MIX"); },
  autoNow: (current: string) => t("Auto · {outfit}", { outfit: current }),
  autoHover: (current: string) => t("Auto · follows the seasons (now: {outfit})", { outfit: current }),
  shuffleNow: (current: string) => t("Shuffle · {outfit}", { outfit: current }),
  shuffleHover: (current: string) => t("Shuffle · a new outfit every 20 minutes (now: {outfit})", { outfit: current }),
};

// ── Logic ─────────────────────────────────────────────────────────────────────

const SELECTION_SET: ReadonlySet<string> = new Set(OUTFIT_SELECTIONS);

/**
 * A stored preference → a selection. Anything unknown (a value from a newer or
 * older build, such as the Mac's removed "topHat", or garbage) means "auto",
 * exactly like `Outfit.stored` on macOS.
 */
export function parseOutfit(raw: unknown): OutfitSelection {
  return typeof raw === "string" && SELECTION_SET.has(raw) ? (raw as OutfitSelection) : DEFAULT_OUTFIT;
}

/** Easter Sunday of `year` (Meeus/Jones/Butcher), as [month 1–12, day]. */
export function easterDate(year: number): [number, number] {
  const a = year % 19;
  const b = Math.floor(year / 100);
  const c = year % 100;
  const d = Math.floor(b / 4);
  const e = b % 4;
  const f = Math.floor((b + 8) / 25);
  const g = Math.floor((b - f + 1) / 3);
  const h = (19 * a + b - d - g + 15) % 30;
  const i = Math.floor(c / 4);
  const k = c % 4;
  const l = (32 + 2 * e + 2 * i - h - k) % 7;
  const m = Math.floor((a + 11 * h + 22 * l) / 451);
  const month = Math.floor((h + l - 7 * m + 114) / 31);
  const day = ((h + l - 7 * m + 114) % 31) + 1;
  return [month, day];
}

const DAY_MS = 86_400_000;

/**
 * The seasonal outfit for `date`, read in the user's local calendar.
 * Priority: party hat > Santa hat > witch hat > bunny ears > sunglasses > none.
 */
export function seasonalOutfit(date: Date): Outfit {
  const day = date.getDate();
  const month = date.getMonth() + 1;
  const year = date.getFullYear();

  // Dec 31 – Jan 2
  if ((month === 12 && day === 31) || (month === 1 && day <= 2)) return "partyHat";
  // Dec 1–26
  if (month === 12 && day <= 26) return "santaHat";
  // Oct 1 – Nov 1
  if (month === 10 || (month === 11 && day === 1)) return "witchHat";
  // Two days before Easter to the day after. Calendar days counted in UTC so a
  // daylight-saving change in between can't shift the count.
  const [em, ed] = easterDate(year);
  const delta = Math.round((Date.UTC(year, month - 1, day) - Date.UTC(year, em - 1, ed)) / DAY_MS);
  if (delta >= -2 && delta <= 1) return "bunnyEars";
  // Jun 21 – Aug 31
  if ((month === 6 && day >= 21) || month === 7 || month === 8) return "sunglasses";
  return "none";
}

/** How long LuLu keeps an outfit when she changes by herself. */
export const SHUFFLE_MS = 20 * 60_000;

/** Every outfit she can pick by herself ("none" is not one). */
export const SHUFFLE_POOL: readonly Outfit[] = OUTFIT_SELECTIONS.filter(
  (s): s is Exclude<Outfit, "none"> => s !== "auto" && s !== "shuffle" && s !== "none",
);

/** The pool in a fixed mixed-up order: walked one step per slot, so no outfit comes twice in a row. */
const SHUFFLE_ORDER: readonly Outfit[] = (() => {
  const order = [...SHUFFLE_POOL];
  let seed = 0x4c754c75; // "LuLu"
  for (let i = order.length - 1; i > 0; i--) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    const j = seed % (i + 1);
    [order[i], order[j]] = [order[j], order[i]];
  }
  return order;
})();

/** What she wears in the SHUFFLE_MS slot that `date` falls in — the same in every window. */
export function shuffledOutfit(date: Date): Outfit {
  const slot = Math.floor(date.getTime() / SHUFFLE_MS);
  const n = SHUFFLE_ORDER.length;
  return SHUFFLE_ORDER[((slot % n) + n) % n];
}

/** "auto" → the season's outfit, "shuffle" → this slot's; anything else is worn as chosen. */
export function resolveOutfit(selection: OutfitSelection, date: Date): Outfit {
  if (selection === "auto") return seasonalOutfit(date);
  if (selection === "shuffle") return shuffledOutfit(date);
  return selection;
}

/**
 * The grey text at the right of the wardrobe header: the hovered outfit,
 * otherwise the current choice — WardrobeView.headerRight on macOS.
 */
export function wardrobeHeader(
  hovered: OutfitSelection | null,
  selection: OutfitSelection,
  date: Date,
): string {
  const season = OUTFIT_LABELS[seasonalOutfit(date)];
  const mixed = OUTFIT_LABELS[shuffledOutfit(date)];
  if (hovered === "auto") return WARDROBE_STRINGS.autoHover(season);
  if (hovered === "shuffle") return WARDROBE_STRINGS.shuffleHover(mixed);
  if (hovered) return OUTFIT_LABELS[hovered];
  if (selection === "auto") return WARDROBE_STRINGS.autoNow(season);
  if (selection === "shuffle") return WARDROBE_STRINGS.shuffleNow(mixed);
  return OUTFIT_LABELS[selection];
}

/** Same as resolveOutfit, remembered for the day (or the shuffle slot) — it is asked every frame. */
export class SeasonCache {
  private key = "";
  private value: Outfit = "none";

  get(selection: OutfitSelection, date = new Date()): Outfit {
    if (selection !== "auto" && selection !== "shuffle") return selection;
    const key = selection === "auto"
      ? `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
      : `shuffle-${Math.floor(date.getTime() / SHUFFLE_MS)}`;
    if (key !== this.key) {
      this.key = key;
      this.value = resolveOutfit(selection, date);
    }
    return this.value;
  }
}
