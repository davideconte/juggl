// new chat (Nov 18): how ui/new-chat.js picks the project and finds similar chats, without the app.
//   node tools/test-new-chat.mjs [app folder]      exits with 1 if something fails
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const app = process.argv[2] || path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const src = fs.readFileSync(path.join(app, "ui/new-chat.js"), "utf8");
const el = () => ({ id: "", textContent: "", style: {}, classList: { add() {}, remove() {}, contains: () => false, toggle() {} }, appendChild() {}, querySelector: () => null, querySelectorAll: () => [] });
const now = Date.now() / 1000;
// fake projects, already sorted by last use like EL.progettiPerUso()
const PROJECTS = [
  { id: "sito", nome: "Sito vetrina", cartella: "~/progetti/sito", parole: ["landing", "homepage"], ultimo: now - 3600 },
  { id: "app", nome: "App mobile", cartella: "~/progetti/app-mobile", parole: ["iphone", "android"], ultimo: now - 86400 },
  { id: "prev", nome: "Preventivi", cartella: "~/Documents/preventivi", parole: [], ultimo: now - 3 * 86400 },
  { id: "news", nome: "Newsletter", cartella: "~/progetti/newsletter", parole: ["brevo"], ultimo: now - 5 * 86400 },
];
const P = (id) => PROJECTS.find((p) => p.id === id);
const ctx = {
  console, Promise, setTimeout, clearTimeout, MutationObserver: class { observe() {} },
  document: { createElement: el, head: el(), querySelector: () => null, querySelectorAll: () => [], addEventListener() {} },
  window: { addEventListener() {} },
  det: null,
  dati: { juggl: "J", imp: {}, sessioni: [
    { sessione: "s1", nome: "Login con Apple e Google", progetto: P("app"), cwd: "~/progetti/app-mobile", vista: "ferma", da: now - 7200 },
    { sessione: "s2", nome: "Pagina prezzi", progetto: P("sito"), cwd: "~/progetti/sito", vista: "aspetta", da: now - 600 },
    { sessione: "s3", nome: "Notifiche push iPhone", progetto: P("app"), cwd: "~/progetti/app-mobile", vista: "ferma", da: now - 90000 },
    { sessione: "J", nome: "Juggl", progetto: null },
  ] },
};
ctx.window.__TAURI__ = { core: { invoke: async () => null }, event: { listen: async () => {} } };
ctx.window.EL = { progettiPerUso: () => PROJECTS, progetti: PROJECTS, icona: () => "" };
ctx.EL = ctx.window.EL;
vm.createContext(ctx);
vm.runInContext(src, ctx);
const capisci = (t) => ctx.window.nuovaChatCapisci(t);

let errors = 0;
const check = (name, ok, got) => { console.log((ok ? "ok      " : "FAIL    ") + name + (ok ? "" : "  -> " + JSON.stringify(got))); if (!ok) errors++; };

let r = capisci("aggiungi il login con Apple nella app mobile");
check("project from its name in the text", r.top?.id === "app" && r.top.why.kind === "text", r);
check("similar chat found (same project, 2+ words)", r.dup === "s1", r);
r = capisci("prepara la newsletter di dicembre per brevo");
check("project from a keyword", r.top?.id === "news", r);
check("no similar chat when nothing matches", r.dup === null, r);
r = capisci("cambia i colori della homepage");
check("project from a keyword (homepage)", r.top?.id === "sito", r);
r = capisci("sistemiamo le notifiche push che non arrivano su iPhone");
check("project from the topic of its chats", r.top?.id === "app", r);
check("similar chat on the same topic", r.dup === "s3", r);
r = capisci("scrivi una mail di ringraziamento");
check("nothing in the text: most recent project", r.top?.id === "sito" && r.top.why.kind === "recent", r);
ctx.det = { progetto: P("prev") };
r = capisci("scrivi una mail di ringraziamento");
check("nothing in the text: project of the open chat", r.top?.id === "prev" && r.top.why.kind === "open", r);
r = capisci("aggiungi il login con Apple nella app mobile");
check("the text wins over the open chat", r.top?.id === "app", r);
ctx.det = null;
r = capisci("pagina");
check("one word only: no similar chat", r.dup === null, r);
check("at most 4 proposals", capisci("ciao").ranked.length <= 4, capisci("ciao"));
console.log(errors ? `${errors} tests failed` : "all tests passed");
process.exit(errors ? 1 : 0);
