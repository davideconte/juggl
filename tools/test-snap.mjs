// snap (Nov 16): test of ui/snap.js without the app. Where the image goes (open chat, Alt -> Juggl chat, Mac view -> Juggl)
// and how the spoken Settings understand sentences about the snap.   node tools/test-snap.mjs [app folder]
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const app = process.argv[2] || path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const src = fs.readFileSync(path.join(app, "ui/snap.js"), "utf8");
const L = {}, events = [];
const ctx = {
  console, Promise, setTimeout, MutationObserver: class { observe() {} },
  document: { querySelector: () => null, querySelectorAll: () => [], addEventListener() {} },
  window: { addEventListener() {} },
  dati: { juggl: "JUGGL" }, vistaMac: false, saAperto: null,
  VA: () => ({ id: "CHAT-APERTA" }),
  aggiungiPercorsi: (p, V) => events.push(["casella", V.id, p[0]]),
  allegaAChat: (id, p, apri) => events.push(["chat", id, p[0], apri]),
  toast: (t) => events.push(["avviso", t]),
};
ctx.window.__TAURI__ = {
  core: { invoke: async (c) => (c === "scatto_stato" ? { mac: true, tasto: "F16", alt: "Alt+F16", base: "F16", permesso: true, chiesto: true } : { ok: true }) },
  event: { listen: async (n, f) => { L[n] = f; } },
};
vm.createContext(ctx);
vm.runInContext(src, ctx);
await new Promise((r) => setTimeout(r, 20));

let failures = 0;
const check = (nome, ok) => { console.log((ok ? "ok      " : "ERRORE  ") + nome); if (!ok) failures++; };
const last = () => JSON.stringify(events.at(-1));

await L.scatto({ payload: { path: "/tmp/scatto-1.png", alt: false } });
check("senza Alt: nella casella della chat aperta", last() === JSON.stringify(["casella", "CHAT-APERTA", "/tmp/scatto-1.png"]));
await L.scatto({ payload: { path: "/tmp/scatto-2.png", alt: true } });
check("con Alt: nella chat di Juggl, aperta", last() === JSON.stringify(["chat", "JUGGL", "/tmp/scatto-2.png", true]));
ctx.vistaMac = true;
await L.scatto({ payload: { path: "/tmp/scatto-3.png", alt: false } });
check("nessuna chat a schermo: nella chat di Juggl", last() === JSON.stringify(["chat", "JUGGL", "/tmp/scatto-3.png", true]));
ctx.vistaMac = false;
const n = events.length; await L.scatto({ payload: {} });
check("senza file non fa niente", events.length === n);

const entry = ctx.window.impostazioniVociExtra?.find((v) => v.id === "scatto");
check("voce per le Impostazioni a parole", !!entry);
const understand = (f) => entry.prendi(f.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, ""), f)?.v;
for (const [f, v] of [["spegni lo scatto", "spento"], ["scatto con F15", "F15"], ["usa cmd+shift+2 per lo screenshot", "cmd+shift+2"], ["fai lo scatto con ⌘⇧2", "cmd+shift+2"], ["accendi lo scatto", "F16"], ["spegni gli avvisi di notte", undefined]])
  check(`«${f}» -> ${understand(f)}`, understand(f) === v);
console.log(failures ? `${failures} prove non passate` : "tutte le prove passate");
process.exit(failures ? 1 : 0);
