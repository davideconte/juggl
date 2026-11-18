// Help and reports (Nov 17). Fixed row at the top of the sidebar; opens a full-page view with a single chat:
// - the assistant (small model with only the Juggl guide, src-tauri/src/support.rs) answers usage questions;
//   it does not see other chats or files: it is a separate chat, outside the index, Requests and per-project usage;
// - if there is a problem or a request it offers a report: "Prepara la segnalazione" opens the PREVIEW of everything
//   that is sent (editable text, version, system, cleaned diagnostics, screenshots) and "Invia" is needed for every send;
// - report states (received, taken in charge, fixed in 0.1.xx, not reproducible) in the header and in the chat;
// - screenshots: Snap (F16) with the view open, paste, drag, paperclip button. Only PNG or JPEG, at most 2.
// Hooks: <script src="help.js"> in index.html, one line in snap.js (window.aiutoPrendiScatto).
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const q = (s, r = document) => r.querySelector(s);
  const src = (p) => TT.core.convertFileSrc(p);
  const textMd = (t) => (typeof md === "function" ? md(t) : `<p>${E(t)}</p>`);
  const MAXA = 2;
  const ST = { aperta: false, d: null, allegati: [], pensa: false, ant: {}, letto: +(localStorage.getItem("aiuto-letto") || 0) };
  const STATE_NAMES = { ricevuta: "Ricevuta", presa: "Presa in carico", risolta: "Risolta", non_riproducibile: "Non riproducibile" };
  const KIND_NAMES = { bug: "Problema", richiesta: "Richiesta", domanda: "Domanda" };
  const PRIORITY_NAMES = { blocca: "Blocca l'uso", fastidio: "Fastidio", idea: "Idea" };
  const ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><circle cx="8" cy="8" r="6.3"/><path d="M6.2 6.2a1.9 1.9 0 1 1 2.6 1.75c-.5.22-.8.6-.8 1.15v.4"/><circle cx="8" cy="11.4" r=".2" fill="currentColor"/></svg>';
  const PAPERCLIP = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M13.2 7.6 8 12.8a3.2 3.2 0 0 1-4.6-4.6l5.6-5.6a2.1 2.1 0 0 1 3 3L6.4 11.2a1 1 0 0 1-1.5-1.5L9.8 4.8"/></svg>';
  const CLOSE_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="m4 4 8 8M12 4l-8 8"/></svg>';

  const css = `
.aiuto-riga{display:flex;align-items:center;gap:8px;margin:2px 10px 6px;padding:6px 9px;border-radius:8px;font-size:13px;color:var(--ink2);text-align:left;width:calc(100% - 20px)}
.aiuto-riga:hover{background:var(--fill)}
.aiuto-riga.sel{background:var(--sel);color:var(--selink)}
.aiuto-riga svg{width:15px;height:15px;flex:none}
.aiuto-riga span{flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.aiuto-riga i{width:7px;height:7px;border-radius:50%;background:var(--ink);flex:none}
.vista-aiuto .scroll{display:flex;flex-direction:column}
.au-pagina{width:min(var(--col),100%);margin:0 auto;padding:18px 24px 12px;flex:1}
.au-segn{display:flex;flex-direction:column;gap:6px;margin:0 0 18px}
.au-segn h4{margin:0 0 2px;font-size:12px;color:var(--ink3);font-weight:500}
.au-s{display:flex;align-items:baseline;gap:8px;font-size:13px;color:var(--ink2)}
.au-s b{font-weight:500;color:var(--ink);flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.au-chip{flex:none;font-size:11.5px;padding:2px 8px;border-radius:6px;background:var(--fill);color:var(--ink2)}
.au-chip.risolta{background:var(--sel);color:var(--selink)}
.au-m{margin:0 0 14px;font-size:var(--t);line-height:1.55}
.au-m.tu{display:flex;flex-direction:column;align-items:flex-end}
.au-m.tu .au-b{background:var(--bolla);color:var(--bollaink);padding:8px 13px;border-radius:14px;max-width:80%;white-space:pre-wrap;word-wrap:break-word}
.au-m.juggl .au-b{color:var(--ink)}
.au-m.juggl .au-b p{margin:0 0 8px}
.au-m.stato .au-b{font-size:13.5px;color:var(--ink2);border-left:2px solid var(--ink);padding-left:10px}
.au-imgs{display:flex;gap:6px;margin-top:6px}
.au-imgs img{height:64px;border-radius:6px;border:1px solid var(--line)}
.au-proponi{margin-top:4px;display:flex;gap:8px;align-items:center;flex-wrap:wrap}
.au-card{border:1px solid var(--line);border-radius:12px;padding:14px 16px;margin-top:8px;display:flex;flex-direction:column;gap:10px;font-size:13.5px}
.au-card h5{margin:0;font-size:13.5px;font-weight:600}
.au-card .riga2{display:flex;gap:8px;flex-wrap:wrap}
.au-card select,.au-card input,.au-card textarea{font:inherit;color:var(--ink);background:var(--bg);border:1px solid var(--line);border-radius:7px;padding:6px 9px}
.au-card input[data-au-campo],.au-card textarea{width:100%}
.au-card input[type=checkbox]{width:auto;margin:0;flex:none}
.au-card textarea{min-height:84px;resize:vertical}
.au-card pre{margin:0;font-family:var(--mono);font-size:11.5px;line-height:1.5;background:var(--code);border-radius:8px;padding:8px 10px;white-space:pre-wrap;word-break:break-word;max-height:220px;overflow:auto;color:var(--ink2)}
.au-card .meta{color:var(--ink3);font-size:12.5px}
.au-card label.sp{display:flex;gap:7px;align-items:center;color:var(--ink2)}
.au-card .info{color:var(--ink3);font-size:12px;line-height:1.45}
.au-card .piede{display:flex;justify-content:flex-end;gap:8px;align-items:center}
.au-card .piede small{flex:1;color:var(--err)}
.au-card .imgs{display:flex;gap:8px;flex-wrap:wrap}
.au-card .imgs figure{margin:0;position:relative}
.au-card .imgs img{height:80px;border-radius:6px;border:1px solid var(--line);display:block}
.au-card .imgs figcaption{font-size:11px;color:var(--ink3);margin-top:3px}
.au-card .imgs figcaption a{color:var(--ink);text-decoration:underline;cursor:pointer}
.au-pensa{color:var(--ink3);font-size:13px;margin:0 0 14px}
.au-fondo{position:sticky;bottom:0;background:var(--bg);padding:8px 24px 16px;width:min(var(--col),100%);margin:0 auto}
.au-pend{display:flex;gap:6px;margin:0 0 6px;flex-wrap:wrap}
.au-pend span{position:relative}
.au-pend img{height:48px;border-radius:6px;border:1px solid var(--line);display:block}
.au-pend button{position:absolute;top:-6px;right:-6px;width:18px;height:18px;border-radius:50%;background:var(--sel);color:var(--selink);font-size:11px;line-height:18px;padding:0}
.au-box{display:flex;gap:8px;align-items:flex-end;border:1px solid var(--line);border-radius:14px;padding:8px 8px 8px 12px}
.au-box textarea{flex:1;border:0;outline:0;resize:none;font:inherit;font-size:var(--t);line-height:1.45;background:transparent;color:var(--ink);max-height:180px;min-height:22px}
.au-box .tasto svg{width:16px;height:16px}
.au-nota{color:var(--ink3);font-size:11.5px;margin:6px 2px 0}
`;
  const styleEl = document.createElement("style"); styleEl.textContent = css; document.head.appendChild(styleEl);

  // ---------- fixed row in the sidebar ----------
  function rowEl() {
    let r = q("#aiuto-riga");
    if (!r) {
      const top = q(".side .side-top"); if (!top) return null;
      r = document.createElement("button");
      r.id = "aiuto-riga"; r.className = "aiuto-riga"; r.title = "Aiuto e segnalazioni: domande su Juggl, problemi, richieste";
      r.innerHTML = `${ICON}<span>Aiuto e segnalazioni</span><i hidden></i>`;
      top.after(r);
      r.addEventListener("click", () => (ST.aperta && isOpen() ? close() : openView()));
    }
    return r;
  }
  const isOpen = () => typeof vistaMac !== "undefined" && vistaMac && !!q("#au-pagina");
  function dotEl() {
    const r = rowEl(); if (!r) return;
    const fresh = (ST.d?.messaggi || []).some((m) => m.stato && m.t * 1000 > ST.letto);
    q("i", r).hidden = !fresh || isOpen();
    r.classList.toggle("sel", isOpen());
  }

  // ---------- view ----------
  async function openView() {
    try { if (typeof chiudiQuota === "function") chiudiQuota(); } catch {}
    vistaMac = true; vistaRq = false; saAperto = null; montata = null;
    if (typeof vistaCons !== "undefined") vistaCons = false;
    if (typeof vistaIc !== "undefined") vistaIc = false;
    document.querySelectorAll(".riga.sel").forEach((x) => x.classList.remove("sel"));
    ST.aperta = true;
    q("#main").innerHTML = `<div class="vista-mac vista-aiuto"><div class="barra" data-tauri-drag-region><span class="sa-ic">${ICON}</span>
      <div class="ic-testa" data-tauri-drag-region><h2>Aiuto e segnalazioni</h2><div class="ic-conta" id="au-conta"></div></div>
      <button class="tasto" id="au-svuota" title="Svuota questa chat (le segnalazioni restano)">Svuota</button>
      <button class="tasto" id="chiudi-mac" title="Chiudi (Esc)">${CLOSE_ICON}</button></div>
      <div class="scroll" id="au-scroll"><div class="au-pagina" id="au-pagina"><div class="ic-vuoto">Apro la chat di aiuto…</div></div>
      <div class="au-fondo"><div class="au-pend" id="au-pend"></div>
        <div class="au-box"><textarea id="au-testo" rows="1" placeholder="Chiedi come si fa qualcosa, o racconta cosa non va…" spellcheck="true"></textarea>
        <input type="file" id="au-file" accept="image/png,image/jpeg" multiple hidden>
        <button class="tasto" id="au-allega" title="Allega una schermata (anche con lo Scatto)">${PAPERCLIP}</button>
        <button class="btn-nero" id="au-manda">Manda</button></div>
        <div class="au-nota">Questa chat non vede le tue altre chat né i tuoi file. Una segnalazione parte solo dopo l'anteprima e il tuo ok.</div></div></div></div>`;
    dotEl();
    await load();
    q("#au-testo")?.focus();
    // states from the server (in the background): changes arrive already written in the chat
    inv("aiuto_stati").then((r) => { if (r?.cambi?.length) load(); }).catch(() => {});
  }
  function close() { if (typeof chiudiMac === "function") chiudiMac(); ST.aperta = false; dotEl(); }
  async function load() {
    try { ST.d = await inv("aiuto_storia"); } catch (e) { console.error(e); ST.d = ST.d || { messaggi: [], segnalazioni: [] }; }
    ST.letto = Date.now(); localStorage.setItem("aiuto-letto", String(ST.letto));
    render(); dotEl();
  }

  function stateChip(s) {
    if (s.stato === "risolta") return `<span class="au-chip risolta">${s.versione_risolta ? `Risolta nella ${E(s.versione_risolta)}` : "Risolta"}</span>`;
    return `<span class="au-chip">${E(STATE_NAMES[s.stato] || s.stato)}</span>`;
  }
  function render() {
    const pageEl = q("#au-pagina"); if (!pageEl) return;
    const D = ST.d || {}, segn = (D.segnalazioni || []).slice().sort((a, b) => (b.creata || 0) - (a.creata || 0));
    const openIds = segn.filter((s) => s.stato === "ricevuta" || s.stato === "presa").length;
    q("#au-conta").innerHTML = segn.length ? `<span class="ic-chip"><b>${openIds}</b>${openIds === 1 ? "aperta" : "aperte"}</span><span class="ic-chip"><b>${segn.length}</b>mandate</span>` : "";
    let h = "";
    if (segn.length) h += `<div class="au-segn"><h4>Le tue segnalazioni</h4>${segn.slice(0, 6).map((s) => `<div class="au-s"><b title="${E(s.titolo)}">${E(s.titolo)}</b>${s.dest === "regia" ? '<span class="au-chip">Alla regia</span>' : stateChip(s)}</div>`).join("")}</div>`;
    const welcome = D.admin
      ? "Chiedimi come si usa Juggl, oppure racconta cosa non va. Le tue segnalazioni vanno dritte alla regia di Juggl, non al server."
      : "Chiedimi come si usa Juggl, oppure racconta cosa non va: preparo la segnalazione e vedi tutto ciò che parte prima di mandarla.";
    h += `<div class="au-m juggl"><div class="au-b">${textMd(welcome)}${D.assistente === false ? textMd("_L'assistente è spento: puoi comunque mandare segnalazioni._") : ""}</div></div>`;
    const mm = D.messaggi || [];
    mm.forEach((m, i) => { h += msgHtml(m, i, mm); });
    if (ST.pensa) h += `<div class="au-pensa">Sto pensando…</div>`;
    pageEl.innerHTML = h;
    pending();
    const sc = q("#au-scroll"); if (sc) sc.scrollTop = sc.scrollHeight;
  }
  // attachments of the user message that message i answers
  function attachmentsBefore(i, mm) { for (let j = i - 1; j >= 0; j--) if (mm[j].chi === "tu") return mm[j].allegati || []; return []; }
  function msgHtml(m, i, mm) {
    if (m.chi === "tu") {
      const imgs = (m.allegati || []).map((p) => `<img src="${E(src(p))}" alt="schermata">`).join("");
      return `<div class="au-m tu">${m.testo ? `<div class="au-b">${E(m.testo)}</div>` : ""}${imgs ? `<div class="au-imgs">${imgs}</div>` : ""}</div>`;
    }
    let h = `<div class="au-m juggl${m.stato ? " stato" : ""}" data-i="${i}"><div class="au-b">${textMd(m.testo || "")}</div>`;
    if (m.bozza && !mm.slice(i + 1).some((x) => x.inviata)) {
      const a = ST.ant[i];
      h += a ? card(i, a) : `<div class="au-proponi"><button class="btn-nero" data-au-prepara="${i}">Prepara la segnalazione</button><small class="meta">Vedi tutto prima che parta</small></div>`;
    }
    return h + "</div>";
  }
  function card(i, a) {
    if (a.carica) return `<div class="au-card"><span class="meta">Preparo l'anteprima…</span></div>`;
    const A = a.anteprima || {}, d = A.diagnostica || {}, err = d.errori || [];
    const opts = (names, sel) => Object.entries(names).map(([k, v]) => `<option value="${k}"${k === sel ? " selected" : ""}>${E(v)}</option>`).join("");
    const diag = a.diag
      ? `<pre>${E([`Memoria di Juggl: ${d.memoria_mb ?? "?"} MB · RAM del computer: ${d.ram_gb ?? "?"} GB · avvio: ${d.avvio_ms ? d.avvio_ms + " ms" : "non misurato"}`, err.length ? "Ultimi errori (ripuliti):" : "Nessun errore recente nel registro di Juggl.", ...err.map((x) => "  " + x)].join("\n"))}</pre>`
      : "";
    const imgs = (A.allegati || []).map((x, k) => `<figure><img src="${E(src(x.percorso))}" alt=""><figcaption>${E(x.nome)} · ${x.kb} KB · <a href="#" data-au-togli="${i}:${k}">togli</a></figcaption></figure>`).join("");
    return `<div class="au-card" data-au-card="${i}">
      <h5>${a.dest === "regia" ? "Va alla regia di Juggl" : "Parte verso juggl.dev"}: controlla tutto</h5>
      <div class="riga2"><select data-au-campo="tipo">${opts(KIND_NAMES, A.tipo)}</select><select data-au-campo="priorita">${opts(PRIORITY_NAMES, A.priorita)}</select></div>
      <input data-au-campo="titolo" maxlength="120" value="${E(A.titolo)}" placeholder="Titolo">
      <textarea data-au-campo="testo" maxlength="4000" placeholder="Cosa succede, e come rifarlo">${E(A.testo)}</textarea>
      <div class="meta">Versione ${E(A.versione)} · ${E(A.sistema)}</div>
      <label class="sp"><input type="checkbox" data-au-diag="${i}"${a.diag ? " checked" : ""}> Includi la diagnostica tecnica (ripulita: niente percorsi, nomi, indirizzi, chiavi o numeri)</label>
      ${diag}
      ${imgs ? `<div class="imgs">${imgs}</div>` : '<div class="meta">Nessuna schermata</div>'}
      <div class="info">${E(a.informativa || "")}</div>
      <div class="piede"><small data-au-err="${i}"></small><button class="tasto" data-au-annulla="${i}">Annulla</button><button class="btn-nero" data-au-invia="${i}"${a.invio ? " disabled" : ""}>${a.invio ? "Invio…" : a.dest === "regia" ? "Manda alla regia" : "Invia a Juggl"}</button></div>
    </div>`;
  }

  async function preview(i, allegati, diag) {
    const m = ST.d?.messaggi?.[i]; if (!m?.bozza) return;
    const before = ST.ant[i];
    const bozza = before?.anteprima ? fields(i) || m.bozza : m.bozza;
    ST.ant[i] = { carica: true, diag, allegati }; renderCard(i);
    const r = await inv("aiuto_anteprima", { bozza, allegati, conDiagnostica: diag }).catch((e) => ({ ok: false, msg: String(e) }));
    if (!r?.ok) { delete ST.ant[i]; render(); return notice(r?.msg || "Anteprima non riuscita", 3500); }
    ST.ant[i] = { anteprima: r.anteprima, dest: r.destinazione, informativa: r.informativa, diag, allegati };
    renderCard(i);
  }
  function renderCard(i) {
    const el = q(`.au-m[data-i="${i}"]`); if (!el) return render();
    const v = el.querySelector(".au-card, .au-proponi"); const h = card(i, ST.ant[i]);
    if (v) v.outerHTML = h; else el.insertAdjacentHTML("beforeend", h);
  }
  function fields(i) {
    const c = q(`[data-au-card="${i}"]`); if (!c) return null;
    const o = {}; c.querySelectorAll("[data-au-campo]").forEach((x) => (o[x.dataset.auCampo] = x.value)); return o;
  }
  async function sendReport(i) {
    const a = ST.ant[i]; if (!a?.anteprima || a.invio) return;
    const b = fields(i);
    if (!b.titolo.trim() || !b.testo.trim()) { const e = q(`[data-au-err="${i}"]`); if (e) e.textContent = "Servono titolo e descrizione"; return; }
    a.invio = true; Object.assign(a.anteprima, b); renderCard(i);
    const r = await inv("aiuto_invia", { idLocale: a.anteprima.id_locale, bozza: b }).catch((e) => ({ ok: false, msg: String(e) }));
    a.invio = false;
    if (!r?.ok) { renderCard(i); const e = q(`[data-au-err="${i}"]`); if (e) e.textContent = r?.msg || "Non partita"; return; }
    delete ST.ant[i];
    notice(a.dest === "regia" ? "Mandata alla regia" : "Segnalazione mandata", 2000);
    load();
  }

  // ---------- writing ----------
  function pending() {
    const p = q("#au-pend"); if (!p) return;
    p.innerHTML = ST.allegati.map((x, k) => `<span><img src="${E(src(x))}" alt=""><button data-au-tpend="${k}" title="Togli">×</button></span>`).join("");
  }
  function add(paths) {
    const valid = (paths || []).filter((x) => /\.(png|jpe?g)$/i.test(x));
    if (!valid.length) return notice("Solo immagini PNG o JPEG", 2500);
    for (const p of valid) { if (ST.allegati.length >= MAXA) { notice(`Al massimo ${MAXA} schermate`, 2500); break; } if (!ST.allegati.includes(p)) ST.allegati.push(p); }
    pending(); q("#au-testo")?.focus();
  }
  async function fromFile(files) {
    for (const f of files) {
      if (!/^image\/(png|jpeg)$/.test(f.type)) { notice("Solo immagini PNG o JPEG", 2500); continue; }
      if (f.size > 2 * 1024 * 1024) { notice("Schermata oltre 2 MB", 2500); continue; }
      const b64 = await new Promise((ok) => { const r = new FileReader(); r.onload = () => ok(r.result); r.readAsDataURL(f); });
      const r = await inv("aiuto_salva_immagine", { b64 }).catch(() => null);
      if (r?.ok) add([r.percorso]); else notice(r?.msg || "Immagine non salvata", 2500);
    }
  }
  async function submit() {
    const t = q("#au-testo"); if (!t || ST.pensa) return;
    const testo = t.value.trim(); if (!testo && !ST.allegati.length) return;
    const allegati = ST.allegati.slice();
    ST.d = ST.d || { messaggi: [] };
    ST.d.messaggi.push({ chi: "tu", testo, allegati, t: Date.now() / 1000 });
    t.value = ""; t.style.height = ""; ST.allegati = []; ST.pensa = true; render();
    const r = await inv("aiuto_chiedi", { testo, allegati }).catch((e) => ({ ok: false, msg: String(e) }));
    ST.pensa = false;
    if (!r?.ok) notice(r?.msg || "Non riuscito", 3000);
    await load();
  }

  // ---------- events ----------
  document.addEventListener("click", (e) => {
    if (!isOpen()) return;
    const t = e.target;
    let x;
    if ((x = t.closest("[data-au-prepara]"))) { const i = +x.dataset.auPrepara; return preview(i, attachmentsBefore(i, ST.d.messaggi), true); }
    if ((x = t.closest("[data-au-invia]"))) return sendReport(+x.dataset.auInvia);
    if ((x = t.closest("[data-au-annulla]"))) { delete ST.ant[+x.dataset.auAnnulla]; return render(); }
    if ((x = t.closest("[data-au-togli]"))) { e.preventDefault(); const [i, k] = x.dataset.auTogli.split(":").map(Number); const a = ST.ant[i]; const al = (a.allegati || []).filter((_, j) => j !== k); return preview(i, al, a.diag); }
    if ((x = t.closest("[data-au-tpend]"))) { ST.allegati.splice(+x.dataset.auTpend, 1); return pending(); }
    if (t.closest("#au-manda")) return submit();
    if (t.closest("#au-allega")) return q("#au-file")?.click();
    if (t.closest("#au-svuota")) { if (!confirm("Svuoto questa chat? Le segnalazioni mandate restano.")) return; return inv("aiuto_svuota").then(() => { ST.ant = {}; load(); }); }
  });
  document.addEventListener("change", (e) => {
    if (!isOpen()) return;
    const t = e.target;
    if (t.id === "au-file") { fromFile([...t.files]); t.value = ""; return; }
    if (t.dataset?.auDiag != null) { const i = +t.dataset.auDiag; const a = ST.ant[i]; if (a) preview(i, a.allegati, t.checked); }
  });
  document.addEventListener("keydown", (e) => {
    if (e.target?.id !== "au-testo") return;
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit(); }
  });
  document.addEventListener("input", (e) => {
    if (e.target?.id !== "au-testo") return;
    const t = e.target; t.style.height = "auto"; t.style.height = Math.min(t.scrollHeight, 180) + "px";
  });
  document.addEventListener("paste", (e) => {
    if (e.target?.id !== "au-testo") return;
    const files = [...(e.clipboardData?.items || [])].filter((x) => x.kind === "file").map((x) => x.getAsFile()).filter(Boolean);
    if (files.length) { e.preventDefault(); fromFile(files); }
  });
  try {
    TT.webview.getCurrentWebview().onDragDropEvent((ev) => { if (isOpen() && ev.payload?.type === "drop") add(ev.payload.paths || []); });
  } catch {}
  // Snap (F16) with the view open: the screenshot goes here (snap.js asks us first)
  window.aiutoPrendiScatto = (path) => { if (!isOpen()) return false; add([path]); return true; };
  window.apriAiuto = openView;
  // the view closes by itself when something else opens: deselect the row
  new MutationObserver(() => { if (ST.aperta && !isOpen()) { ST.aperta = false; dotEl(); } }).observe(document.getElementById("main") || document.body, { childList: true });

  // row in the sidebar at startup and dot for new states (the Mac checks states at startup and every 6 hours)
  rowEl();
  inv("aiuto_storia").then((d) => { ST.d = d; dotEl(); }).catch(() => {});
  setInterval(() => { if (!isOpen()) inv("aiuto_storia").then((d) => { ST.d = d; dotEl(); }).catch(() => {}); }, 10 * 60 * 1000);
})();
