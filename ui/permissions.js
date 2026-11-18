// Chat permissions (Nov 12, src-tauri/src/permissions.rs).
// - Settings > Permissions: three levels (Always ask / File edits yes, commands on request / Never ask),
//   exceptions per project or folder, "Il pilota puo' consentire i permessi" switch (off by default).
//   "Mai chiedere" only turns on after a confirmation that explains the risk.
// - Chat settings (Model button): "Permessi di questa chat" (like the settings or a level of its own).
// - Chat header: "Senza permessi" label when the chat was started with "Mai chiedere".
// - One-time notice to accounts that move to "Chiedi sempre" with the update.
// Permission requests arrive in the usual question panel (disegnaDomanda in index.html), with the buttons
// Allow / Always allow in this chat / Deny.
// Hooks in index.html (marked "permessi (12/11)"): window.permessiTesta in testa(), the option description in
// disegnaDomanda and the <script src="permissions.js"> tag at the bottom.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const D = () => { try { return dati || {}; } catch { return {}; } };
  const showToast = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const reload = () => { try { if (typeof caricaElenco === "function") caricaElenco(); } catch {} };

  const LEVELS = [
    ["chiedi", "Chiedi sempre", "Ogni comando o modifica ai file aspetta il tuo ok, qui e sull'iPhone"],
    ["modifiche", "Modifiche ai file sì, comandi su richiesta", "Claude modifica i file del progetto da solo; per i comandi chiede"],
    ["mai", "Mai chiedere", "Claude esegue comandi e modifica file senza chiedere. Le regole di divieto del tuo settings.json valgono sempre"],
  ];
  const name = (l) => (LEVELS.find((x) => x[0] === l) || LEVELS[0])[1];
  const SHORT = { chiedi: "Chiedi sempre", modifiche: "Modifiche ai file", mai: "Mai chiedere" };
  const RISK = "Claude potrà eseguire comandi e modificare file senza chiedere, anche cancellare, pubblicare o mandare messaggi. Usalo solo se ti fidi di quello che gli chiedi di fare. Le regole di divieto del tuo settings.json restano valide.";

  const css = `
.pm-liv{display:flex;align-items:flex-start;gap:11px;width:100%;text-align:left;padding:10px 12px;border:1px solid var(--line);border-radius:11px;margin-bottom:6px;background:transparent;color:inherit;font:inherit;cursor:pointer}
.pm-liv:hover{background:var(--fill)}
.pm-liv.on{border-color:var(--ink);box-shadow:inset 0 0 0 1px var(--ink)}
.pm-liv .pm-pt{flex:none;width:14px;height:14px;margin-top:2px;border-radius:50%;border:1.5px solid var(--ink3)}
.pm-liv.on .pm-pt{border:4px solid var(--ink)}
.pm-liv b{display:block;font-weight:600;font-size:13px}
.pm-liv small{display:block;color:var(--ink3);font-size:11.5px;margin-top:1px}
.pm-liv .pm-risk{display:inline-block;margin-left:6px;font-size:10.5px;font-weight:600;padding:1px 6px;border-radius:5px;background:var(--fill2);color:var(--ink2);vertical-align:1px}
.pm-ecc select,.pm-agg select{height:28px;border-radius:7px;border:1px solid var(--line);background:var(--fill);color:var(--ink);font:inherit;font-size:12px;padding:0 6px;outline:none;max-width:200px}
.pm-agg{display:flex;flex-wrap:wrap;gap:6px;align-items:center;padding:10px 12px;border:1px dashed var(--line);border-radius:11px;margin-bottom:6px}
.pm-agg input{flex:1;min-width:140px;height:28px;border-radius:7px;border:1px solid var(--line);background:var(--fill);padding:0 9px;font:inherit;font-size:12px;color:var(--ink);outline:none}
.pm-agg .tasto,.pm-ecc .tasto{border:1px solid var(--line);height:28px;font-size:12px}
.pm-etich{display:inline-flex;align-items:center;gap:5px;flex:none;height:22px;padding:0 8px;border-radius:7px;font-size:11px;font-weight:600;letter-spacing:.01em;background:var(--ink);color:var(--bg);white-space:nowrap;cursor:default}
.pm-etich svg{width:11px;height:11px}
.pm-velo{position:fixed;inset:0;z-index:300;background:rgba(0,0,0,.32);display:flex;align-items:center;justify-content:center;animation:pmIn .15s ease}
@keyframes pmIn{from{opacity:0}to{opacity:1}}
.pm-box{width:min(430px,92vw);background:var(--bg);color:var(--ink);border:1px solid var(--line);border-radius:16px;padding:20px 20px 16px;box-shadow:0 24px 60px rgba(0,0,0,.3)}
.pm-box h3{margin:0 0 8px;font-size:16px;font-weight:650}
.pm-box p{margin:0 0 10px;font-size:13px;line-height:1.5;color:var(--ink2)}
.pm-box .pm-az{display:flex;justify-content:flex-end;gap:8px;margin-top:14px}
.pm-box .pm-az button{height:30px;padding:0 13px;border-radius:9px;font:inherit;font-size:12.5px;cursor:pointer}
.pm-box .pm-no{border:1px solid var(--line);background:transparent;color:var(--ink)}
.pm-box .pm-si{border:0;background:var(--ink);color:var(--bg);font-weight:600}
.regola-pop .pm-seg button{font-size:11.5px}
`;
  const st = document.createElement("style"); st.textContent = css; document.head.appendChild(st);

  // ---------- small confirmation dialog (above Settings, which stay open) ----------
  function askConfirm(titleText, bodyText, si, no = "Annulla") {
    return new Promise((result) => {
      const v = document.createElement("div"); v.className = "pm-velo";
      v.innerHTML = `<div class="pm-box" role="dialog" aria-modal="true"><h3>${E(titleText)}</h3>${bodyText.split("\n").map((t) => `<p>${E(t)}</p>`).join("")}<div class="pm-az">${no ? `<button class="pm-no">${E(no)}</button>` : ""}<button class="pm-si">${E(si)}</button></div></div>`;
      const finish = (x) => { v.remove(); document.removeEventListener("keydown", btns, true); result(x); };
      const btns = (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); finish(false); } else if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); finish(true); } };
      v.addEventListener("click", (e) => { if (e.target.closest(".pm-si")) finish(true); else if (e.target.closest(".pm-no") || e.target === v) finish(false); });
      document.addEventListener("keydown", btns, true);
      document.body.appendChild(v);
      v.querySelector(".pm-si")?.focus();
    });
  }
  const confirmNever = (where) => askConfirm(`Mai chiedere${where ? " " + where : ""}?`, RISK, "Sì, mai chiedere");

  // ---------- Settings > Permissions ----------
  let S = null;
  async function load() { try { S = await inv("permessi_stato"); } catch { S = null; } renderSettings(true); }
  function settingsHtml() {
    if (!S) return `<h4>Permessi</h4><p class="imp-nota">Carico…</p>`;
    const lvl = LEVELS.map(([v, t, d]) => `<button class="pm-liv ${S.livello === v ? "on" : ""}" data-pm-liv="${v}" role="radio" aria-checked="${S.livello === v}"><span class="pm-pt"></span><span><b>${E(t)}${v === "mai" ? `<span class="pm-risk">senza controlli</span>` : ""}</b><small>${E(d)}</small></span></button>`).join("");
    const pr = new Map((S.elenco_progetti || []).map((p) => [p.id, p]));
    const sel = (attrs, chiave, val) => `<select ${attrs}="${E(chiave)}">${LEVELS.map(([v]) => `<option value="${v}" ${val === v ? "selected" : ""}>${E(SHORT[v])}</option>`).join("")}<option value="">Togli eccezione</option></select>`;
    const exc = [
      ...Object.entries(S.progetti || {}).map(([k, l]) => `<div class="imp-riga pm-ecc"><div class="tx"><b>${E(pr.get(k)?.nome || k)}</b><small>Progetto${pr.get(k)?.cartella ? " · " + E(pr.get(k).cartella) : ""}</small></div>${sel("data-pm-progetto", k, l)}</div>`),
      ...Object.entries(S.cartelle || {}).map(([k, l]) => `<div class="imp-riga pm-ecc"><div class="tx"><b>${E(k.split("/").filter(Boolean).pop() || k)}</b><small>Cartella · ${E(k)}</small></div>${sel("data-pm-cartella", k, l)}</div>`),
    ].join("");
    const free = (S.elenco_progetti || []).filter((p) => !(p.id in (S.progetti || {})));
    return `<h4>Permessi</h4>${lvl}
      <p class="imp-nota">Vale per le chat nuove e per quelle che ripartono. Le chat ferme ripartono subito con il livello nuovo, quelle al lavoro appena finiscono il turno.</p>
      <h4 style="margin-top:14px">Eccezioni</h4>${exc || ""}
      <div class="pm-agg">${free.length ? `<select id="pm-nuovo-prog"><option value="">Progetto…</option>${free.map((p) => `<option value="${E(p.id)}">${E(p.nome)}</option>`).join("")}</select>` : ""}<input id="pm-nuova-cart" placeholder="oppure una cartella, es. ~/Clienti" spellcheck="false" autocomplete="off">
        <select id="pm-nuovo-liv">${LEVELS.map(([v]) => `<option value="${v}">${E(SHORT[v])}</option>`).join("")}</select><button class="tasto" data-pm="aggiungi">Aggiungi</button></div>
      <p class="imp-nota">Un'eccezione vale per tutte le chat di quel progetto o dentro quella cartella. Ogni chat può avere anche il suo livello, dal tasto del modello nella testata.</p>
      <div class="imp-riga" style="margin-top:10px"><div class="tx"><b>Il pilota può consentire i permessi</b><small>Spento: il pilota automatico al massimo consiglia, decidi tu. Acceso: consente da solo quello che giudica sicuro, dopo l'attesa del pilota</small></div><button class="interr ${S.pilota_permessi ? "on" : ""}" data-pm="pilota" role="switch" aria-checked="${!!S.pilota_permessi}" title="${S.pilota_permessi ? "Spegni" : "Accendi"}"></button></div>`;
  }
  function renderSettings(force) {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-permessi");
    if (!sec) {
      // after "Tu" (or at the top if missing): it is a security choice, it must be visible right away
      const sections = [...f.querySelectorAll(".imp-sez")];
      const anchor = sections.find((x) => x.querySelector("h4")?.textContent === "Tu") || null;
      if (!sections.length) return;
      sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-permessi";
      if (anchor) anchor.after(sec); else sections[0].before(sec);
      if (!S && !force) load();
    }
    const h = settingsHtml();
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (sheet.classList.contains("imp") && !document.getElementById("imp-permessi")) { S = null; renderSettings(); } }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });

  async function outcome(r, ok) {
    if (r?.ok) showToast(ok + (r.riparte ? ` · ${r.riparte === 1 ? "1 chat riparte" : r.riparte + " chat ripartono"}` : ""), 2600);
    else showToast("Non salvato: " + (r?.msg || "errore"), 3000);
    await load(); reload();
  }
  document.addEventListener("click", async (e) => {
    const l = e.target.closest?.("[data-pm-liv]");
    if (l) {
      e.preventDefault(); e.stopPropagation();
      const v = l.dataset.pmLiv; if (S && S.livello === v) return;
      let confirmed = false;
      if (v === "mai") { confirmed = await confirmNever(); if (!confirmed) return; }
      return outcome(await inv("permessi_imposta", { livello: v, confermato: confirmed }), "Permessi: " + name(v));
    }
    const b = e.target.closest?.("[data-pm]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    if (b.dataset.pm === "pilota") return outcome(await inv("permessi_pilota", { acceso: !b.classList.contains("on") }), b.classList.contains("on") ? "Il pilota non consente più i permessi" : "Il pilota può consentire i permessi");
    if (b.dataset.pm === "aggiungi") {
      const proj = document.getElementById("pm-nuovo-prog")?.value || "", dir = (document.getElementById("pm-nuova-cart")?.value || "").trim(), lvl = document.getElementById("pm-nuovo-liv")?.value || "chiedi";
      if (!proj && !dir) return showToast("Scegli un progetto o scrivi una cartella", 2400);
      let confirmed = false;
      if (lvl === "mai") { confirmed = await confirmNever(dir ? "in questa cartella" : "in questo progetto"); if (!confirmed) return; }
      return outcome(await inv("permessi_eccezione", { tipo: dir ? "cartella" : "progetto", chiave: dir || proj, livello: lvl, confermato: confirmed }), "Eccezione aggiunta");
    }
  }, true);
  document.addEventListener("change", async (e) => {
    const s = e.target.closest?.("[data-pm-progetto],[data-pm-cartella]"); if (!s) return;
    const tipo = s.dataset.pmProgetto !== undefined ? "progetto" : "cartella", chiave = s.dataset.pmProgetto ?? s.dataset.pmCartella, lvl = s.value;
    let confirmed = false;
    if (lvl === "mai") { confirmed = await confirmNever(tipo === "cartella" ? "in questa cartella" : "in questo progetto"); if (!confirmed) return renderSettings(); }
    outcome(await inv("permessi_eccezione", { tipo, chiave, livello: lvl, confermato: confirmed }), lvl ? "Eccezione: " + name(lvl) : "Eccezione tolta");
  }, true);

  // ---------- chat header: "Senza permessi" ----------
  window.permessiTesta = (d) => d && !d.locale && d.permessi_livello === "mai"
    ? `<span class="pm-etich" title="Questa chat è partita con «Mai chiedere»: esegue comandi e modifica file senza chiedere. Cambia dal tasto del modello o in Impostazioni > Permessi"><svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M6 1.2 2 2.8v3c0 2.4 1.7 4.2 4 5 2.3-.8 4-2.6 4-5v-3z"/><path d="M3.5 8.5l5-5"/></svg>Senza permessi</span>` : "";

  // ---------- Chat settings: permission level of this chat ----------
  new MutationObserver(() => {
    const popup = document.querySelector(".regola-pop");
    if (!popup || popup.querySelector(".pm-seg") || !popup.querySelector('[data-rg="modello"]')) return;
    let d = null; try { d = det; } catch {}
    if (!d || d.locale) return;
    const current = d.permessi_chat || "";
    const voci = [["", "Come le impostazioni"], ["chiedi", "Chiedi"], ["modifiche", "Modifiche"], ["mai", "Mai chiedere"]];
    const hint = popup.querySelector(".rg-nota");
    const blk = document.createElement("div");
    blk.innerHTML = `<h5>Permessi di questa chat${d.permessi_livello ? `<span>adesso: ${E(name(d.permessi_livello))}</span>` : ""}</h5><div class="rg-seg pm-seg">${voci.map(([v, t]) => `<button data-pm-chat="${v}" class="${v === current ? "on" : ""}">${E(t)}</button>`).join("")}</div>`;
    if (hint) hint.before(...blk.childNodes); else popup.append(...blk.childNodes);
  }).observe(document.body, { childList: true });
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-pm-chat]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    let d = null; try { d = det; } catch {}
    if (!d) return;
    const v = b.dataset.pmChat;
    let confirmed = false;
    if (v === "mai") { confirmed = await confirmNever("in questa chat"); if (!confirmed) return; }
    document.querySelector(".regola-pop")?.remove();
    const r = await inv("permessi_chat", { id: d.sessione, livello: v, confermato: confirmed });
    showToast(r?.ok ? r.msg : "Non salvato: " + (r?.msg || "errore"), 2600);
    try { if (typeof caricaDettaglio === "function") caricaDettaglio(false); } catch {}
    reload();
  }, true);

  // ---------- one-time notice (update: this account moves to "Chiedi sempre") ----------
  let noticeShown = false;
  async function checkNotice() {
    if (noticeShown || !D().imp?.permessi_avviso) return;
    noticeShown = true;
    const openSettings = await askConfirm("Adesso Claude chiede prima di agire",
      "Da questa versione le chat di Juggl chiedono il tuo ok prima di eseguire comandi o modificare file: la richiesta compare nella chat con i tasti Consenti, Consenti sempre in questa chat e Nega, anche sull'iPhone.\nLe chat già al lavoro restano come sono finché non ripartono. Puoi cambiare livello in Impostazioni > Permessi.",
      "Apri Impostazioni", "Ho capito");
    await inv("permessi_avviso_visto").catch(() => {});
    // same event as the Settings menu item (listened to by index.html)
    if (openSettings) TT.event.emit("impostazioni").catch(() => {});
  }
  setInterval(checkNotice, 3000);
  setTimeout(checkNotice, 1200);
})();
