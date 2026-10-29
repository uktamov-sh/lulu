// Uzbek Latin → the new Uzbek alphabet (law OʻRQ-1184: Ö, Ğ, Ş, Ç and the
// tutuq belgisi ʼ). Same rules as the Alifbo converter
// (ozbek-klaviatura/prototype/converter.js), old-Latin part only: the
// translations are written in today's Latin and the "uz-yangi" table was made
// from them with these rules. At run time it only turns the dates Intl writes.

const TUTUQ = "ʼ";
const AP = "'‘’ʻʼ`´";
const SEPARATE = "\u0000";
const wordRe = new RegExp(`[\\p{L}\\p{M}${AP}]+`, "gu");
const oldOG = new RegExp(`([OoGg])([ʻ‘]|[${AP}](?=[\\p{L}${AP}]))`, "gu");
const separateSH = new RegExp(`([SsCc])[${AP}](?=[Hh])`, "gu");
const digraph = /[SsCc][Hh]/gu;
const innerAp = new RegExp(`(?<=\\p{L})[${AP}](?=\\p{L})`, "gu");
const OG: Record<string, string> = { O: "Ö", o: "ö", G: "Ğ", g: "ğ" };
const SC: Record<string, string> = { S: "Ş", s: "ş", C: "Ç", c: "ç" };

function word(w: string): string {
  let out = w.replace(oldOG, (_m, l: string) => OG[l]);
  out = out.replace(separateSH, (_m, l: string) => l + SEPARATE);
  out = out.replace(digraph, (m) => SC[m[0]]);
  return out.split(SEPARATE).join("").replace(innerAp, TUTUQ);
}

/** "Chorshanba, 28-oktyabr" → "Çorşanba, 28-oktyabr". */
export function toYangi(text: string): string {
  return text.replace(wordRe, word);
}
