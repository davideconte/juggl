// Juggl helpers (0.1.70): "Consumi di Juggl, ultime 24 h" section in Settings.
// Shows how many calls to the small model the helpers made in the last 24 hours (real data from the log, not estimates),
// a switch for each helper and the two values that tune request extraction: pause between two analyses of the same
// chat and the 24-hour cap. Past the cap new requests are not lost: they are analyzed later.
// Hook in index.html: <script src="helpers.js"> at the bottom. Commands: aiutanti_stato, aiutanti_imposta (main.rs).
(() => {
  if (!window.__TAURI__) return;
  const NAMES = { estrattore: "Richieste", riassunti: "Riassunti del lavoro", pilota: "Pilota automatico", permessi: "Permessi",
    nomi: "Nomi delle chat", supporto: "Aiuto e segnalazioni", altro: "Altro" };
  const SUBTITLES = {
    estrattore: "Capisce dai tuoi messaggi cosa hai chiesto", riassunti: "Le frasi che spiegano cosa sta facendo un agente", pilota: "Risponde da solo alle domande semplici degli agenti",
    permessi: "Valuta le richieste di permesso",
    nomi: "Dà un nome alle chat nuove", supporto: "Risponde nella chat Aiuto e segnalazioni", altro: "Altre chiamate",
  };
  let stats = null;
  const esc = (x) => String(x ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  async function load() { try { stats = await invoke("aiutanti_stato"); } catch { stats = null; } }
  function html() {
    if (!stats) return `<h4>Consumi di Juggl, ultime 24 h</h4><div class="imp-riga"><div class="tx"><small>Dati non disponibili adesso</small></div></div>`;
    const c = stats.consumi || {}, settings = stats.impostazioni || {}, byCategory = c.categorie || {};
    const rows = Object.keys(NAMES).map((k) => {
      const x = byCategory[k] || {}, on = settings.attivi?.[k] !== false;
      const saved = (x.cache || 0) + (x.rimandate || 0);
      const detail = `${x.chiamate || 0} chiamate${saved ? `, ${saved} risparmiate` : ""}${x.spenta ? `, ${x.spenta} saltate perché spento` : ""}`;
      return `<div class="imp-riga"><div class="tx"><b>${NAMES[k]}</b><small>${esc(SUBTITLES[k])} · ${detail}</small></div><button class="interr ${on ? "on" : ""}" data-aiut-sw="${k}" role="switch" aria-checked="${on}"></button></div>`;
    }).join("");
    const warning = c.avviso ? `<div class="imp-riga"><div class="tx"><b>Tetto raggiunto</b><small>${esc(c.avviso)}</small></div></div>` : "";
    const num = (k, v, t, sub, min, max) => `<div class="imp-riga"><div class="tx"><b>${t}</b><small>${sub}</small></div><input type="number" min="${min}" max="${max}" value="${esc(v)}" data-aiut-n="${k}" style="width:84px"></div>`;
    return `<h4>Consumi di Juggl, ultime 24 h</h4>
      <div class="imp-riga"><div class="tx"><b>${c.totale_chiamate || 0} chiamate al modello piccolo</b><small>${c.risparmiate || 0} risparmiate da cache, pausa e interruttori. Sono numeri veri, letti dal registro di Juggl</small></div></div>
      ${warning}${rows}
      ${num("pausa_estrattore_s", Math.round(settings.pausa_estrattore_s || 150), "Pausa tra due analisi della stessa chat", "In secondi: i messaggi che arrivano nel frattempo si analizzano insieme", 5, 3600)}
      ${num("tetto_estrattore", settings.tetto_estrattore || 600, "Tetto di chiamate per le richieste in 24 ore", "Oltre il tetto le richieste nuove si analizzano più tardi, non si perdono. I turni che aspettano te passano sempre", 20, 5000)}`;
  }
  function render() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-aiut");
    const h = html();
    if (!sec) { const footer = f.querySelector(".piedef"); if (!footer) return; sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-aiut"; footer.before(sec); }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  async function open() { await load(); render(); }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (!document.getElementById("imp-aiut")) open(); }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  setInterval(() => { if (document.getElementById("imp-aiut")) open(); }, 30000);
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-aiut-sw]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    const k = b.dataset.aiutSw, enabled = !b.classList.contains("on");
    try { stats = await invoke("aiutanti_imposta", { valori: { attivi: { [k]: enabled } } }); } catch (x) { return toast("Non riuscito: " + x); }
    render();
  }, true);
  document.addEventListener("change", async (e) => {
    const i = e.target.closest?.("[data-aiut-n]"); if (!i) return;
    const v = Number(i.value); if (!Number.isFinite(v)) return;
    try { stats = await invoke("aiutanti_imposta", { valori: { [i.dataset.aiutN]: v } }); } catch (x) { return toast("Non riuscito: " + x); }
    render();
  }, true);
})();
