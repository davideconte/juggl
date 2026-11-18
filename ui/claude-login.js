// Switch Claude account (Nov 12, src-tauri/src/claude_login.rs).
// Juggl chats are `claude -p`: "/login" does not exist. Here login is done from Juggl, in three steps:
//   1. the Claude sign-in page opens (or the link is copied),
//   2. paste the code the page shows (not needed if the page returns to Juggl by itself),
//   3. result: "Account cambiato: <email>, N chat si riavviano da sole".
// Account details (Nov 14): email, plan, organization, access expiry, model, usage with reset
// time, where the credential comes from; buttons Refresh / Switch account / Sign out (accesso_dettagli).
// Shown in Settings ("Account Claude" section, with plan and Sign out button) and in the quota popover.
// Messages for "/..." commands that do not exist in -p: window.accessoComando(r), called by index.html (manda) and
// by the "comando-non-disponibile" event.
// Hooks in index.html (marked "cambia account (12/11)"): manda() and the <script src="claude-login.js"> tag.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const PLANS = { max_20x: "Max 20x", max_5x: "Max 5x", max: "Max", pro: "Pro", team: "Team", enterprise: "Enterprise", free: "Gratis" };
  const planName = (p) => PLANS[String(p || "").toLowerCase()] || (p ? String(p) : "");

  const css = `
.ac-vel{position:fixed;inset:0;z-index:90;background:rgba(0,0,0,.28);display:flex;align-items:center;justify-content:center;animation:sale .18s ease}
.ac-dlg{width:min(430px,calc(100vw - 32px));border-radius:16px;background:var(--bg);border:1px solid var(--line);box-shadow:0 22px 60px rgba(0,0,0,.28);padding:18px 18px 14px}
.ac-dlg h4{margin:0 0 4px;font-size:15px;font-weight:600}
.ac-dlg p{margin:6px 0 0;font-size:13px;line-height:1.45;color:var(--ink2)}
.ac-dlg small{display:block;margin-top:6px;font-size:11.5px;line-height:1.4;color:var(--ink3)}
.ac-passi{display:flex;gap:6px;margin:0 0 12px}
.ac-passi i{flex:1;height:3px;border-radius:2px;background:var(--line)}
.ac-passi i.on{background:var(--ink)}
.ac-az{display:flex;gap:8px;justify-content:flex-end;align-items:center;margin-top:14px;flex-wrap:wrap}
.ac-az .sx{margin-right:auto}
.ac-dlg input{width:100%;height:36px;margin-top:10px;border-radius:9px;border:1px solid var(--line);background:var(--fill);color:inherit;font:inherit;font-size:13px;padding:0 10px;outline:none;-webkit-user-select:text;user-select:text}
.ac-dlg input:focus{background:var(--bg);border-color:var(--ink3)}
.ac-dlg .ac-err{color:var(--err)}
.ac-dlg .ac-ok b{font-weight:600;color:var(--ink)}
.ac-sez .ac-chi{display:flex;align-items:baseline;gap:8px;min-width:0}
.ac-sez .ac-chi b{font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.ac-sez .ac-piano{flex:none;font-size:10.5px;font-weight:600;letter-spacing:.03em;text-transform:uppercase;color:var(--ink3);border:1px solid var(--line);border-radius:5px;padding:0 4px}
.ac-det{margin:6px 0 10px;display:flex;flex-direction:column;gap:7px;font-size:13px}
.ac-det small{display:block;font-size:11.5px;line-height:1.4;color:var(--ink3);margin-top:1px}
.ac-r,.ac-cons{display:grid;grid-template-columns:150px 1fr;gap:2px 10px;align-items:baseline}
.ac-r>span:first-child,.ac-cons>span:first-child{color:var(--ink2)}
.ac-v{min-width:0;overflow-wrap:anywhere;color:var(--ink)}
.ac-v b{font-weight:600}
.ac-spento{color:var(--ink3)}
.ac-sep{height:1px;background:var(--line);margin:3px 0}
.ac-bar{grid-column:2;height:5px;border-radius:3px;background:var(--line);overflow:hidden}
.ac-bar i{display:block;height:100%;background:var(--ink);border-radius:3px}
.ac-eta{margin-top:0}
.ac-vuoto{padding:10px 12px;border:1px dashed var(--line);border-radius:10px}
.ac-vuoto b{font-weight:600}
.ac-tasti{display:flex;gap:8px;align-items:center;margin-top:4px}
.ac-tasti .sx{margin-right:auto}
.ac-nota{position:fixed;left:50%;bottom:92px;transform:translateX(-50%);z-index:80;width:min(460px,calc(100vw - 32px));border-radius:14px;background:var(--bg);border:1px solid var(--line);box-shadow:0 16px 44px rgba(0,0,0,.24);padding:12px 14px;animation:sale .2s ease}
.ac-nota b{font-size:12.5px;font-weight:600}
.ac-nota p{margin:4px 0 0;font-size:12.5px;line-height:1.45;color:var(--ink2)}
.ac-nota .ac-az{margin-top:10px}
`;
  const st = document.createElement("style"); st.id = "accesso-css"; st.textContent = css; document.head.appendChild(st);

  let ST = null;           // {account, login} from accesso_stato
  let FLOW = null;       // {passo: 1|2|3|"esci", link, manuale, esito, occupato, errore}
  let poll = null;

  async function load() {
    try { ST = await inv("accesso_stato"); } catch { ST = null; }
    rerender();
    return ST;
  }

  // ---------- account row: Settings and quota popover ----------
  function sectionHtml() {
    const a = ST?.account || {};
    const chi = a.collegato ? `<div class="ac-chi"><b>${E(a.email || "Account collegato")}</b>${a.piano ? `<span class="ac-piano">${E(planName(a.piano))}</span>` : ""}</div>` : `<div class="ac-chi"><b>Nessun account Claude collegato</b></div>`;
    const sotto = a.collegato ? (a.organizzazione && a.organizzazione !== a.email ? E(a.organizzazione) : "Claude Code, usato dalle chat di Juggl") : "Le chat non possono rispondere finché non entri.";
    return { chi, sotto, collegato: !!a.collegato };
  }
  // ---------- details (Nov 14) ----------
  let DET = null;          // answer of accesso_dettagli
  let DET_LOADING = false;  // reading
  const MONTHS = ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"];
  const pad2 = (n) => String(n).padStart(2, "0");
  function whenText(ts) {   // ts: Unix seconds (number) or ISO date (text)
    const d = typeof ts === "number" ? new Date(ts * 1000) : new Date(ts);
    if (!ts || isNaN(d)) return "";
    const today = new Date(); const tomorrow = new Date(today.getTime() + 86400000);
    const hhmm = `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
    if (d.toDateString() === today.toDateString()) return `oggi alle ${hhmm}`;
    if (d.toDateString() === tomorrow.toDateString()) return `domani alle ${hhmm}`;
    return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}, ${hhmm}`;
  }
  function untilText(r) {   // "tra 2 h 10 min" from Unix seconds
    const s = Math.round(r - Date.now() / 1000);
    if (s <= 0) return "";
    const m = Math.round(s / 60), h = Math.floor(m / 60), g = Math.floor(h / 24);
    return g >= 1 ? `tra ${g} g ${h % 24} h` : h >= 1 ? `tra ${h} h ${m % 60} min` : `tra ${Math.max(1, m)} min`;
  }
  const MODELS = { opus: "Opus", sonnet: "Sonnet", haiku: "Haiku", default: "Predefinito di Claude", opusplan: "Opus (piano) + Sonnet" };
  const modelName = (m) => MODELS[String(m).toLowerCase()] || String(m);
  function age(min) { return min == null ? "" : min < 1 ? "aggiornato adesso" : min < 90 ? `aggiornato ${min} min fa` : `aggiornato ${Math.round(min / 60)} h fa`; }
  function barHtml(w, name) {
    if (!w || typeof w.used_percentage !== "number") return `<div class="ac-cons"><span>${name}</span><span class="ac-v ac-spento">non disponibile</span></div>`;
    const p = Math.max(0, Math.min(100, w.used_percentage));
    const az = w.resets_at ? `si azzera ${whenText(w.resets_at)}${untilText(w.resets_at) ? " (" + untilText(w.resets_at) + ")" : ""}` : "";
    return `<div class="ac-cons"><span>${name}</span><span class="ac-v"><b>${Math.round(p)}%</b> usato${az ? " · " + E(az) : ""}</span><div class="ac-bar" role="img" aria-label="${name}: ${Math.round(p)}% usato"><i style="width:${p}%"></i></div></div>`;
  }
  function rowHtml(k, v) { return v ? `<div class="ac-r"><span>${k}</span><span class="ac-v">${v}</span></div>` : ""; }
  function detailsHtml() {
    const d = DET;
    if (!d) return `<div class="ac-det"><small>${DET_LOADING ? "Leggo i dati dell'account…" : "Dati dell'account non disponibili."}</small></div>`;
    if (!d.collegato) return `<div class="ac-det"><div class="ac-vuoto"><b>Claude non è collegato</b><small>Le chat non possono rispondere finché non entri con il tuo account.</small></div></div>`;
    const c = d.credenziale || {};
    const source = c.dove === "portachiavi" ? "Portachiavi di macOS" : c.dove === "file" ? "File delle credenziali" : "Non trovata";
    const sourceDetail = c.dove === "portachiavi" ? `elemento «Claude Code-credentials»${c.file_esiste ? "; esiste anche un vecchio file nella cartella di Claude, non usato" : ""}` : c.dove === "file" ? E(c.nome || "") : "";
    const k = d.consumi || {};
    const e = d.scade ? whenText(d.scade) : "";
    return `<div class="ac-det">
      ${rowHtml("Email", E(d.email || "—"))}
      ${rowHtml("Piano", d.piano ? E(planName(d.piano)) : "")}
      ${rowHtml("Organizzazione", d.organizzazione && d.organizzazione !== d.email ? E(d.organizzazione) : "")}
      ${rowHtml("Accesso valido fino a", e ? E(e) : "")}
      ${rowHtml("Accesso rinnovato", c.data ? E(whenText(c.data)) : "")}
      ${rowHtml("Modello predefinito", d.modello ? E(modelName(d.modello)) : "<span class=\"ac-spento\">non impostato</span>")}
      ${rowHtml("Credenziale", `${E(source)}${sourceDetail ? `<small>${sourceDetail}</small>` : ""}`)}
      <div class="ac-sep"></div>
      ${barHtml(k.cinque_ore, "Finestra di 5 ore")}
      ${barHtml(k.settimana, "Settimana")}
      <small class="ac-eta">${k.eta_min != null ? E(age(k.eta_min)) : "Consumi non ancora letti"}${k.errore ? ` · ${E(k.errore)}` : ""}</small>
    </div>`;
  }
  async function loadDetails(refresh) {
    DET_LOADING = true; rerender();
    try { DET = await inv("accesso_dettagli", { aggiorna: !!refresh }); } catch { DET = null; }
    DET_LOADING = false; rerender();
  }
  function settingsHtml() {
    const { collegato } = sectionHtml();
    const info = DET ? DET.collegato : collegato;
    return `<h4>Account Claude</h4>
      ${detailsHtml()}
      <div class="ac-tasti"><button class="tasto bordo" data-ac="aggiorna" ${DET_LOADING ? "disabled" : ""}>${DET_LOADING ? "Aggiorno…" : "Aggiorna"}</button><span class="sx"></span>
        ${info ? `<button class="tasto" data-ac="esci">Esci</button>` : ""}<button class="btn-nero" data-ac="avvia">${info ? "Cambia account" : "Collega"}</button></div>
      <p class="imp-nota">Le chat ripartono da sole con l'account nuovo. Funziona anche dall'iPhone.</p>`;
  }
  function renderSettings() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-accesso");
    if (!sec) {
      const sections = [...f.querySelectorAll(".imp-sez")];
      if (!sections.length) return;
      // right before the Claude Code connection (or at the bottom if not found)
      const anchor = sections.find((x) => x.querySelector("h4")?.textContent === "Collegamento con Claude Code");
      sec = document.createElement("div"); sec.className = "imp-sez ac-sez"; sec.id = "imp-accesso";
      if (anchor) anchor.before(sec); else sections[sections.length - 1].after(sec);
      if (!ST) load();
      if (!DET && !DET_LOADING) loadDetails(false);
    }
    const h = settingsHtml();
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  function renderQuota() {
    const popup = document.querySelector(".quota-pop");
    if (!popup) return;
    let sec = popup.querySelector(".ac-quota");
    if (!sec) { sec = document.createElement("section"); sec.className = "ac-quota ac-sez"; popup.appendChild(sec); if (!ST) load(); }
    const { chi, collegato } = sectionHtml();
    const h = `<h5>Account Claude</h5><div class="imp-riga" style="display:flex;align-items:center;gap:8px;justify-content:space-between"><div class="tx" style="min-width:0">${chi}</div><button class="tasto bordo" data-ac="avvia">${collegato ? "Cambia account" : "Entra"}</button></div>`;
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  function rerender() { renderSettings(); renderQuota(); }

  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (sheet.classList.contains("imp") && !document.getElementById("imp-accesso")) { ST = null; DET = null; renderSettings(); } }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  // the quota popover redraws itself (innerHTML): the section is put back after every redraw
  new MutationObserver(() => { const p = document.querySelector(".quota-pop"); if (p && !p.querySelector(".ac-quota")) renderQuota(); }).observe(document.body, { childList: true, subtree: true });

  // ---------- steps window ----------
  function closeFlow() {
    clearInterval(poll); poll = null; FLOW = null;
    document.querySelector(".ac-vel")?.remove();
  }
  function stepsHtml(n) { return `<div class="ac-passi">${[1, 2, 3].map((i) => `<i class="${i <= n ? "on" : ""}"></i>`).join("")}</div>`; }
  function bodyHtml() {
    const F = FLOW;
    if (F.passo === "esci") {
      return `<h4>Uscire dall'account Claude?</h4><p>Le chat di Juggl non potranno rispondere finché non rientri con «Cambia account Claude». Le conversazioni restano salvate.</p>${F.errore ? `<p class="ac-err">${E(F.errore)}</p>` : ""}
        <div class="ac-az"><button class="tasto" data-ac="chiudi">Annulla</button><button class="tasto conferma" data-ac="esci-si" ${F.occupato ? "disabled" : ""}>${F.occupato ? "Esco…" : "Esci dall'account"}</button></div>`;
    }
    if (F.passo === 1) {
      return `${stepsHtml(1)}<h4>Cambia account Claude</h4>
        <p>${F.occupato ? "Preparo la pagina di accesso…" : "Accedi con l'account che vuoi usare. Se il browser è già dentro con un altro account, esci prima da claude.ai."}</p>${F.errore ? `<p class="ac-err">${E(F.errore)}</p>` : ""}
        <div class="ac-az"><button class="tasto" data-ac="annulla">Annulla</button>${F.errore ? `<button class="btn-nero" data-ac="riprova">Riprova</button>` : ""}</div>`;
    }
    if (F.passo === 2) {
      return `${stepsHtml(2)}<h4>Incolla il codice</h4>
        <p>Nella pagina di Claude accedi e tocca Autorizza: ti mostra un codice. Incollalo qui. Se la pagina ti riporta da sola a Juggl, non serve.</p>
        <input id="ac-codice" type="text" placeholder="Codice di Claude" spellcheck="false" autocomplete="off" autocapitalize="off" ${F.occupato ? "disabled" : ""}>
        ${F.errore ? `<p class="ac-err">${E(F.errore)}</p>` : ""}<small>Il codice va solo a Claude Code su questo Mac: Juggl non lo salva.</small>
        <div class="ac-az"><span class="sx"><button class="tasto" data-ac="apri">Apri la pagina</button><button class="tasto" data-ac="copia">Copia link</button></span><button class="tasto" data-ac="annulla">Annulla</button><button class="btn-nero" data-ac="conferma" ${F.occupato ? "disabled" : ""}>${F.occupato ? "Controllo…" : "Conferma"}</button></div>`;
    }
    const e = F.esito || {};
    return `${stepsHtml(3)}<h4>${e.ok ? "Fatto" : "Non è andata"}</h4><p class="${e.ok ? "ac-ok" : "ac-err"}">${E(e.msg || "")}</p>
      ${e.ok && e.piano ? `<small>Piano: ${E(planName(e.piano))}. I limiti in basso sono stati riletti adesso.</small>` : ""}
      <div class="ac-az">${e.ok ? "" : `<button class="tasto" data-ac="riprova">Riprova</button>`}<button class="btn-nero" data-ac="chiudi">${e.ok ? "Fatto" : "Chiudi"}</button></div>`;
  }
  function renderFlow() {
    if (!FLOW) return;
    let veil = document.querySelector(".ac-vel");
    if (!veil) { veil = document.createElement("div"); veil.className = "ac-vel"; veil.innerHTML = `<div class="ac-dlg" role="dialog" aria-modal="true"></div>`; document.body.appendChild(veil); }
    const dlg = veil.querySelector(".ac-dlg");
    // the code field is not lost when the window redraws
    const kept = dlg.querySelector("#ac-codice")?.value || "";
    dlg.innerHTML = bodyHtml();
    const c = dlg.querySelector("#ac-codice");
    if (c) { c.value = kept; if (!FLOW.occupato) c.focus(); }
  }
  async function startFlow() {
    closeFlow();
    FLOW = { passo: 1, occupato: true };
    renderFlow();
    const r = await inv("accesso_avvia").catch((e) => ({ ok: false, msg: String(e) }));
    if (!FLOW) return;
    if (!r.ok) { FLOW = { passo: 1, errore: r.msg || "Non riesco ad avviare il login" }; return renderFlow(); }
    FLOW = { passo: 2, link: r.manuale || r.link, mac: r.link };
    renderFlow();
    inv("apri_link", { url: FLOW.link });
    // if the page returns to Juggl by itself the login ends without a code: check every 2.5 seconds
    poll = setInterval(async () => {
      const s = await inv("accesso_stato").catch(() => null);
      const l = s?.login;
      if (!FLOW || !l) return;
      if (l.fase === "finito" || l.fase === "errore") finish(l.esito || { ok: false, msg: "Login chiuso" });
      else if (l.fase === "scaduto" || l.fase === "annullato" || l.fase === "nessuno") finish({ ok: false, msg: l.esito?.msg || "Il login si è chiuso. Riprova." });
    }, 2500);
  }
  function finish(esito) {
    clearInterval(poll); poll = null;
    FLOW = { passo: 3, esito };
    renderFlow();
    ST = null; load(); DET = null; loadDetails(false);
  }
  async function confirm() {
    const v = document.getElementById("ac-codice")?.value.trim() || "";
    if (!v) { FLOW.errore = "Incolla il codice che vedi nella pagina"; return renderFlow(); }
    FLOW.occupato = true; FLOW.errore = ""; renderFlow();
    const r = await inv("accesso_codice", { codice: v }).catch((e) => ({ ok: false, msg: String(e) }));
    if (!FLOW) return;
    if (r.ok) return finish(r);
    // wrong or slow: the field stays, the login goes on
    FLOW.occupato = false; FLOW.errore = r.msg || "Codice non accettato";
    const afterEl = document.getElementById("ac-codice")?.value; renderFlow();
    if (afterEl) document.getElementById("ac-codice").value = afterEl;
  }
  async function signOut() {
    FLOW.occupato = true; FLOW.errore = ""; renderFlow();
    const r = await inv("accesso_esci", { conferma: true }).catch((e) => ({ ok: false, msg: String(e) }));
    if (!r.ok) { FLOW.occupato = false; FLOW.errore = r.msg; return renderFlow(); }
    closeFlow(); notice(r.msg, 4500); ST = null; load(); DET = null; loadDetails(false);
  }

  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-ac]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    const a = b.dataset.ac;
    if (a === "aggiorna") { load(); return loadDetails(true); }
    if (a === "avvia" || a === "riprova") { document.querySelector(".ac-nota")?.remove(); return startFlow(); }
    if (a === "esci") { closeFlow(); FLOW = { passo: "esci" }; return renderFlow(); }
    if (a === "esci-si") return signOut();
    if (a === "conferma") return confirm();
    if (a === "apri") return FLOW?.link && inv("apri_link", { url: FLOW.link });
    if (a === "copia") return FLOW?.link && inv("copia_testo", { testo: FLOW.link }).then(() => notice("Link copiato", 1800));
    if (a === "annulla") { inv("accesso_annulla").catch(() => {}); return closeFlow(); }
    if (a === "chiudi") return closeFlow();
    if (a === "nota-chiudi") return document.querySelector(".ac-nota")?.remove();
  }, true);
  document.addEventListener("keydown", (e) => {
    if (!FLOW) return;
    if (e.key === "Enter" && FLOW.passo === 2 && e.target?.id === "ac-codice") { e.preventDefault(); confirm(); }
    if (e.key === "Escape") { e.stopPropagation(); if (FLOW.passo === 2 || FLOW.passo === 1) inv("accesso_annulla").catch(() => {}); closeFlow(); }
  }, true);

  // ---------- "/..." commands that do not exist in -p ----------
  // r = {comando, msg, azione}: card at the bottom, with the button if needed (Cambia account Claude)
  window.accessoComando = (r) => {
    document.querySelector(".ac-nota")?.remove();
    const n = document.createElement("div"); n.className = "ac-nota";
    n.innerHTML = `<b>${E(r.comando || "Comando")}</b><p>${E(r.msg || "")}</p><div class="ac-az">${r.azione === "cambia-account" ? `<button class="btn-nero" data-ac="avvia">Cambia account Claude</button>` : ""}<button class="tasto" data-ac="nota-chiudi">Ho capito</button></div>`;
    document.body.appendChild(n);
    clearTimeout(n._t); n._t = setTimeout(() => n.remove(), r.azione ? 30000 : 14000);
  };
  TT.event.listen("comando-non-disponibile", (ev) => {
    const p = ev.payload || {};
    let isOpen = null; try { isOpen = det?.sessione; } catch {}
    if (!p.id || p.id === isOpen) window.accessoComando(p);
  });
  TT.event.listen("accesso-finito", () => { ST = null; load(); DET = null; if (document.getElementById("imp-accesso")) loadDetails(false); if (FLOW && FLOW.passo === 2) {} });
  window.accessoAvvia = startFlow;
  window.accessoEsci = () => { FLOW = { passo: "esci" }; renderFlow(); };   // for /logout from the commands menu (confirmation included)
})();
