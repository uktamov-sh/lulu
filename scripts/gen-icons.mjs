// Draws LuLu into the PNG/ICO set Tauri needs. No dependencies: the icons are
// rasterised here and encoded with node:zlib, so the app icon stays "drawn in
// code" like the character itself.
//
//   node scripts/gen-icons.mjs

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons");

// ── LuLu the panda ───────────────────────────────────────────────────────────

const BASE_TOP = [255, 255, 252]; // #FFFFFC
const BASE_BOTTOM = [217, 215, 210]; // #D9D7D2
const BLACK = [28, 25, 24]; // #1C1918
const WHITE = [255, 255, 255];
const RIM = [0, 0, 0];

const SS = 4; // supersampling factor
const EXP = 2.25; // the engine's round head

/** Superellipse test in body-local coordinates. */
function insideBody(x, y, rx, ry) {
  return Math.pow(Math.abs(x / rx), EXP) + Math.pow(Math.abs(y / ry), EXP) <= 1;
}

function insideEllipse(x, y, rx, ry, rot = 0) {
  const c = Math.cos(rot);
  const s = Math.sin(rot);
  const u = x * c + y * s;
  const v = -x * s + y * c;
  return (u / rx) ** 2 + (v / ry) ** 2 <= 1;
}

function insidePill(x, y, w, h) {
  const hw = w / 2;
  const hh = h / 2;
  const r = Math.min(hw, hh);
  const cx = Math.max(-hw + r, Math.min(hw - r, x));
  const cy = Math.max(-hh + r, Math.min(hh - r, y));
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}

function renderLuLu(size) {
  const px = new Uint8Array(size * size * 4);
  const R = size * 0.3;
  const rx = R * 1.14;
  const ry = R * 0.88;
  const cx = size / 2;
  const cy = size / 2 + R * 0.22;
  const rim = Math.max(0.6, R * 0.05); // dark outline so the tray icon reads on light themes

  // Same geometry as BotEngine: ears, eyes (yaw ±0.37, pitch −0.12), patches, nose.
  const earX = Math.sin(0.74) * rx * 0.9;
  const earY = -ry * 0.98;
  const earR = R * 0.34;
  const ex = Math.sin(0.37) * Math.cos(-0.12) * rx;
  const ey = -Math.sin(-0.12) * ry;
  const fx = Math.max(0.18, Math.cos(0.37));
  const ew = R * 0.25;
  const eh = R * 0.27;
  const noseY = -Math.sin(-0.12 - 0.3) * ry;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let rimHits = 0;
      const acc = [0, 0, 0];
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const X = x + (sx + 0.5) / SS - cx;
          const Y = y + (sy + 0.5) / SS - cy;
          const inEar = [-1, 1].some((sd) => (X - sd * earX) ** 2 + (Y - earY) ** 2 <= earR ** 2);
          const inEarRim = [-1, 1].some((sd) => (X - sd * earX) ** 2 + (Y - earY) ** 2 <= (earR + rim) ** 2);
          const inBody = insideBody(X, Y, rx, ry);
          if (!inBody && !inEar && !insideBody(X, Y, rx + rim, ry + rim) && !inEarRim) continue;
          rimHits++;
          let col = RIM;
          if (inEar) col = BLACK;
          if (inBody) {
            const t = Math.min(1, Math.max(0, (X * -0.6 + Y * 0.8) / (2 * ry) + 0.5));
            col = [0, 1, 2].map((i) => BASE_TOP[i] + (BASE_BOTTOM[i] - BASE_TOP[i]) * t);
            for (const sd of [-1, 1]) {
              const lx = (X - sd * ex) / fx;
              const ly = Y - ey;
              if (insideEllipse(lx, ly - eh * 0.1, ew * 1.12, eh * 1.32, sd * 0.5)) col = BLACK;
              if (insidePill(lx, ly, ew, eh)) col = WHITE;
              if (lx ** 2 + (ly - eh * 0.08) ** 2 <= (Math.min(ew, eh) * 0.34) ** 2) col = BLACK;
            }
            if (insideEllipse(X, Y - noseY, R * 0.09, R * 0.06)) col = BLACK;
          }
          acc[0] += col[0];
          acc[1] += col[1];
          acc[2] += col[2];
        }
      }
      if (rimHits === 0) continue;
      const o = (y * size + x) * 4;
      px[o] = Math.round(acc[0] / rimHits);
      px[o + 1] = Math.round(acc[1] / rimHits);
      px[o + 2] = Math.round(acc[2] / rimHits);
      px[o + 3] = Math.round((rimHits / (SS * SS)) * 255);
    }
  }
  return px;
}

// ── PNG ───────────────────────────────────────────────────────────────────────

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePNG(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    Buffer.from(rgba.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ── ICO (PNG-in-ICO, Vista and later) ─────────────────────────────────────────

function encodeICO(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = header.length + dir.length;
  entries.forEach((e, i) => {
    const o = i * 16;
    dir[o] = e.size >= 256 ? 0 : e.size;
    dir[o + 1] = e.size >= 256 ? 0 : e.size;
    dir[o + 2] = 0;
    dir[o + 3] = 0;
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(e.png.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += e.png.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.png)]);
}

// ── Go ────────────────────────────────────────────────────────────────────────

mkdirSync(OUT, { recursive: true });

const png = (size) => encodePNG(size, renderLuLu(size));

const files = {
  "32x32.png": png(32),
  "128x128.png": png(128),
  "128x128@2x.png": png(256),
  "icon.png": png(512),
};
for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(OUT, name), data);
  console.log(`${name} — ${data.length} bytes`);
}

const ico = encodeICO([16, 24, 32, 48, 64, 128, 256].map((size) => ({ size, png: png(size) })));
writeFileSync(join(OUT, "icon.ico"), ico);
console.log(`icon.ico — ${ico.length} bytes`);
