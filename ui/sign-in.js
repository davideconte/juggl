// "Entra in Juggl" (Nov 11): without a valid license (src-tauri/src/license.rs) the window shows only this screen,
// full window: email -> 6-digit code -> account state (pending, suspended, rejected). When the license
// becomes valid ("licenza" event) the screen goes away by itself. Contract: ~/tavolo/account/CONTRATTO.md
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  const css = `
html.senza-licenza body > *:not(#entra){visibility:hidden!important}
#entra,#entra *{box-sizing:border-box}
#entra{position:fixed;inset:0;z-index:2147483000;background:var(--bg);color:var(--ink);display:flex;flex-direction:column;font:13.5px/1.5 var(--sans);-webkit-user-select:none;user-select:none}
#entra .e-testa{height:52px;flex:none}
#entra .e-corpo{flex:1;display:flex;align-items:center;justify-content:center;padding:0 24px 40px}
#entra .e-scheda{width:100%;max-width:380px;display:flex;flex-direction:column;align-items:center;text-align:center}
#entra .e-icona{width:64px;height:64px;margin-bottom:20px;filter:drop-shadow(0 8px 20px rgba(0,0,0,.16))}
#entra .e-icona .fondo{fill:#161616}#entra .e-icona .segni{fill:#fff;color:#fff}
@media (prefers-color-scheme: dark){#entra .e-icona .fondo{fill:#f3f3f1}#entra .e-icona .segni{fill:#161616;color:#161616}#entra .e-icona{filter:none}}
#entra h1{font:600 28px/1.15 var(--serif);letter-spacing:-.015em;margin:0 0 8px}
#entra .e-sotto{color:var(--ink2);font-size:14px;margin:0 0 22px;text-wrap:balance}
#entra .e-sotto b{color:var(--ink);font-weight:600}
#entra form{width:100%;display:flex;flex-direction:column;gap:10px;margin:0}
#entra input{font:inherit;color:inherit;width:100%;height:40px;border-radius:10px;border:1px solid var(--line);background:var(--fill);padding:0 13px;outline:none;font-size:14.5px;-webkit-user-select:text;user-select:text}
#entra input:focus{border-color:var(--ink3);background:var(--bg)}
#entra input.e-codice{height:52px;text-align:center;font:600 26px/1 var(--mono);letter-spacing:.42em;padding-left:.42em}
#entra .e-nero{height:40px;border-radius:10px;background:var(--ink);color:var(--bg);font:500 14px var(--sans);border:0;display:flex;align-items:center;justify-content:center;gap:8px;transition:opacity .15s}
#entra .e-nero:disabled{opacity:.35}
#entra .e-link{background:none;border:0;color:var(--ink2);font:13px var(--sans);padding:6px 8px;border-radius:7px}
#entra .e-link:hover{color:var(--ink);background:var(--fill)}
#entra .e-link u{text-underline-offset:2px}
#entra .e-altri{display:flex;justify-content:center;gap:4px;margin-top:10px;flex-wrap:wrap}
#entra .e-errore{color:var(--err);font-size:12.5px;min-height:18px;margin:2px 0 0}
#entra .e-stato{width:100%;border:1px solid var(--line);border-radius:14px;padding:14px 16px;background:var(--side);text-align:left;margin:0 0 16px;display:flex;gap:12px;align-items:center}
#entra .e-stato .e-punto{width:9px;height:9px;border-radius:50%;flex:none;background:var(--ink3)}
#entra .e-stato.attesa .e-punto{background:var(--ink);animation:e-batte 1.6s ease-in-out infinite}
#entra .e-stato b{display:block;font-weight:600;font-size:13.5px}
#entra .e-stato small{color:var(--ink2);font-size:12.5px;word-break:break-all}
#entra .e-piede{flex:none;text-align:center;color:var(--ink3);font-size:11.5px;padding:0 24px 22px}
#entra .e-giro{width:14px;height:14px;border-radius:50%;border:2px solid currentColor;border-right-color:transparent;animation:e-gira .7s linear infinite}
@keyframes e-gira{to{transform:rotate(360deg)}}
@keyframes e-batte{50%{opacity:.35}}
@media (prefers-reduced-motion:reduce){#entra .e-giro,#entra .e-stato.attesa .e-punto{animation:none}}`;

  const ICON = `<svg class="e-icona" viewBox="0 0 100 100" aria-hidden="true"><rect class="fondo" x="4" y="4" width="92" height="92" rx="24"/><g class="segni" transform="translate(19.64 19.64) scale(1.265)"><g transform="translate(9.05 -0.05)"><path d="M14 16V36a8 8 0 0 1-8 8h-2" fill="none" stroke="currentColor" stroke-width="8" stroke-linecap="round"/><circle cx="23.9" cy="6.1" r="6" fill="#ff5b1f"/></g></g></svg>`;

  let ST = null;          // license state from licenza_stato / "licenza" event
  let step = "email";    // email | codice (the other steps come from the state)
  let email = "";
  let busy = false;
  let error = "";
  let el = null;

  function view() {
    if (!ST || ST.valida) return null;
    if (step === "codice" && !ST.entrato) return "codice";
    if (!ST.entrato) return "email";
    const m = ST.motivo;
    if (m === "in_attesa" || m === "sospeso" || m === "rifiutato") return m;
    return "rinnovo";
  }

  function spinner(t) { return busy ? `<span class="e-giro" aria-hidden="true"></span>` : E(t); }

  function contenuto(v) {
    if (v === "email") return `
      <h1>Entra in Juggl</h1>
      <p class="e-sotto">Scrivi la tua email: ti mandiamo un codice di 6 cifre.</p>
      <form data-e="codice">
        <input type="email" id="e-email" placeholder="nome@esempio.it" autocomplete="email" spellcheck="false" value="${E(email)}" required>
        <button class="e-nero" ${busy ? "disabled" : ""}>${spinner("Ricevi il codice")}</button>
        <p class="e-errore">${E(error)}</p>
      </form>
      <div class="e-altri"><button class="e-link" data-e="richiedi">Non hai un account? <u>Richiedilo su juggl.dev</u></button></div>`;
    if (v === "codice") return `
      <h1>Controlla l'email</h1>
      <p class="e-sotto">Abbiamo mandato un codice di 6 cifre a <b>${E(email)}</b>. Vale 10 minuti.</p>
      <form data-e="entra">
        <input class="e-codice" id="e-codice" inputmode="numeric" autocomplete="one-time-code" maxlength="6" placeholder="000000" spellcheck="false">
        <button class="e-nero" ${busy ? "disabled" : ""}>${spinner("Entra")}</button>
        <p class="e-errore">${E(error)}</p>
      </form>
      <div class="e-altri"><button class="e-link" data-e="altra">Usa un'altra email</button><button class="e-link" data-e="di-nuovo">Manda di nuovo</button></div>`;
    const acct = `<small>${E(ST.email)}</small>`;
    if (v === "in_attesa") return `
      <h1>Account in attesa</h1>
      <p class="e-sotto">Lo attiviamo a mano e ti arriva una email. Poi Juggl si sblocca da solo.</p>
      <div class="e-stato attesa"><i class="e-punto"></i><div><b>In attesa di attivazione</b>${acct}</div></div>
      ${actions()}`;
    if (v === "sospeso") return `
      <h1>Account sospeso</h1>
      <p class="e-sotto">Questo account al momento non può usare Juggl. Per informazioni scrivi a <b>hello@juggl.dev</b>.</p>
      <div class="e-stato"><i class="e-punto"></i><div><b>Sospeso</b>${acct}</div></div>
      ${actions()}`;
    if (v === "rifiutato") return `
      <h1>Account rifiutato</h1>
      <p class="e-sotto">La richiesta di accesso non è stata accettata. Per informazioni scrivi a <b>hello@juggl.dev</b>.</p>
      <div class="e-stato"><i class="e-punto"></i><div><b>Rifiutato</b>${acct}</div></div>
      ${actions()}`;
    // license expired with no network, or no longer valid for other reasons
    return `
      <h1>Licenza da rinnovare</h1>
      <p class="e-sotto">${ST.errore === "rete" ? "Juggl non riesce a raggiungere juggl.dev. Collegati a internet e riprova." : "Juggl deve ricontrollare il tuo account su juggl.dev."}</p>
      <div class="e-stato"><i class="e-punto"></i><div><b>Licenza non valida</b>${acct}</div></div>
      ${actions()}`;
  }

  function actions() {
    return `<button class="e-nero" data-e="controlla" style="width:100%" ${busy ? "disabled" : ""}>${spinner("Controlla di nuovo")}</button>
      <p class="e-errore">${E(error)}</p>
      <div class="e-altri"><button class="e-link" data-e="esci">Esci e usa un'altra email</button></div>`;
  }

  function render() {
    const v = view();
    if (!v) {
      if (el) { el.remove(); el = null; }
      document.documentElement.classList.remove("senza-licenza");
      return;
    }
    if (!el) {
      el = document.createElement("div");
      el.id = "entra";
      el.setAttribute("role", "dialog");
      el.setAttribute("aria-modal", "true");
      document.body.appendChild(el);
    }
    document.documentElement.classList.add("senza-licenza");
    const first = document.activeElement?.id;
    el.dataset.vista = v;
    el.innerHTML = `<div class="e-testa" data-tauri-drag-region></div>
      <div class="e-corpo" data-tauri-drag-region><div class="e-scheda">${ICON}${contenuto(v)}</div></div>
      <div class="e-piede">Le chat restano su questo Mac: a juggl.dev vanno solo i dati dell'account.</div>`;
    const f = el.querySelector("#" + (first && el.querySelector("#" + first) ? first : v === "codice" ? "e-codice" : "e-email"));
    if (f && !busy) { f.focus(); if (f.id === "e-email") f.setSelectionRange?.(f.value.length, f.value.length); }
  }

  async function requestCode() {
    const i = el?.querySelector("#e-email");
    if (i) email = i.value.trim().toLowerCase();
    if (!email || !email.includes("@")) { error = "Scrivi la tua email."; return render(); }
    busy = true; error = ""; render();
    const r = await inv("account_codice", { email }).catch((e) => ({ ok: false, msg: String(e) }));
    busy = false;
    if (r.ok) { step = "codice"; error = ""; } else error = r.msg || "Non riuscito";
    render();
  }

  async function signIn() {
    const codice = (el?.querySelector("#e-codice")?.value || "").replace(/\D/g, "");
    if (codice.length !== 6) { error = "Il codice ha 6 cifre."; return render(); }
    busy = true; error = ""; render();
    const r = await inv("account_entra", { email, codice }).catch((e) => ({ ok: false, msg: String(e) }));
    busy = false;
    if (r.ok) { step = "email"; ST = r; error = ""; } else error = r.msg || "Non riuscito";
    render();
  }

  async function check() {
    busy = true; error = ""; render();
    const r = await inv("licenza_rinnova").catch(() => null);
    busy = false;
    if (r) ST = r;
    if (ST && !ST.valida && ST.errore === "rete") error = "Nessuna risposta da juggl.dev.";
    else if (ST && !ST.valida && view() === "in_attesa") error = "Ancora in attesa.";
    render();
  }

  async function signOut() {
    ST = await inv("account_esci").catch(() => ST);
    email = ST?.email_ultima || email; step = "email"; error = "";
    render();
  }

  document.addEventListener("submit", (e) => {
    if (!el || !el.contains(e.target)) return;
    e.preventDefault();
    if (busy) return;
    if (e.target.dataset.e === "codice") requestCode(); else signIn();
  }, true);
  document.addEventListener("click", (e) => {
    if (!el) return;
    if (!el.contains(e.target)) { e.stopPropagation(); e.preventDefault(); return; }
    const b = e.target.closest("[data-e]");
    if (!b || b.tagName === "FORM" || busy) return;
    const a = b.dataset.e;
    if (a === "richiedi") inv("apri_link", { url: "https://juggl.dev" });
    if (a === "altra") { step = "email"; error = ""; render(); }
    if (a === "di-nuovo") { requestCode().then(() => { if (!error && typeof toast === "function") toast("Codice mandato di nuovo", 2000); }); }
    if (a === "controlla") check();
    if (a === "esci") signOut();
  }, true);
  document.addEventListener("input", (e) => {
    if (!el || e.target.id !== "e-codice") return;
    const t = e.target.value.replace(/\D/g, "").slice(0, 6);
    if (t !== e.target.value) e.target.value = t;
    if (t.length === 6 && !busy) signIn();      // code complete (also pasted or autofilled): sign in
  }, true);
  // with the screen in front, app shortcuts must not reach the UI underneath
  for (const kind of ["keydown", "keyup", "keypress"]) {
    window.addEventListener(kind, (e) => {
      if (!el) return;
      e.stopImmediatePropagation();
      if (kind === "keydown" && e.key === "Escape") e.preventDefault();
    }, true);
  }

  const st = document.createElement("style");
  st.textContent = css;
  document.head.appendChild(st);

  function refresh(s) {
    const first = ST;
    ST = s;
    if (!email) email = s.email || s.email_ultima || "";
    render();
    if (first && !first.valida && s.valida && typeof toast === "function") toast("Account attivo: benvenuto in Juggl", 2600);
  }
  TT.event.listen("licenza", (ev) => refresh(ev.payload));
  inv("licenza_stato").then(refresh).catch(() => {});
})();
