// "In corso" view (⌘0), variant A "Plancia" (Nov 17). Data from src-tauri/src/in_progress.rs via caricaElenco.
// At the top a sentence instead of counters. Then, per chat:
// - WAITING FOR YOU: orange border (the brand color means "waiting for you"), the full question, the right buttons
//   (Ok/No for approvals, Reply with the box that opens in the card, Open chat);
// - WORKING NOW: what it does with the "thinking" dots, background jobs and helpers with the spinner, steps done,
//   how long it has been working, Stop on hover;
// - TO READ: one row per answer, with Read;
// - STOPPED: one box per project, dimmed logo, how many chats and since when (same icon as the sidebar: EL.icona).
// Rules kept from the stable version (Nov 12): FIXED ORDER (a card never moves by itself; changes are collected and
// applied at most once every 5 s, with sliding cards; never with the pointer over it or while you type: the header shows
// "N da spostare", click = right away), only changed parts are redrawn, "Reduce motion" respected.
// Hooks in index.html: the <script src="in-progress.js"> tag and window.inCorsoDati in caricaElenco. The header style
// (ic-testa) stays in index.html because the Usage view uses it too.
(() => {
  if (!window.__TAURI__) return;
  const CHANGES_MS = 5000, COLLECT_MS = 700, SLIDE_MS = 420, MAXF = 12;
  const SEZ = ["asp", "lav", "let", "fer"];
  const sectionOf = (v) => (v === "aspetta" || v === "bloccata" ? "asp" : v === "lavora" ? "lav" : v === "fatta" ? "let" : v === "ferma" || v === "ibernata" ? "fer" : "");
  const stoppedOnes = () => RIDOTTO || document.hidden;
  const page = () => document.getElementById("ics-pagina");
  const isOpen = () => vistaMac && !!page();
  const ST = { lay: { asp: [], lav: [], let: [], fer: [] }, forma: new Map(), ultimi: new Map(), el: new Map(), applicato: 0, timer: 0, sopra: false, ferme: false, scrive: null };

  // ---------- data ----------
  const within = (s) => !!s && !/^(wa|chiusa|locale):/.test(s.sessione || "") && !s.locale && !/\/tavolo\/prova\/?$/.test(s.cwd || "") && !!sectionOf(s.vista) && !(s.vista === "ibernata" && s.pronta);
  const nameOf = (s) => (s.sessione === dati?.juggl ? "Juggl" : s.nome || "Chat");
  const projectOf = (s) => (s.progetto?.id && s.progetto.id !== "locale" ? s.progetto : null);
  const sd = (sid) => (dati?.sessioni || []).find((x) => x.sessione === sid) || ST.ultimi.get(sid);
  const lines = (t) => piano(t).split("\n").map((x) => x.trim()).filter(Boolean);
  const lastLine = (t) => lines(t).slice(-1)[0] || "";
  const okOf = (sid) => (datiOk?.voci || []).find((x) => x.sessione === sid && okAperto(x));
  const stoppable = (s) => !!s && typeof turnoInCorso === "function" && turnoInCorso(s) && !!(s.ospitata || s.ospite || s.surface);
  const iconOf = (s, st, sideHtml) => (window.EL?.icona ? EL.icona(s, st, sideHtml) : avatar(s));

  /** The entries as they would be now: key -> { k, sez, sids, firma, dal }. Chats one by one; stopped ones merged per project. */
  function liveSecs() {
    const out = new Map();
    for (const s of dati?.sessioni || []) {
      if (!within(s)) continue;
      ST.ultimi.set(s.sessione, s);
      const z = sectionOf(s.vista), p = projectOf(s);
      const k = z === "fer" && p ? "p:" + p.id : z + ":" + s.sessione;
      const x = out.get(k) || { k, sez: z, sids: [], dal: 0 };
      x.sids.push(s.sessione); x.dal = Math.max(x.dal, s.da || 0);
      out.set(k, x);
    }
    if (ST.ultimi.size > 400) for (const id of [...ST.ultimi.keys()].slice(0, 200)) ST.ultimi.delete(id);
    for (const x of out.values()) x.firma = x.sez + "|" + x.sids.length;
    return out;
  }
  function pendingChanges(v = liveSecs()) {
    let n = 0;
    for (const [k, x] of v) if (ST.forma.get(k)?.firma !== x.firma) n++;
    for (const k of ST.forma.keys()) if (!v.has(k)) n++;
    return n;
  }

  // ---------- pieces ----------
  const STOP_ICON = '<svg viewBox="0 0 16 16" fill="currentColor"><rect x="4" y="4" width="8" height="8" rx="1.5"/></svg>';
  const ARROW = '<svg viewBox="0 0 12 12" fill="none" stroke-width="1.8" stroke-linecap="round"><path d="M6 10V2M2.5 5.5 6 2l3.5 3.5"/></svg>';
  const ICON = '<svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1.5" y="2" width="5.5" height="5.5" rx="1.5"/><rect x="9" y="2" width="5.5" height="5.5" rx="1.5"/><rect x="1.5" y="9.5" width="5.5" height="5" rx="1.5"/><rect x="9" y="9.5" width="5.5" height="5" rx="1.5"/></svg>';
  const THINKING = '<span class="el-pensa"><i></i><i></i><i></i></span>';
  const html = (el, h) => { if (el && el._h !== h) { el.innerHTML = h; el._h = h; } };
  const attr = (el, k, v) => { if (!el) return; v = String(v); if (el.getAttribute(k) !== v) el.setAttribute(k, v); };
  const show = (el, si) => { if (el && el.hidden === !!si) el.hidden = !si; };
  /** Text that changes with a short fade (never with Reduce motion). */
  function put(box, val) {
    if (!box) return;
    val = String(val ?? "");
    if (box._v === val) return;
    const firstEl = box._v === undefined; box._v = val;
    if (firstEl || stoppedOnes() || !box.isConnected) { box.textContent = val; return; }
    box.classList.remove("ics-cambia"); void box.offsetWidth; box.textContent = val; box.classList.add("ics-cambia");
  }
  const write = (sid) => `<form class="ics-scrivi" data-sid="${esc(sid)}" autocomplete="off" hidden><input data-ics-scrivi placeholder="Rispondi…" spellcheck="false"><button type="submit" title="Manda (Invio)">${ARROW}</button></form>`;

  function fresh(k, sez, sid) {
    const el = document.createElement(sez === "let" || sez === "fer" ? "button" : "article");
    el.dataset.ick = k; el.dataset.k = k;
    if (sez === "asp") {
      el.className = "ics-asp";
      el.innerHTML = `<div class="ics-testa"><span class="ics-ic"></span><b class="ics-nm"></b><span class="ics-pr"></span><span class="ics-q"></span></div>
        <div class="ics-cosa"></div><div class="ics-dett"></div><div class="ics-tasti"></div>${write(sid)}`;
    } else if (sez === "lav") {
      el.className = "ics-lav";
      el.innerHTML = `<div class="ics-testa"><span class="ics-ic"></span><b class="ics-nm"></b><button type="button" class="ics-ferma" data-ics-ferma title="Ferma il turno in corso (come Esc)">${STOP_ICON}<span>Ferma</span></button><span class="ics-q"><span class="ad-tempo"></span></span></div>
        <div class="ics-ora">${THINKING}<span class="ics-ora-t"></span></div><div class="ics-bg"></div>
        <div class="ics-piede"><span class="ics-passi" hidden><span class="ics-barra"><i></i></span><b></b></span><span class="ics-pr"></span></div>`;
    } else if (sez === "let") {
      el.type = "button"; el.className = "ics-let";
      el.innerHTML = `<span class="ics-ic"></span><span class="ics-t"><b class="ics-nm"></b><span class="ics-ris"></span></span><span class="ics-q"></span><span class="ics-letto" data-ics-letto role="button" tabindex="0">Letto</span>`;
    } else {
      el.type = "button"; el.className = "ics-fer";
      el.innerHTML = `<span class="ics-ic"></span><span class="ics-t"><b class="ics-nm"></b><small class="ics-sub"></small></span>`;
    }
    return el;
  }

  // ---------- fill (only texts and states: the shape stays) ----------
  function fill1(el, x) {
    const ss = x.sids.map(sd).filter(Boolean); if (!ss.length) return;
    const s = ss[0], z = x.sez, p = projectOf(s);
    attr(el, "data-v", s.vista);
    if (z === "fer") {
      const more = [...ss].sort((a, b) => (b.da || 0) - (a.da || 0))[0];
      el.dataset.sid = more.sessione;
      html(el.querySelector(".ics-ic"), iconOf(p ? { nome: p.nome, progetto: p } : s, "fer", 24));
      put(el.querySelector(".ics-nm"), p ? p.nome : nameOf(s));
      put(el.querySelector(".ics-sub"), `${ss.length === 1 ? (p ? nameOf(s) : "chat") : ss.length + " chat"} · ferma da ${quanto(more.da)}`);
      attr(el, "title", ss.map(nameOf).join(" · "));
      return;
    }
    el.dataset.sid = s.sessione;
    html(el.querySelector(".ics-ic"), iconOf(s, z === "asp" ? (s.vista === "bloccata" ? "blo" : "asp") : z === "lav" ? "lav" : "let", z === "let" ? 18 : 24));
    put(el.querySelector(".ics-nm"), nameOf(s));
    const pr = el.querySelector(".ics-pr"); if (pr) put(pr, p?.nome && p.nome !== nameOf(s) ? p.nome : "");
    if (z === "asp") {
      put(el.querySelector(".ics-q"), "da " + quanto(s.da));
      const ok = okOf(s.sessione);
      const whatText = s.serve_te_cosa || ok?.titolo || (s.vista === "bloccata" ? "Ti chiede un permesso" : "Ti aspetta");
      const detail = ok?.testo && ok.testo !== ok.titolo ? ok.testo : s.vista === "bloccata" ? s.domanda || "" : lines(s.risposta || s.domanda || "").slice(-3).join("\n");
      put(el.querySelector(".ics-cosa"), whatText);
      put(el.querySelector(".ics-dett"), detail === whatText ? "" : detail);
      const b = (az, t, cls = "") => `<button type="button" class="${cls}${provOk(ok, az)}" data-oka="${az}" data-id="${esc(ok.id)}">${t}</button>`;
      const confirm = ok && typeof barraOk === "function" ? barraOk(ok) : "";
      html(el.querySelector(".ics-tasti"), confirm || (ok ? b("ok", ok.fai_tu ? "Fatto" : "Ok", "nero") + (ok.fai_tu ? "" : b("no", "No")) + `<button type="button" data-ics-rispondi>Rispondi</button>` : `<button type="button" class="nero" data-ics-rispondi>Rispondi</button>`) + `<button type="button" data-ics-chat>Apri la chat</button>`);
    } else if (z === "lav") {
      attr(el.querySelector(".ad-tempo"), "data-da", s.da || 0);
      put(el.querySelector(".ics-ora-t"), s.adesso_chiaro || (s.lavora_sotto ? testoSotto(s.lavora_sotto) : "Lavora"));
      const bg = (Array.isArray(s.in_background) ? s.in_background : []).map((b) => b.descrizione || "lavoro in sottofondo");
      const sa = (s.sotto_recenti || []).filter((a) => a.stato === "lavora").map((a) => "aiutante: " + (a.descrizione || a.tipo || "al lavoro"));
      const chips = [...bg, ...sa];
      html(el.querySelector(".ics-bg"), chips.slice(0, 3).map((t) => `<span class="ics-chip"><i></i>${esc(t)}</span>`).join("") + (chips.length > 3 ? `<span class="ics-chip piu">+${chips.length - 3}</span>` : ""));
      const ps = el.querySelector(".ics-passi"), pi = s.passi_ic;
      show(ps, !!pi?.totale);
      if (pi?.totale) { const w = Math.round((pi.fatti / pi.totale) * 100) + "%"; const i = ps.querySelector("i"); if (i.style.width !== w) i.style.width = w; put(ps.querySelector("b"), `${pi.fatti} di ${pi.totale}`); }
      el.querySelector(".ics-ferma").classList.toggle("si", stoppable(s));
    } else {
      put(el.querySelector(".ics-ris"), lastLine(s.risposta) || "Turno finito");
      put(el.querySelector(".ics-q"), quanto(s.da));
    }
  }
  function fillAll() {
    const pageEl = page(); if (!pageEl) return;
    const v = liveSecs();
    for (const [k, el] of ST.el) { const x = v.get(k) || ST.forma.get(k); if (x && el.isConnected) fill1(el, x); }
    headline();
  }
  function headline() {
    const el = document.getElementById("ics-frase"); if (!el) return;
    const n = { asp: 0, lav: 0, let: 0 };
    for (const x of ST.forma.values()) if (x.sez in n) n[x.sez]++;
    const pz = [];
    if (n.lav) pz.push(`<b>${n.lav === 1 ? "1 al lavoro" : n.lav + " al lavoro"}</b>`);
    if (n.asp) pz.push(`<b class="asp">${n.asp === 1 ? "1 ti aspetta" : n.asp + " ti aspettano"}</b>`);
    if (n.let) pz.push(n.let === 1 ? "1 risposta da leggere" : n.let + " risposte da leggere");
    const last = Math.max(0, ...(dati?.sessioni || []).filter((s) => within(s) && s.vista !== "lavora").map((s) => s.da || 0));
    let t = pz.length ? pz.join(", ") + "." : "Tutto tranquillo: nessuno al lavoro e nessuno che ti aspetta.";
    if (last) t += ` Ultima risposta ${quanto(last)} fa.`;
    html(el, t);
  }

  // ---------- fixed order: position changes are applied at most every 5 s ----------
  const locked = () => ST.sopra || !!document.activeElement?.closest?.(".vista-ic .ics-scrivi, .vista-ic .ics-tasti .conf2");
  function pendingTimer(n) {
    const b = document.querySelector(".vista-ic .ics-attesa"); if (!b) return;
    b.classList.toggle("si", n > 0);
    const t = n === 1 ? "1 da spostare" : `${n} da spostare`;
    if (n > 0 && b.lastChild.textContent !== t) b.lastChild.textContent = t;
  }
  function schedule() {
    const n = pendingChanges();
    pendingTimer(locked() ? n : 0);
    if (!n) { clearTimeout(ST.timer); ST.timer = 0; return; }
    if (locked() || ST.timer) return;
    const delay = Math.max(COLLECT_MS, CHANGES_MS - (Date.now() - ST.applicato));
    ST.timer = setTimeout(() => { ST.timer = 0; if (!isOpen()) return; if (locked()) { pendingTimer(pendingChanges()); return; } apply(false); }, delay);
  }
  function apply(immediate) {
    clearTimeout(ST.timer); ST.timer = 0;
    const pageEl = page(); if (!pageEl) return;
    const v = liveSecs();
    const beforePos = !immediate && animaOk(pageEl) ? measure(pageEl) : null;
    for (const z of SEZ) ST.lay[z] = ST.lay[z].filter((k) => v.get(k)?.sez === z);
    // new ones: at the bottom in live sections, at the top among answers and stopped ones (the most recent)
    const added = [...v.values()].filter((x) => !ST.lay[x.sez].includes(x.k)).sort((a, b) => a.dal - b.dal || a.k.localeCompare(b.k));
    for (const x of added) if (x.sez === "fer" || x.sez === "let") ST.lay[x.sez].unshift(x.k); else ST.lay[x.sez].push(x.k);
    for (const [k, x] of v) ST.forma.set(k, x);
    for (const k of [...ST.forma.keys()]) if (!v.has(k)) ST.forma.delete(k);
    for (const z of SEZ) {
      const container = pageEl.querySelector(`[data-sez="${z}"]`);
      const keyList = z === "fer" && !ST.ferme ? ST.lay.fer.slice(0, MAXF) : ST.lay[z];
      let prev = null;
      for (const k of keyList) {
        let el = ST.el.get(k);
        if (!el) { el = fresh(k, z, v.get(k).sids[0]); ST.el.set(k, el); }
        const where = prev ? prev.nextSibling : container.firstChild;
        if (where !== el) container.insertBefore(el, where);
        prev = el;
      }
      for (const el of [...container.children]) if (!keyList.includes(el.dataset.k)) el.remove();
    }
    for (const k of [...ST.el.keys()]) if (!ST.forma.has(k)) ST.el.delete(k);
    headers(pageEl);
    fillAll();
    if (beforePos) slide(pageEl, beforePos);
    ST.applicato = Date.now();
    pendingTimer(0);
  }
  function headers(pageEl) {
    const n = { asp: ST.lay.asp.length, lav: ST.lay.lav.length, let: ST.lay.let.length, fer: 0 };
    for (const k of ST.lay.fer) n.fer += ST.forma.get(k)?.sids.length || 0;
    const sez = (z, textVal, below) => {
      const h = pageEl.querySelector(`[data-ick="h:${z}"]`), isEmpty = !ST.lay[z].length;
      show(h, !isEmpty); show(pageEl.querySelector(`[data-sez="${z}"]`), !isEmpty);
      if (!isEmpty) { h.firstChild.textContent = textVal; h.querySelector("span").textContent = below; }
    };
    sez("asp", n.asp === 1 ? "Ti aspetta" : "Ti aspettano", n.asp);
    sez("lav", "Al lavoro adesso", n.lav);
    sez("let", "Da leggere", n.let);
    sez("fer", "Fermi", `${n.fer} chat in ${ST.lay.fer.length} ${ST.lay.fer.length === 1 ? "progetto" : "progetti"}`);
    show(pageEl.querySelector('[data-ick="vuoto"]'), !ST.lay.asp.length && !ST.lay.lav.length && !ST.lay.let.length);
    const others = pageEl.querySelector('[data-ick="altre"]'), m = ST.lay.fer.length - MAXF;
    show(others, m > 0);
    if (m > 0) others.textContent = ST.ferme ? "Mostra meno" : `Altri ${m} · mostra`;
  }
  // FLIP: every element starts from its old place and slides to the new one; newcomers fade in, leavers fade out
  function measure(pageEl) {
    const m = new Map();
    for (const el of pageEl.querySelectorAll("[data-ick]")) { if (el._via) continue; const r = el.getBoundingClientRect(); if (r.width || r.height) m.set(el, r); }
    return { m };
  }
  function slide(pageEl, beforePos) {
    const after = new Map();
    for (const el of pageEl.querySelectorAll("[data-ick]")) { if (el._via) continue; const r = el.getBoundingClientRect(); if (r.width || r.height) after.set(el, r); }
    const vh = innerHeight, pr = pageEl.getBoundingClientRect(), moved = [], arrivals = [], gone = [];
    for (const [el, r] of beforePos.m) {
      const n = after.get(el);
      if (!n) { gone.push([el, r]); continue; }
      const dx = r.left - n.left, dy = r.top - n.top, outside = (r.bottom < 0 && n.bottom < 0) || (r.top > vh && n.top > vh);
      if (!outside && (Math.abs(dx) > 0.5 || Math.abs(dy) > 0.5)) moved.push([el, dx, dy]);
    }
    for (const el of after.keys()) if (!beforePos.m.has(el)) arrivals.push(el);
    if (!moved.length && !arrivals.length && !gone.length) return;
    for (const [el, dx, dy] of moved) { el.style.transition = "none"; el.style.transform = `translate(${dx}px,${dy}px)`; el.style.zIndex = "2"; }
    for (const el of arrivals) { el.style.transition = "none"; el.style.opacity = "0"; el.style.transform = "translateY(8px) scale(.985)"; }
    const ghosts = gone.map(([el, r]) => {
      const g = el.isConnected ? el.cloneNode(true) : el;
      g._via = true; g.removeAttribute("data-ick"); g.hidden = false; g.removeAttribute("id");
      Object.assign(g.style, { position: "absolute", top: r.top - pr.top + "px", left: r.left - pr.left + "px", width: r.width + "px", height: r.height + "px", margin: "0", pointerEvents: "none", transition: "none", transform: "", opacity: "1", zIndex: "1" });
      pageEl.appendChild(g);
      return g;
    });
    void pageEl.offsetWidth;
    const tr = `transform ${SLIDE_MS}ms ${MOLLA}, opacity ${Math.round(SLIDE_MS * 0.7)}ms ease`;
    const finish = (el) => { clearTimeout(el._tf); el._tf = setTimeout(() => { Object.assign(el.style, { transition: "", transform: "", opacity: "", zIndex: "" }); }, SLIDE_MS + 60); };
    for (const [el] of moved) { el.style.transition = tr; el.style.transform = ""; finish(el); }
    for (const el of arrivals) { el.style.transition = tr; el.style.opacity = ""; el.style.transform = ""; finish(el); }
    for (const g of ghosts) { g.style.transition = `opacity 220ms ease, transform ${SLIDE_MS}ms ${MOLLA}`; g.style.opacity = "0"; g.style.transform = "scale(.97)"; setTimeout(() => g.remove(), 260); }
  }

  // ---------- opening the view ----------
  const sectionH = (z) => `<div class="ics-sez" data-ick="h:${z}" hidden>x<span></span></div>`;
  const skeleton = () => `<p class="ics-frase" id="ics-frase"></p>
    ${sectionH("asp")}<div class="ics-aspetti" data-sez="asp" hidden></div>
    ${sectionH("lav")}<div class="ics-lavori" data-sez="lav" hidden></div>
    <div class="ics-vuoto" data-ick="vuoto" hidden>Nessuno al lavoro, nessuno che ti aspetta e niente da leggere.</div>
    ${sectionH("let")}<div class="ics-letti" data-sez="let" hidden></div>
    ${sectionH("fer")}<div class="ics-fermi" data-sez="fer" hidden></div><button type="button" class="ics-altre" data-ick="altre" data-ics-ferme hidden></button>`;
  window.apriInCorso = function () {
    vistaMac = true; vistaRq = false; saAperto = null; montata = null;
    if (typeof vistaCons !== "undefined") vistaCons = false;
    document.querySelectorAll(".riga.sel").forEach((x) => x.classList.remove("sel"));
    $("#main").innerHTML = `<div class="vista-mac vista-ic"><div class="barra" data-tauri-drag-region><span class="sa-ic">${ICON}</span>
      <div class="ic-testa" data-tauri-drag-region><h2>In corso</h2></div>
      <button type="button" class="ics-attesa" title="Cambi di posto in attesa: si applicano quando togli il puntatore dalla vista. Clic: adesso"><i></i><span></span></button>
      <button class="tasto" id="chiudi-mac" title="Chiudi (Esc)"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="m4 4 8 8M12 4l-8 8"/></svg></button></div>
      <div class="scroll"><div class="ic-pagina ics-pagina" id="ics-pagina">${skeleton()}</div></div></div>`;
    ST.el = new Map(); ST.sopra = false;
    const viewEl = document.querySelector(".vista-ic");
    viewEl.addEventListener("pointerenter", () => { ST.sopra = true; });
    viewEl.addEventListener("pointerleave", () => { ST.sopra = false; schedule(); });
    viewEl.addEventListener("focusout", () => setTimeout(schedule, 0));
    apply(true);
    invoke("ok_voci").then((x) => { datiOk = x; if (isOpen()) fillAll(); }).catch(() => {});
  };
  window.disegnaInCorso = function () { if (isOpen()) { fillAll(); schedule(); } };
  window.inCorsoDati = function () { if (isOpen()) { fillAll(); schedule(); } };
  let okT = 0;
  T.event.listen("cambiato", () => {
    if (!isOpen()) return;
    clearTimeout(okT);
    okT = setTimeout(() => { if (isOpen()) invoke("ok_voci").then((x) => { datiOk = x; if (isOpen()) fillAll(); }).catch(() => {}); }, 900);
  });
  document.addEventListener("visibilitychange", () => { if (!document.hidden && isOpen()) schedule(); });

  // ---------- buttons ----------
  function sendTo(f) {
    const inp = f.querySelector("input"), id = f.dataset.sid, t = inp.value.trim();
    if (!t || !id) return inp.focus();
    inp.value = ""; f.hidden = true;
    const s = sd(id);
    manda(id, t);
    toast(`Mandato a ${s ? nameOf(s) : "la chat"}`, 2000);
  }
  document.addEventListener("click", (e) => {
    if (e.target.closest?.("#incorso-btn")) { e.preventDefault(); e.stopPropagation(); return apriInCorso(); }
    const viewEl = e.target.closest?.(".vista-ic"); if (!viewEl) return;
    if (e.target.closest(".ics-attesa")) { e.stopPropagation(); return apply(false); }
    if (e.target.closest("[data-ics-ferme]")) { e.stopPropagation(); ST.ferme = !ST.ferme; return apply(false); }
    const el = e.target.closest("[data-k]"), sid = el?.dataset.sid;
    if (e.target.closest("[data-ics-ferma]")) { e.stopPropagation(); if (sid) invoke("interrompi", { id: sid }).then((r) => toast(r?.ok ? "Fermata" : r?.msg || "Non riuscito")).catch(() => {}); return; }
    if (e.target.closest("[data-ics-letto]")) {
      e.stopPropagation(); if (!sid) return;
      el.style.opacity = ".4";
      invoke("segna_letta", { id: sid }).then(() => caricaElenco()).catch(() => { el.style.opacity = ""; });
      return;
    }
    if (e.target.closest("[data-ics-rispondi]")) {
      e.stopPropagation();
      const f = el?.querySelector(".ics-scrivi"); if (!f) return;
      f.hidden = false; f.querySelector("input").focus();
      return;
    }
    if (e.target.closest("[data-ics-chat]")) { e.stopPropagation(); if (sid) scegli(sid); return; }
    if (e.target.closest("[data-oka], .conf2, .ics-scrivi, #chiudi-mac, .ics-tasti")) return;
    if (el && sid) { e.stopPropagation(); scegli(sid); }
  }, true);
  document.addEventListener("submit", (e) => {
    const f = e.target.closest?.(".vista-ic .ics-scrivi"); if (!f) return;
    e.preventDefault(); sendTo(f);
  });
  document.addEventListener("keydown", (e) => {
    if (e.target.matches?.("[data-ics-scrivi]")) {
      const f = e.target.closest(".ics-scrivi");
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); e.target.value = ""; e.target.blur(); f.hidden = true; return; }
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); e.stopPropagation(); sendTo(f); return; }
      e.stopPropagation(); return;
    }
    if (e.metaKey && !e.shiftKey && !e.altKey && !e.ctrlKey && e.key === "0") { e.preventDefault(); e.stopPropagation(); apriInCorso(); }
  }, true);

  // ---------- style (only inside .vista-ic) ----------
  const css = `
.vista-ic .scroll{overflow-anchor:none}
.vista-ic .ics-pagina{position:relative;max-width:1180px;margin:0 auto;padding:18px 28px 40px}
.vista-ic .ics-attesa{flex:none;display:inline-flex;align-items:center;gap:6px;height:26px;padding:0 10px;border-radius:7px;font-size:12px;color:var(--ink2);background:var(--fill);opacity:0;pointer-events:none;transition:opacity .25s;white-space:nowrap}
.vista-ic .ics-attesa.si{opacity:1;pointer-events:auto}
.vista-ic .ics-attesa i{width:6px;height:6px;border-radius:50%;background:var(--ink3)}
.ics-frase{margin:0 0 4px;color:var(--ink2);font-size:13.5px;line-height:1.5;min-height:20px}
.ics-frase b{color:var(--ink);font-weight:600}.ics-frase b.asp{color:#ff5b1f}
.ics-sez{display:flex;align-items:baseline;gap:8px;margin:24px 0 10px;font-size:11px;font-weight:600;color:var(--ink3);text-transform:uppercase;letter-spacing:.06em}
.ics-sez span{font-weight:400;text-transform:none;letter-spacing:0}
.ics-sez[hidden],.ics-aspetti[hidden],.ics-lavori[hidden],.ics-letti[hidden],.ics-fermi[hidden],.ics-altre[hidden],.ics-vuoto[hidden],.ics-passi[hidden],.ics-scrivi[hidden]{display:none}
.ics-aspetti{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(440px,100%),1fr));gap:12px}
.ics-lavori{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(262px,100%),1fr));gap:12px}
.ics-testa{display:flex;align-items:center;gap:9px;min-width:0;height:26px}
.ics-ic{flex:none;display:grid}
.ics-nm{font-weight:600;font-size:13.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0}
.ics-pr{color:var(--ink3);font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0}
.ics-q{margin-left:auto;flex:none;font:500 11.5px/1 var(--mono);color:var(--ink3);font-variant-numeric:tabular-nums;white-space:nowrap}
.ics-asp{position:relative;border:1px solid var(--line);border-radius:14px;padding:14px 16px 14px 18px;background:var(--bg);cursor:pointer;transition:border-color .2s,box-shadow .2s}
.ics-asp::before{content:"";position:absolute;left:0;top:14px;bottom:14px;width:3px;border-radius:0 3px 3px 0;background:#ff5b1f}
.ics-asp:hover,.ics-lav:hover{border-color:var(--fill2);box-shadow:0 6px 22px rgba(0,0,0,.07)}
.ics-cosa{margin:10px 0 0;font-size:14px;line-height:1.45;color:var(--ink)}
.ics-dett{margin:4px 0 0;color:var(--ink2);font-size:12.5px;line-height:1.5;white-space:pre-line;display:-webkit-box;-webkit-line-clamp:3;-webkit-box-orient:vertical;overflow:hidden}
.ics-dett:empty{display:none}
.ics-tasti{display:flex;flex-wrap:wrap;gap:6px;margin-top:12px}
.ics-tasti>button{font:500 12px/1 var(--sans);height:28px;padding:0 12px;border-radius:8px;border:1px solid var(--line);color:var(--ink);background:var(--bg)}
.ics-tasti>button.nero{background:var(--ink);color:var(--bg);border-color:var(--ink)}
.ics-tasti>button:hover{border-color:var(--ink3)}
.ics-tasti .conf2{margin:0;flex:1}
.ics-scrivi{display:flex;align-items:center;gap:8px;margin-top:10px;border:1px solid var(--ink3);border-radius:10px;padding:3px 3px 3px 10px;background:var(--side)}
.ics-scrivi input{flex:1;min-width:0;border:0;background:transparent;font:13px var(--sans);color:var(--ink);outline:none;padding:5px 0}
.ics-scrivi button{width:26px;height:26px;border-radius:7px;background:var(--ink);display:grid;place-items:center;flex:none}
.ics-scrivi button svg{width:12px;height:12px;stroke:var(--bg)}
.ics-lav{border:1px solid var(--line);border-radius:14px;padding:13px 15px 12px;background:var(--bg);display:flex;flex-direction:column;gap:9px;min-height:150px;cursor:pointer;transition:border-color .2s,box-shadow .2s}
.ics-ora{flex:1;font-size:13.5px;line-height:1.45;display:-webkit-box;-webkit-line-clamp:3;-webkit-box-orient:vertical;overflow:hidden}
.ics-ora .el-pensa{display:inline-flex;vertical-align:middle;margin-right:6px}
.ics-bg{display:flex;gap:6px;flex-wrap:wrap}.ics-bg:empty{display:none}
.ics-chip{display:inline-flex;align-items:center;gap:6px;font-size:11.5px;color:var(--ink2);background:var(--fill);border-radius:7px;padding:4px 8px;max-width:100%;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.ics-chip i{flex:none;width:10px;height:10px;border-radius:50%;border:1.5px solid var(--fill2);border-top-color:var(--ink2);animation:icsGira .9s linear infinite}
.ics-chip.piu i{display:none}
.ics-piede{display:flex;align-items:center;gap:10px;min-height:16px}
.ics-passi{display:flex;align-items:center;gap:8px;flex:1}
.ics-barra{flex:1;height:3px;border-radius:2px;background:var(--fill2);overflow:hidden}
.ics-barra i{display:block;height:100%;width:0;background:var(--ink);transition:width .6s cubic-bezier(.2,.9,.25,1)}
.ics-passi b{font-weight:500;font-size:11px;color:var(--ink3);font-variant-numeric:tabular-nums;white-space:nowrap}
.ics-passi:not([hidden])~.ics-pr{display:none}
.ics-ferma{flex:none;display:inline-flex;align-items:center;gap:4px;height:22px;padding:0 8px;border-radius:7px;font-size:11.5px;color:var(--ink2);border:1px solid var(--line);visibility:hidden;opacity:0;transition:opacity .2s;margin-left:auto}
.ics-ferma svg{width:10px;height:10px}
.ics-ferma.si{visibility:visible}
.ics-lav:hover .ics-ferma.si{opacity:1}
.ics-ferma:hover{color:var(--ink);border-color:var(--ink3)}
.ics-ferma+.ics-q{margin-left:8px}
.ics-letti{border:1px solid var(--line);border-radius:14px;overflow:hidden;display:flex;flex-direction:column}
.ics-let{display:flex;align-items:center;gap:10px;padding:10px 14px;border-top:1px solid var(--line);text-align:left;width:100%;background:var(--bg)}
.ics-let:first-child{border-top:0}
.ics-let:hover{background:var(--fill)}
.ics-let .ics-t{flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:var(--ink2);font-size:12.5px}
.ics-let .ics-t .ics-nm{font-size:12.5px;margin-right:6px}
.ics-letto{font-size:12px;color:var(--ink3);padding:5px 9px;border-radius:7px}
.ics-letto:hover{color:var(--ink);background:var(--fill2)}
.ics-fermi{display:grid;grid-template-columns:repeat(auto-fill,minmax(176px,1fr));gap:8px}
.ics-fer{display:flex;align-items:center;gap:10px;padding:9px 11px;border-radius:11px;background:var(--fill);text-align:left;min-width:0;transition:background .2s}
.ics-fer:hover{background:var(--fill2)}
.ics-fer .ics-t{min-width:0;display:flex;flex-direction:column}
.ics-fer .ics-nm{font-weight:500;font-size:12.5px}
.ics-sub{color:var(--ink3);font-size:11.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.ics-altre{display:block;font-size:12px;color:var(--ink3);padding:10px 2px;text-align:left}
.ics-altre:hover{color:var(--ink)}
.ics-vuoto{color:var(--ink3);font-size:13px;padding:34px 0;text-align:center}
.ics-cambia{animation:icsArriva .2s ease}
@keyframes icsArriva{from{opacity:.2}to{opacity:1}}
@keyframes icsGira{to{transform:rotate(360deg)}}
@media (prefers-reduced-motion: reduce){.vista-ic .ics-cambia,.vista-ic .ics-chip i{animation:none}.vista-ic .ics-barra i{transition:none}}`;
  const st = document.createElement("style"); st.id = "incorso-css"; st.textContent = css; document.head.appendChild(st);
})();
