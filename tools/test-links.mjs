// links with punctuation (Nov 17): test of md() and rifilaLink() from ui/index.html without launching the app.
//   node tools/test-links.mjs [app folder]
// Exits with 1 if a case fails.
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const app = process.argv[2] || path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const src = fs.readFileSync(path.join(app, "ui/index.html"), "utf8");
const start = src.indexOf("// links in messages (17/11)"), end = src.indexOf("// ---------- two-step confirmation");
if (start < 0 || end < 0) { console.error("ERRORE: non trovo rifilaLink() o md() in ui/index.html"); process.exit(1); }
const esc = src.match(/^const esc = .*$/m)[0], hash = src.match(/^const hash = .*$/m)[0];
const ctx = { document: { addEventListener() {} }, console };
vm.createContext(ctx);
vm.runInContext(`${esc}\n${hash}\nconst TIPI_EL = new Set(["stato"]);\nconst jchip = () => "";\nconst tabellaHtml = () => "<table></table>";\nconst elemento = () => "";\nconst elementoInArrivo = () => "";\n${src.slice(start, end)}\nthis.md = md; this.rifilaLink = rifilaLink;`, ctx);

let failures = 0, total = 0;
const check = (nome, ok) => { total++; console.log((ok ? "ok      " : "ERRORE  ") + nome); if (!ok) failures++; };
const U = "https://esempio.it";
// bare link in a sentence: (text, expected link, expected text after the link)
const cases = [
  [`Apri ${U}, premi`, U, ","], [`Apri ${U}. Poi`, U, "."], [`vai (${U})`, U, ")"],
  [`${U}/a?b=1).`, `${U}/a?b=1`, ")."], [`«${U}»`, U, "»"], [`"${U}",`, U, `",`],
  ["https://it.wikipedia.org/wiki/Roma_(citta).", "https://it.wikipedia.org/wiki/Roma_(citta)", "."],
  [`${U}/x;`, `${U}/x`, ";"], [`${U}!?`, U, "!?"], [`${U}...`, U, "..."], [`${U}/p`, `${U}/p`, ""],
  [`${U}/p?q=1&r=2#sez-3`, `${U}/p?q=1&r=2#sez-3`, ""], [`“${U}”`, U, "”"], [`'${U}'`, U, "'"], [`[${U}]`, U, "]"],
  [`${U}/a#b,`, `${U}/a#b`, ","],
];
for (const [t, url, rest] of cases) {
  check(`rifilaLink ${JSON.stringify(t)}`, ctx.rifilaLink(t.match(/https?:\/\/\S+/)[0]) === url);
  const h = ctx.md(`Vai a ${t} ok`);
  const m = h.match(/<a data-url="([^"]*)">([^<]*)<\/a>(.{0,6})/);
  const expected = esc2(url);
  check(`md       ${JSON.stringify(t)}`, !!m && m[1] === expected && m[2] === expected && (rest === "" || esc2(rest).startsWith(m[3].slice(0, esc2(rest).length)) || m[3].startsWith(esc2(rest))));
}
function esc2(x) { return x.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;"); }
let h = ctx.md("Guarda [testo](https://esempio.it/a_(b)), poi");
check("markdown [testo](url) con parentesi bilanciata", h.includes('<a data-url="https://esempio.it/a_(b)">testo</a>, poi'));
h = ctx.md("Guarda [testo](https://esempio.it/a).");
check("markdown [testo](url) seguito da punto", h.includes('<a data-url="https://esempio.it/a">testo</a>.'));
h = ctx.md("[https://esempio.it](https://esempio.it/x)");
check("testo del link uguale a un url", (h.match(/<a /g) || []).length === 1);
h = ctx.md("Prova `https://esempio.it/c` fine");
check("codice con url invariato", h.includes('<a class="cod" data-url="https://esempio.it/c">'));
check("solo http:// non diventa link", !ctx.md("scrivi https:// e basta").includes("<a "));
console.log(failures ? `${failures} prove non passate su ${total}` : `tutte le ${total} prove passate`);
process.exit(failures ? 1 : 0);
