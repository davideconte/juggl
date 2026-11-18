// Request cleanup (0.1.79), in Settings: cleanup status, the dry-run sample, "Annulla ultima pulizia"
// and the "Pulizia estesa" switch (off by default). Requests are never deleted: the ones Juggl closes stay
// with a reason and can be reopened ("Chiuse o unite da Juggl" section of the Requests view).
// Commands (requests.rs): richieste_pulizia_stato, richieste_campione_pulizia, richieste_annulla_pulizia, richieste_pulizia_estesa_imposta.
(() => {
  if (!window.__TAURI__) return;
  const NAMES = { decisione: "Decisioni tue", lavoro_in_corso: "Lavori in corso", appesa: "Appese (ferme, nessuna domanda per te)", vecchia: "Vecchie (in Mostra altre)" };
  const esc = (x) => String(x ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  let st = null, drySample = null;
  async function load() { try { st = await invoke("richieste_pulizia_stato"); } catch { st = null; } }
  function html() {
    if (!st?.ok) return `<h4>Pulizia delle richieste</h4><div class="imp-riga"><div class="tx"><small>Dati non disponibili adesso</small></div></div>`;
    const kinds = Object.entries(NAMES).map(([k, n]) => `${n}: <b>${st.per_tipo?.[k] || 0}</b>`).join(" · ");
    const l = st.ultimo_lotto, when = l?.il ? new Date(l.il * 1000).toLocaleString("it-IT", { weekday: "short", hour: "2-digit", minute: "2-digit" }) : "";
    const smp = drySample ? `<div class="imp-riga"><div class="tx"><b>Campione a secco</b><small>${drySample.ok ? `Su ${drySample.aperte} richieste aperte la pulizia di base ne chiuderebbe ${drySample.chiuderebbe}; con la pulizia estesa ${drySample.chiuderebbe_con_estesa}.${(drySample.esempi || []).slice(0, 5).map((e) => `<br>· ${esc(e.titolo)}`).join("")}` : esc(drySample.msg || "Non disponibile")}</small></div></div>` : "";
    return `<h4>Pulizia delle richieste</h4>
      <div class="imp-riga"><div class="tx"><b>Come sono le richieste aperte</b><small>${kinds}</small></div></div>
      <div class="imp-riga"><div class="tx"><b>Pulizia estesa</b><small>${st.estesa ? "Accesa: chiude anche le richieste di più di 3 giorni con la chat ferma e senza domande per te. Si riaprono dalla vista Richieste." : "Spenta: quelle richieste restano aperte e finiscono in Mostra altre."}</small></div><button class="interr ${st.estesa ? "on" : ""}" data-pul-estesa role="switch" aria-checked="${!!st.estesa}"></button></div>
      <div class="imp-riga"><div class="tx"><b>Ultima pulizia</b><small>${l ? `${l.n} richieste chiuse ${esc(when)}. Niente è cancellato: si riaprono.` : "Nessuna richiesta chiusa da Juggl di recente."}</small></div>
        <button class="tasto" data-pul-campione>Campione</button>${l ? `<button class="tasto" data-pul-annulla>Annulla ultima pulizia</button>` : ""}</div>${smp}`;
  }
  function render() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-pul");
    const h = html();
    if (!sec) { const footer = f.querySelector(".piedef"); if (!footer) return; sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-pul"; footer.before(sec); }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  async function open() { await load(); render(); }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (!document.getElementById("imp-pul")) open(); }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-pul-estesa],[data-pul-campione],[data-pul-annulla]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    try {
      if (b.hasAttribute("data-pul-estesa")) await invoke("richieste_pulizia_estesa_imposta", { acceso: !b.classList.contains("on") });
      else if (b.hasAttribute("data-pul-campione")) drySample = await invoke("richieste_campione_pulizia");
      else { const r = await invoke("richieste_annulla_pulizia"); toast(`${r?.riaperte || 0} richieste riaperte`); }
    } catch (x) { return toast("Non riuscito: " + x); }
    await open();
  }, true);
})();
