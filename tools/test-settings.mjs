// settings as a chat (Nov 16): how sentences are understood (local rules of ui/settings-chat.js), without the app.
//   node tools/test-settings.mjs [app folder]      exits with 1 if something fails
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const app = process.argv[2] || path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const src = fs.readFileSync(path.join(app, "ui/settings-chat.js"), "utf8");
const el = () => ({ id: "", textContent: "", style: {}, classList: { add() {}, remove() {}, contains: () => false, toggle() {} }, appendChild() {}, querySelector: () => null, querySelectorAll: () => [] });
const fake = new Proxy(function () {}, { get: (t, k) => (k === "then" ? undefined : fake), apply: () => Promise.resolve(null) });
const ctx = {
  console, Promise, setTimeout, clearTimeout, setInterval, clearInterval, MutationObserver: class { observe() {} },
  document: { createElement: el, head: el(), querySelector: () => null, querySelectorAll: () => [], addEventListener() {}, activeElement: null },
  window: {},
};
const TEST_FEATURES = ["iphone", "mac", "spazio", "cmux", "pc"];
ctx.window.__TAURI__ = { core: { invoke: async (c) => (c === "profilo" ? { funzioni: TEST_FEATURES.map((id) => ({ id, titolo: id, disponibile: true, accesa: false })) } : null) }, event: { listen: async () => {}, emit: async () => {} } };
ctx.window.tipografia = { misure: () => ["piccolo", "normale", "grande"], attuale: () => "normale", imposta() {}, larghezza: () => true, attualeLarghezza: () => "normale" };
vm.createContext(ctx);
vm.runInContext(src, ctx);
await new Promise((r) => setTimeout(r, 20));   // carica() of the fake profile
const understand = ctx.window.impostazioniCapisci;

let failures = 0;
const check = (frase, id, v, extra) => {
  const r = understand(frase);
  const ok = id === null ? r === null : r && r.id === id && r.v === v && (!extra || JSON.stringify(r.extra) === JSON.stringify(extra));
  console.log((ok ? "ok      " : "ERRORE  ") + `«${frase}» -> ${r ? r.id + " = " + r.v + (r.extra ? " " + JSON.stringify(r.extra) : "") : "niente"}`);
  if (!ok) failures++;
};
check("spegni gli avvisi di notte", "avvisi_notte", "acceso");
check("Spegni le notifiche la notte dalle 22 alle 7", "avvisi_notte", "acceso", { da: "22:00", a: "07:00" });
check("riaccendi gli avvisi di notte", "avvisi_notte", "spento");
check("metti il silenzio di notte", "avvisi_notte", "acceso");
check("usa Sonnet per le chat nuove", "modello_nuove", "sonnet");
check("le nuove chat con opus", "modello_nuove", "opus");
check("chat nuove col modello predefinito", "modello_nuove", "predefinito");
check("cambia account", "account_claude", "cambia");
check("esci dall'account claude", "account_claude", "esci");
check("esci dall'account di Juggl", "account_juggl", "esci");
check("accendi eco", "eco", "acceso");
check("spegni la modalità eco", "eco", "spento");
check("testo più grande", "testo", "grande");
check("caratteri piccoli", "testo", "piccolo");
check("collega l'iPhone", "funzioni.iphone", "acceso");
check("togli il collegamento con Claude Code", "collegamento", "togli");
check("cerca aggiornamenti", "aggiornamenti", "cerca");
check("cambia i permessi", "permessi", "apri");
check("chiamami Ale", "nome", "Ale");
check("colonna più larga", "larghezza", "larga");
check("testo più largo", "larghezza", "larga");
check("usa tutta la larghezza dello schermo", "larghezza", "larga");
check("colonna stretta", "larghezza", "stretta");
check("torna alla colonna normale", "larghezza", "normale");
check("fammi un caffè", null);
console.log(failures ? `${failures} prove non passate` : "tutte le prove passate");
process.exit(failures ? 1 : 0);
