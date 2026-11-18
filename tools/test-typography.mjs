// typography (Nov 16): test of the md() converter of ui/index.html without launching the app.
//   node tools/test-typography.mjs [app folder]
// Extracts md() and codiceHtml() from the page, runs them with small stubs and checks headings and code. Exits with 1 if something fails.
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const app = process.argv[2] || path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const src = fs.readFileSync(path.join(app, "ui/index.html"), "utf8");
const start = src.indexOf("const codAperti = new Set();"), end = src.indexOf("// ---------- two-step confirmation");
if (start < 0 || end < 0) { console.error("ERRORE: non trovo md() o codiceHtml() in ui/index.html"); process.exit(1); }
const chunk = src.slice(start, end);
const esc = src.match(/^const esc = .*$/m)[0], hash = src.match(/^const hash = .*$/m)[0];
const ctx = { document: { addEventListener() {} }, console };
vm.createContext(ctx);
vm.runInContext(`${esc}\n${hash}\nconst TIPI_EL = new Set(["stato"]);\nconst jchip = () => "";\nconst tabellaHtml = () => "<table></table>";\nconst elemento = () => "";\nconst elementoInArrivo = () => "";\n${chunk}\nthis.md = md; this.codAperti = codAperti;`, ctx);

let failures = 0;
const check = (nome, ok) => { console.log((ok ? "ok      " : "ERRORE  ") + nome); if (!ok) failures++; };
const long = Array.from({ length: 12 }, (_, i) => `riga ${i + 1}`).join("\n");

let h = ctx.md("# Uno\n\n## Due\n\n### Tre\n\n#### Quattro");
check("# e ## diventano h2", (h.match(/<h2>/g) || []).length === 2);
check("### diventa h3", h.includes("<h3>Tre</h3>"));
check("#### diventa h4", h.includes("<h4>Quattro</h4>"));
h = ctx.md("## Titolo\nTesto subito sotto\n- voce");
check("titolo attaccato al paragrafo", h.startsWith("<h2>Titolo</h2><p>Testo subito sotto</p><ul><li>voce</li></ul>"));
h = ctx.md("#### Dettagli\n| a | b |\n|---|---|\n| 1 | 2 |");
check("titolo attaccato a una tabella", h === "<h4>Dettagli</h4><table></table>");
check("#1 non e' un titolo", !ctx.md("#1 in classifica").includes("<h"));
check("codice corto aperto", /^<pre><button class="copia-cod">Copia<\/button><code>a\nb<\/code><\/pre>$/.test(ctx.md("```bash\na\nb\n```")));
h = ctx.md("```bash\n" + long + "\n```");
check("codice lungo chiuso", h.startsWith('<details class="cod2"') && !h.includes(" open>") && h.includes("12 righe") && h.includes("riga 1</span>"));
check("codice lungo: lingua e Copia nella riga", h.includes('<span class="cod-l">bash</span>') && /<summary>.*copia-cod.*<\/summary>/.test(h));
const k = h.match(/data-cod="([^"]+)"/)[1]; ctx.codAperti.add(k);
check("codice aperto resta aperto al ridisegno", ctx.md("```bash\n" + long + "\n```").includes(" open>"));
check("codice senza lingua", ctx.md("```\n" + long + "\n```").includes('<span class="cod-l">codice</span>'));
check("html nel codice protetto", !ctx.md("```\n" + long + "\n<script>x</script>\n```").includes("<script>"));
console.log(failures ? `${failures} prove non passate` : "tutte le prove passate");
process.exit(failures ? 1 : 0);
