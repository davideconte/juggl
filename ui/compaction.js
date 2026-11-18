// Idle compaction (Nov 16): "Compattazione" section in Settings.
// Main switch (off by default), the thresholds and the real summary from Juggl's log (compattazione.jsonl):
// how many compactions in the last 24 hours and how many tokens removed from context (before/after read from Claude Code, not estimates).
// Hook in index.html: <script src="compaction.js"> at the bottom. Commands: compattazione_stato, compattazione_imposta (main.rs).
(() => {
  if (!window.__TAURI__) return;
  let stats = null;
  const esc = (x) => String(x ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const k = (n) => (n >= 1000 ? Math.round(n / 1000) + "k" : String(n || 0));
  async function load() { try { stats = await invoke("compattazione_stato"); } catch { stats = null; } }
  const OUTCOMES = { fatta: "compattata", mandata: "mandata", non_riuscita: "non riuscita", non_mandata: "non mandata", a_mano: "a mano", interruttore: "interruttore" };
  function line(r) {
    const when = new Date((r.t || 0) * 1000).toLocaleString("it-IT", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
    const detail = r.esito === "fatta" ? `da ${k(r.prima)} a ${k(r.dopo)} token` : r.motivo || (r.token ? k(r.token) + " token" : "");
    return `<small style="display:block">${esc(when)} · ${esc(r.nome || (r.sessione || "").slice(0, 8))} · ${esc(OUTCOMES[r.esito] || r.esito)}${detail ? " · " + esc(detail) : ""}</small>`;
  }
  function html() {
    if (!stats) return `<h4>Compattazione</h4><div class="imp-riga"><div class="tx"><small>Dati non disponibili adesso</small></div></div>`;
    const settings = stats.impostazioni || {}, on = !!settings.attiva;
    const num = (key, v, t, sub, min, max) => `<div class="imp-riga"><div class="tx"><b>${t}</b><small>${sub}</small></div><input type="number" min="${min}" max="${max}" value="${esc(v)}" data-comp-n="${key}" style="width:84px"></div>`;
    const recent = (stats.ultime || []).slice(0, 6).map(line).join("");
    return `<h4>Compattazione</h4>
      <div class="imp-riga"><div class="tx"><b>Compatta le chat ferme</b><small>Una chat grande e ferma si riassume prima che la cache scada: quando ci torni riparte leggera. Mai se lavora, ti aspetta o ha lavori in background</small></div><button class="interr ${on ? "on" : ""}" data-comp-sw role="switch" aria-checked="${on}"></button></div>
      <div class="imp-riga"><div class="tx"><b>${stats.oggi || 0} compattazioni nelle ultime 24 ore</b><small>${stats.tolti_oggi ? k(stats.tolti_oggi) + " token tolti dal contesto. " : ""}Numeri veri, letti da Claude Code dopo ogni compattazione</small>${recent}</div></div>
      ${num("soglia_k", Math.round(settings.soglia_k || 150), "Contesto minimo", "In migliaia di token: sotto questa soglia la chat non si tocca", 50, 900)}
      ${num("ferma_min", Math.round(settings.ferma_min || 45), "Ferma da almeno", "Minuti senza attività prima di compattare (la cache dura 60 minuti)", 10, 58)}
      ${num("urgente_pct", Math.round(settings.urgente_pct || 85), "Quasi piena", "Percentuale della finestra oltre cui compatta a fine turno, anche se la chat è ferma da poco", 50, 95)}`;
  }
  function render() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-comp");
    const h = html();
    if (!sec) { const footer = f.querySelector(".piedef"); if (!footer) return; sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-comp"; footer.before(sec); }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  async function open() { await load(); render(); }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (!document.getElementById("imp-comp")) open(); }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  setInterval(() => { if (document.getElementById("imp-comp")) open(); }, 30000);
  async function apply(valori) {
    try { const r = await invoke("compattazione_imposta", { valori }); if (!r.ok) return toast(r.msg || "Non riuscito"); } catch (x) { return toast("Non riuscito: " + x); }
    await open();
  }
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-comp-sw]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    apply({ attiva: !b.classList.contains("on") });
  }, true);
  // row in the chat instead of the raw summary: discreet, grey; the button only on the ones done by Juggl
  const st = document.createElement("style");
  st.textContent = ".nota-compatta{margin:10px auto;text-align:center;font-size:12px;color:var(--grigio,#888);display:flex;gap:10px;justify-content:center;align-items:center}.nota-compatta .tasto{font-size:11px;padding:2px 8px}";
  document.head.appendChild(st);
  window.compattataRiga = (e, d) => {
    const disabled = d && d.compattazione === false;
    const btn = e.da_juggl && d?.sessione ? `<button class="tasto" data-comp-chat="${esc(d.sessione)}" data-comp-acceso="${disabled ? "1" : "0"}">${disabled ? "Riaccendi per questa chat" : "Spegni per questa chat"}</button>` : "";
    return `<div class="nota-compatta"><span>${esc(e.testo)}</span>${btn}</div>`;
  };
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-comp-chat]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    const enabled = b.dataset.compAcceso === "1";
    try { const r = await invoke("compattazione_chat", { id: b.dataset.compChat, acceso: enabled ? null : false }); if (!r.ok) return toast("Non riuscito"); } catch (x) { return toast("Non riuscito: " + x); }
    toast(enabled ? "Compattazione riaccesa per questa chat" : "Compattazione spenta per questa chat");
    b.dataset.compAcceso = enabled ? "0" : "1"; b.textContent = enabled ? "Spegni per questa chat" : "Riaccendi per questa chat";
  }, true);
  document.addEventListener("change", (e) => {
    const i = e.target.closest?.("[data-comp-n]"); if (!i) return;
    const v = Number(i.value); if (!Number.isFinite(v)) return;
    apply({ [i.dataset.compN]: v });
  }, true);
})();
