// snap (Nov 16): "Scatto in Juggl" (src-tauri/src/snap.rs). F16 (by default) from any app acts like ⌘⇧4; the image
// arrives here: in the attachments of the open chat with the input box ready, or (with Alt) in the Juggl chat.
// Also here: explanation of the Screen Recording permission before the first snap, message if missing, the
// "Scatto in Juggl" section in Settings, entry for Settings by voice (window.impostazioniVociExtra).
// Mac only: elsewhere scatto_stato says mac:false and nothing is shown here.
// Hooks in index.html: only the <script src="snap.js"> tag.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const q = (s) => document.querySelector(s);
  let ST = null;   // {mac, tasto, alt, base, permesso, chiesto}
  const load = async () => { ST = await inv("scatto_stato").catch(() => null); return ST; };

  // "Shift+Cmd+2" -> "⇧⌘2", "Alt+F16" -> "⌥F16"
  const pretty = (t) => !t ? "spento" : t.split("+").map((p) => ({ Cmd: "⌘", Shift: "⇧", Alt: "⌥", Ctrl: "⌃" })[p] || p).join("");

  // ---------- the image arrives ----------
  TT.event.listen("scatto", async (ev) => {
    const { path, alt } = ev.payload || {};
    if (!path) return;
    if (!alt && window.aiutoPrendiScatto?.(path)) return;   // help and reports (Nov 17): with the help chat open it goes there
    try {
      const inChat = typeof vistaMac !== "undefined" && !vistaMac && !(typeof saAperto !== "undefined" && saAperto);
      const V = inChat && typeof VA === "function" ? VA() : null;
      if (!alt && V?.id) { aggiungiPercorsi([path], V); return; }   // attachment + input ready (aggiungiPercorsi sets focus)
      const jid = typeof dati !== "undefined" ? dati.juggl : "";
      if (jid && typeof allegaAChat === "function") { allegaAChat(jid, [path], true); return; }
      notice("Scatto salvato, ma non c'è una chat dove metterlo", 3000);
    } catch (e) { console.error(e); }
  });

  // ---------- Screen Recording permission ----------
  function sheet(h) {
    const f = q("#foglio"); if (!f) return;
    f.classList.remove("salto", "imp", "ic"); f.innerHTML = h; q("#velo")?.classList.add("on");
  }
  const close = () => (typeof chiudiNuova === "function" ? chiudiNuova() : q("#velo")?.classList.remove("on"));
  let pendingAlt = false;
  TT.event.listen("scatto-permesso", async (ev) => {
    const { stato, alt } = ev.payload || {}; pendingAlt = !!alt;
    await load();
    const keyName = pretty(ST?.tasto || "F16");
    if (stato === "da_chiedere") sheet(`<h3>Scatto in Juggl</h3>
      <p class="conferma-testo">Con ${E(keyName)} fotografi una parte dello schermo e la mandi alla chat aperta, come ⌘⇧4. Per farlo Juggl ha bisogno del permesso <b>Registrazione schermo</b>.</p>
      <p class="conferma-testo">Lo usa solo quando premi ${E(keyName)}: niente foto automatiche. macOS te lo chiede adesso, una volta sola.</p>
      <div class="piedef"><small>Esc per annullare</small><button class="tasto" data-sca="no">Non ora</button><button class="btn-nero" data-sca="chiedi">Continua</button></div>`);
    else sheet(`<h3>Manca il permesso Registrazione schermo</h3>
      <p class="conferma-testo">Apri Impostazioni di Sistema, Privacy e sicurezza, Registrazione schermo: attiva Juggl, poi riapri Juggl.</p>
      <div class="piedef"><small>Esc per chiudere</small><button class="tasto" data-sca="no">Chiudi</button><button class="btn-nero" data-sca="apri">Apri Impostazioni di Sistema</button></div>`);
  });

  // ---------- Settings: "Scatto in Juggl" section ----------
  let recording = false;
  function sectionEl() {
    const f = q("#foglio"); if (!f || !f.classList.contains("imp") || !ST?.mac) return;
    let sec = f.querySelector("#imp-scatto");
    const h = `<h4>Scatto in Juggl</h4>
      <div class="imp-riga"><div class="tx"><b>${recording ? "Premi la combinazione…" : ST.tasto ? E(pretty(ST.tasto)) : "Spento"}</b><small>${recording ? "Esc per annullare" : `Come ⌘⇧4, ma l'immagine va nella chat aperta${ST.alt ? `. Con ${E(pretty(ST.alt))} va a Chiedi a Juggl` : ""}`}</small></div>
        ${recording ? "" : `<button class="tasto" data-sca="registra">Cambia…</button>${ST.tasto ? `<button class="tasto" data-sca="spegni">Spegni</button>` : `<button class="tasto" data-sca="base">Usa ${E(ST.base)}</button>`}`}</div>
      <div class="imp-riga"><div class="tx"><b>Registrazione schermo: ${ST.permesso ? "permesso dato" : "permesso da dare"}</b><small>${ST.permesso ? "Juggl lo usa solo quando premi la scorciatoia" : "Te lo chiedo al primo scatto, con una spiegazione"}</small></div>
        ${!ST.permesso && ST.chiesto ? `<button class="tasto" data-sca="apri">Impostazioni di Sistema</button>` : ""}</div>`;
    if (!sec) {
      const foot = f.querySelector(".piedef"); if (!foot) return;
      sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-scatto";
      // after "Aggiornamenti" if present, otherwise before the footer
      const updates = [...f.querySelectorAll(":scope > .imp-sez")].find((x) => /aggiornamenti/i.test(x.querySelector("h4")?.dataset.icT || x.querySelector("h4")?.textContent || ""));
      if (updates) updates.before(sec); else foot.before(sec);
    }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  const fg = q("#foglio");
  if (fg) new MutationObserver(() => { const f = q("#foglio"); if (f?.classList.contains("imp") && !q("#imp-scatto")) { if (ST) sectionEl(); else load().then(sectionEl); } }).observe(fg, { childList: true, attributes: true, attributeFilter: ["class"] });
  TT.event.listen("impostazioni", () => { load().then(sectionEl); });

  async function apply(t) {
    const r = await inv("scatto_imposta", { tasto: t }).catch((e) => ({ ok: false, msg: String(e) }));
    await load(); recording = false; q("#imp-scatto") && (q("#imp-scatto")._h = ""); sectionEl();
    notice(r.ok ? (r.tasto ? `Scatto con ${pretty(r.tasto)}` : "Scatto spento") : r.msg || "Non riuscito", 2600);
    return r;
  }
  // key combo pressed in the Juggl window -> "Shift+Cmd+2" (snap.rs normalizes the form)
  window.addEventListener("keydown", (e) => {
    if (!recording) return;
    e.preventDefault(); e.stopImmediatePropagation();
    if (e.key === "Escape") { recording = false; q("#imp-scatto") && (q("#imp-scatto")._h = ""); return sectionEl(); }
    const k = /^Key([A-Z])$/.exec(e.code)?.[1] || /^Digit(\d)$/.exec(e.code)?.[1] || (/^F\d{1,2}$/.test(e.code) ? e.code : "");
    if (!k) return;   // only modifiers so far: waiting for the key
    apply([e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Cmd", k].filter(Boolean).join("+"));
  }, true);

  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-sca]"); if (!b) return;
    const a = b.dataset.sca;
    if (a === "no") return close();
    if (a === "apri") { await inv("scatto_apri_impostazioni"); return; }
    if (a === "chiedi") {
      close();
      const r = await inv("scatto_permesso_chiedi").catch(() => ({ ok: false }));
      if (r.ok) return inv("scatto_ora", { alt: pendingAlt });   // snap requested by the user with the shortcut: do it now
      return notice("Quando avrai dato il permesso in Impostazioni di Sistema, riapri Juggl", 4200);
    }
    if (a === "registra") { recording = true; q("#imp-scatto")._h = ""; return sectionEl(); }
    if (a === "spegni") return apply("");
    if (a === "base") return apply(ST?.base || "F16");
  });

  // ---------- Settings by voice (ui/settings-chat.js reads window.impostazioniVociExtra) ----------
  const norm = (s) => String(s || "").toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
  (window.impostazioniVociExtra = window.impostazioniVociExtra || []).push({
    id: "scatto", nome: "Scatto in Juggl (scorciatoia che fotografa lo schermo e lo allega alla chat)", valori: ["F16", "Cmd+Shift+2", "spento"],
    prendi(f, raw) {
      if (!ST?.mac || !/(scatto|screenshot|cattura|foto dello schermo|fotografa)/.test(f)) return null;
      if (/\b(spegni|disattiva|togli|niente)\b/.test(f)) return { v: "spento" };
      const fk = f.match(/\bf(1[3-9]|[1-9]|2[0-4])\b/); if (fk) return { v: "F" + fk[1] };
      const g = norm(raw).replace(/[⌘]/g, "cmd+").replace(/[⇧]/g, "shift+").replace(/[⌥]/g, "alt+");
      const c = g.match(/((?:(?:cmd|command|comando|shift|maiusc|alt|option|ctrl|control)\s*\+?\s*)+)([a-z0-9])\b/);
      if (c) return { v: c[1].replace(/\s+/g, "").split("+").filter(Boolean).concat(c[2]).join("+") };
      return { v: ST?.base || "F16" };
    },
    async fai(v) {
      const r = await inv("scatto_imposta", { tasto: v === "spento" ? "" : v });
      if (!r?.ok) throw new Error(r?.msg || "non riuscito");
      await load(); sectionEl();
      return { titolo: r.tasto ? `Scatto in Juggl con ${pretty(r.tasto)}` : "Scatto in Juggl spento",
        dett: r.tasto ? `Da qualunque app: l'immagine va nella chat aperta${r.alt ? `; con ${pretty(r.alt)} a Chiedi a Juggl` : ""}.` : "", da: r.prima, a: r.tasto,
        annulla: () => inv("scatto_imposta", { tasto: r.prima || "" }).then(load) };
    },
  });
  load();
})();
