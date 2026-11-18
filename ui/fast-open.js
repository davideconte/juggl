// fast chats (Nov 14): open a chat without waiting and without empty screens.
// - cache of the last 8 chats opened or prepared (16 MB cap): used ONLY if the list row has not changed
//   since then (firmaRiga: state, last activity, answer), then the fresh version arrives anyway
// - skeleton: if the chat does not arrive within 80 ms, real title + blurred bubbles; past 1.5 s the j mark with the orange
//   dot; past 4 s "e' una chat grande"
// - errors: retries by itself 2 times, then a box with the reason and Retry. A hanging request times out at 15 s
// - older messages: the chat arrives with the latest ones (read cap on the Mac), scrolling to the top loads the
//   others 120 at a time, without moving what you are reading
// - chat prepared when you rest 150 ms on a list row (never if the chat you are reading is working)
// Hooks in index.html: caricaDettaglioV, applicaDettaglio, monta, scegli ("veloci (14/11)" blocks).
(() => {
  const VEL = (window.VEL = {});
  const css = `
.vel-sch{padding-top:28px}
.vel-sch .vel-b{opacity:0;animation:velEntra .32s ease-out .12s forwards}
.vel-b.tu{margin:12px 0 12px auto;height:34px;border-radius:18px;background:var(--fill2)}
.vel-b.ag{margin:16px 0}
.vel-b.ag i{display:block;height:11px;border-radius:6px;background:var(--fill);margin:9px 0}
.vel-sch .vel-b:nth-child(2){animation-delay:.16s}.vel-sch .vel-b:nth-child(3){animation-delay:.2s}.vel-sch .vel-b:nth-child(4){animation-delay:.24s}
/* caricamenti (16/11): riflesso lento che si sposta con transform (lo muove la scheda video, niente ridisegno a ogni
   fotogramma: prima il riflesso spostava lo sfondo e costava ~47 ms di lavoro al secondo). Stesso riflesso nel nastro. */
.vel-b i,.vel-b.tu,.fm-vuota{position:relative;overflow:hidden}
.vel-b i::after,.vel-b.tu::after,.fm-vuota::after{content:"";position:absolute;inset:0;background:linear-gradient(100deg,transparent 25%,rgba(127,127,127,.11) 50%,transparent 75%);transform:translateX(-100%);animation:velRiflesso 2.4s ease-in-out .4s 8}
/* al massimo 8 passaggi (~20 s): poi il riquadro resta fermo e non costa piu' niente; mai sui file che non arriveranno */
.fm-vuota.fm-ferma::after{display:none}
.fm-vuota{display:block;background:var(--fill)}
.fm-img.fm-entra{opacity:0;transition:opacity .2s ease-out}
.fm-img.fm-entra.caricata{opacity:1}
.fuori-vista,.fuori-vista *,.fuori-vista::after,.fuori-vista *::after{animation-play-state:paused}
.vel-attesa{display:flex;flex-direction:column;align-items:center;gap:10px;margin:34px 0 0;opacity:0;animation:velEntra .4s ease-out 1.5s forwards;color:var(--ink3);font-size:12.5px}
.vel-j{width:30px;height:30px;border-radius:8px;background:var(--ink);color:var(--bg);display:grid;place-items:center}
.vel-j svg{width:20px;height:20px;overflow:visible}
.vel-j circle{animation:velPunto 1.4s ease-in-out infinite;transform-origin:23.9px 6.1px}
.vel-err{margin:40px auto 0;max-width:420px;border:1px solid var(--line);border-radius:14px;padding:16px 18px;text-align:center;font-size:13px;color:var(--ink2);animation:velEntra .25s ease-out both}
.vel-err b{display:block;color:var(--ink);font-size:14px;margin-bottom:4px;font-weight:600}
.vel-err small{display:block;color:var(--ink3);margin:2px 0 12px;word-break:break-word}
.vel-casella{height:44px;border:1px solid var(--line);border-radius:14px;opacity:.6}
.vel-vecchi{display:flex;justify-content:center;padding:14px 0 0;min-height:22px}
.vel-vecchi[hidden]{display:none}
.vel-vecchi button{font:inherit;font-size:12px;color:var(--ink3);background:none;border:0;cursor:pointer;display:inline-flex;align-items:center;gap:7px;padding:4px 10px;border-radius:8px}
.vel-vecchi button:hover{color:var(--ink);background:var(--fill)}
.vel-vecchi .ruota{width:11px;height:11px;border-radius:50%;border:1.5px solid var(--fill2);border-top-color:var(--ink3);animation:velGira .8s linear infinite}
.colonna.vel-arriva .r-thread{animation:velArriva .16s ease-out}
/* avvio (17/11): finche' i dati non arrivano, mai valori vuoti. Colonna centrale: marchio j col puntino arancio che
   respira e "Riapro le tue chat…" (compare dopo 150 ms: con un avvio veloce non lampeggia); fascia: pillola scheletro.
   La barra laterale disegna i suoi scheletri in sidebar.js con la stessa classe html.in-avvio. */
.vel-avvio{margin:auto;display:flex;flex-direction:column;align-items:center;gap:14px;color:var(--ink3);font-size:13px;text-align:center;opacity:0;animation:velEntra .4s ease-out .15s forwards}
.vel-avvio-j{width:46px;height:46px;border-radius:12px;background:var(--ink);color:var(--bg);display:grid;place-items:center}
.vel-avvio-j svg{width:30px;height:30px;overflow:visible}
.vel-avvio-j circle{transform-box:fill-box;transform-origin:center;animation:velRespira 2.4s ease-in-out infinite}
.vel-avvio-t{margin:0;min-height:18px}
.vel-avvio small{display:block;color:var(--ink3);max-width:340px;margin-top:-6px;word-break:break-word}
html.in-avvio #fascia:empty::before{content:"";display:block;width:132px;height:22px;margin:10px 16px;border-radius:11px;background:var(--fill);animation:velPulsa 1.8s ease-in-out infinite}
html.avvio-arriva .side,html.avvio-arriva #main,html.avvio-arriva #fascia{animation:velArrivo .32s ease-out}
html.in-avvio #fm-binario:not(:has(.fm-t))::after{content:"";flex:none;align-self:flex-start;margin-top:2px;width:min(640px,60vw);height:var(--nm-h,72px);border-radius:10px;-webkit-mask:repeating-linear-gradient(90deg,#000 0 112px,transparent 112px 124px);mask:repeating-linear-gradient(90deg,#000 0 112px,transparent 112px 124px);background:var(--fill);animation:velPulsa 1.8s ease-in-out infinite}
@keyframes velRespira{0%,100%{transform:scale(1);opacity:1}50%{transform:scale(.7);opacity:.6}}
@keyframes velPulsa{0%,100%{opacity:.55}50%{opacity:1}}
@keyframes velArrivo{from{opacity:.25}to{opacity:1}}
@keyframes velEntra{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}
@keyframes velArriva{from{opacity:.35}to{opacity:1}}
@keyframes velRiflesso{0%{transform:translateX(-100%)}45%,100%{transform:translateX(100%)}}
@keyframes velPunto{0%,100%{transform:scale(1);opacity:1}50%{transform:scale(.72);opacity:.55}}
@keyframes velGira{to{transform:rotate(360deg)}}
@media (prefers-reduced-motion:reduce){.vel-avvio{animation:none;opacity:1}.vel-avvio-j circle,html.in-avvio #fascia:empty::before,html.in-avvio #fm-binario::after{animation:none}html.avvio-arriva .side,html.avvio-arriva #main,html.avvio-arriva #fascia{animation:none}.vel-sch .vel-b,.vel-attesa,.vel-err{animation:none;opacity:1}.vel-b i::after,.vel-b.tu::after,.fm-vuota::after{animation:none;display:none}.fm-img.fm-entra{transition:none}.vel-j circle{animation:none}.colonna.vel-arriva .r-thread{animation:none}}
`;
  const st = document.createElement("style"); st.id = "veloci-css"; st.textContent = css; document.head.appendChild(st);

  const isSpecial = (id) => !id || /^(locale|wa|chiusa):/.test(id);   // local chats (model on the PC) and the like: never cached

  // ---------- chat cache ----------
  const cache = new Map();   // id -> { d, firma, peso, t }
  const CAP = 16e6, MAX = 8, MAX_VOCE = 3e6, TIMEOUT_MS = 10 * 60e3;
  const lowMemory = () => { const p = dati?.mac?.libera_pct; return typeof p === "number" && p < 6; };
  const weigh = (d) => { try { return JSON.stringify(d).length * 2; } catch { return Infinity; } };
  VEL.metti = (id, d) => {
    if (isSpecial(id) || !d || d.sessione !== id) return;
    if (lowMemory()) { cache.clear(); return; }
    const c = cache.get(id);
    // the same chat updated often (working): recompute the weight at most every 30 s
    const peso = c && c.d !== d && Date.now() - c.t < 30000 ? c.peso : c && c.d === d ? c.peso : weigh(d);
    if (peso > MAX_VOCE) { cache.delete(id); return; }
    cache.delete(id); cache.set(id, { d, firma: firmaRiga(id), peso, t: Date.now() });
    let total = 0; for (const x of cache.values()) total += x.peso;
    while (cache.size > MAX || total > CAP) { const [k, x] = cache.entries().next().value; cache.delete(k); total -= x.peso; }
  };
  /** The cached chat, only if nothing changed in the list since it was stored. */
  VEL.prendi = (id) => {
    const x = cache.get(id); if (!x) return null;
    const f = firmaRiga(id);
    if (!f || x.firma !== f || Date.now() - x.t > TIMEOUT_MS) { cache.delete(id); return null; }
    return x.d;
  };
  VEL.stato = () => ({ voci: cache.size, mb: +([...cache.values()].reduce((a, x) => a + x.peso, 0) / 1e6).toFixed(1) });

  // ---------- request with timeout ----------
  /** invoke("sessione") that does not hang: past 15 s it is an error (the late result is discarded). */
  VEL.chiedi = (args) => new Promise((ok, no) => {
    const t = setTimeout(() => no(new Error("la chat non risponde da 15 secondi")), 15000);
    invoke("sessione", args).then((d) => { clearTimeout(t); ok(d); }, (e) => { clearTimeout(t); no(e); });
  });

  // ---------- skeleton ----------
  const rowOf = (id) => dati.sessioni.find((x) => x.sessione === id) || null;
  const nameOf = (id) => { const s = dati.sessioni.find((x) => x.sessione === id) || (typeof archiviataDi === "function" ? archiviataDi(id) : null); return s ? (s.sessione === dati.juggl ? "Juggl" : s.nome || "Chat") : "Chat"; };
  const J = '<svg viewBox="0 0 48 48" aria-hidden="true"><g transform="translate(9.05 -0.05)"><path d="M14 16V36a8 8 0 0 1-8 8h-2" fill="none" stroke="currentColor" stroke-width="8" stroke-linecap="round"/><circle cx="23.9" cy="6.1" r="6" fill="#ff5b1f"/></g></svg>';
  const BUBBLES = `<div class="vel-b tu" style="width:38%"></div><div class="vel-b ag"><i style="width:94%"></i><i style="width:81%"></i><i style="width:58%"></i></div><div class="vel-b tu" style="width:26%"></div><div class="vel-b ag"><i style="width:88%"></i><i style="width:67%"></i></div>`;
  function skel(V, id, body) {
    if (!V.el) return;
    const tr = multi() ? "" : " data-tauri-drag-region";
    V.osservatore?.disconnect();
    // header like the real one (icon, name, subtitle from the list row): it does not jump when the chat arrives
    const r = rowOf(id);
    let sub = ""; try { sub = r ? sottotitolo(r) : ""; } catch {}
    V.el.innerHTML = `<div class="barra"${tr}>${testaCol(V)}${r ? avatar(r, "piccolo") : ""}<div class="tit"${tr}><h2>${esc(nameOf(id))}</h2><div class="sub vel-sub">${esc(sub) || "&nbsp;"}</div></div>${BTN_CHIUDI_COL}</div>
      <div class="scroll"><div class="thread vel-sch" aria-busy="true" aria-label="Carico la chat">${body}</div></div>
      <div class="compositore"><div class="vel-casella"></div></div>`;
    V.el._h = ""; V.el._scheletro = id;
  }
  const waitTimer = (t) => `<div class="vel-attesa"><span class="vel-j">${J}</span><span class="vel-t">${t}</span></div>`;
  function showSkeleton(V, id) {
    if (V.montata === id || V.id !== id || V.el?._scheletro === id) return;
    skel(V, id, BUBBLES + waitTimer("Carico la chat…"));
    clearTimeout(V._velLungo);
    V._velLungo = setTimeout(() => { const t = V.el?._scheletro === id && V.el.querySelector(".vel-t"); if (t) t.textContent = "È una chat lunga, ancora un attimo…"; }, 4000);
  }
  function showError(V, id, why) {
    clearTimeout(V._velLungo);
    skel(V, id, `<div class="vel-err" role="alert"><b>Non riesco a caricare questa chat</b><small>${esc(why)}</small><button class="btn-nero" data-vel-riprova>Riprova</button></div>`);
  }
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-vel-riprova]"); if (!b) return;
    const V = vistaDi(b); if (!V?.id) return;
    V._velTent = 0; V.el._scheletro = null; showSkeleton(V, V.id); caricaDettaglioV(V, true);
  });

  /** Before asking for the chat (not on screen yet): from the cache right away, otherwise the skeleton after 80 ms. */
  VEL.primaDiCaricare = (V, id, fromOffset) => {
    if (V._velId !== id) { V._velId = id; V._velTent = 0; }
    if (V.montata === id) return false;
    const c = !fromOffset && VEL.prendi(id);
    if (c) { applicaDettaglio(V, c, true); V._velDaCache = true; return true; }
    clearTimeout(V._velT);
    if (V.el?._scheletro !== id) V._velT = setTimeout(() => { if (V.carica && V.id === id && V.montata !== id) showSkeleton(V, id); }, 80);
    return false;
  };
  /** The chat arrived: into the cache, older-messages row, fade if it came from the skeleton. */
  VEL.arrivata = (V, d, wasSkeleton) => {
    clearTimeout(V._velT); clearTimeout(V._velLungo);
    V._velTent = 0;
    if (!V.detDal) VEL.metti(d.sessione, d);
    if (wasSkeleton && V.el && !RIDOTTO) { V.el.classList.add("vel-arriva"); setTimeout(() => V.el?.classList.remove("vel-arriva"), 200); }
    olderRow(V);
  };
  /** It did not arrive (error, null, timeout): retry by myself twice, then say so. */
  VEL.fallita = (V, id, errore) => {
    clearTimeout(V._velT);
    if (V.id !== id) return;
    const why = errore || "la chat non si trova più: forse è stata chiusa, archiviata o spostata";
    if (V.montata === id) {   // already on screen: retry silently, remove nothing
      if ((V._velTentA = (V._velTentA || 0) + 1) <= 3) setTimeout(() => { if (V.id === id) caricaDettaglioV(V); }, 1500 * V._velTentA);
      return;
    }
    V._velTent = (V._velTent || 0) + 1;
    console.warn("veloci: chat non caricata", id, why, "tentativo", V._velTent);
    if (V._velTent <= 2) {
      showSkeleton(V, id);
      const t = V.el?.querySelector(".vel-t"); if (t) t.textContent = "Non è arrivata, riprovo…";
      setTimeout(() => { if (V.id === id && V.montata !== id) caricaDettaglioV(V, true); }, 600 * V._velTent);
    } else showError(V, id, why);
  };

  // ---------- older messages ----------
  function olderRow(V) {
    const r = qv(V, "vecchi"); if (!r) return;
    const si = !!V.det?.piu_vecchi;
    r.hidden = !si;
    if (!si) return;
    const h = V._velVecchi === "carico" ? `<button disabled><span class="ruota"></span>Carico i messaggi più vecchi…</button>`
      : V._velVecchi === "errore" ? `<button data-vel-vecchi>Messaggi più vecchi non caricati · riprova</button>` : `<button data-vel-vecchi>Messaggi più vecchi</button>`;
    if (r._h !== h) { r.innerHTML = h; r._h = h; }
  }
  VEL.vecchi = async (V) => {
    const d = V.det; if (!d?.piu_vecchi || V._velVecchi === "carico" || V.carica || !V.id) return;
    const firstEl = (d.eventi || []).find((e) => e.t)?.t; if (!firstEl) return;
    const id = V.id, sc = qv(V, "scroll"), th = qv(V, "thread"); if (!sc || !th) return;
    V._velVecchi = "carico"; olderRow(V);
    let nd = null;
    try { nd = await VEL.chiedi({ id, dal: firstEl, prima: 120 }); } catch (e) { console.warn("veloci: vecchi", e); }
    if (V.id !== id || !V.el?.isConnected) return;
    if (!nd || nd.sessione !== id) { V._velVecchi = "errore"; olderRow(V); return; }
    V._velVecchi = "";
    // the first block you see stays at the same spot on screen (also while scrolling: the app anchor stops there)
    const top = sc.getBoundingClientRect().top;
    const ref = [...th.children].find((el) => el.dataset?.k && el.getBoundingClientRect().bottom > top + 1);
    const k = ref?.dataset.k, off = ref ? ref.getBoundingClientRect().top - top : 0;
    V.dal = firstEl; V.prima = 120; V.detDal = firstEl;
    V.det = nd; V.tieniPos = true; V.attaccato = false;   // you are at the top: the bottom must not pull you back
    ridisegnaV(V, false);
    olderRow(V);   // before restoring the view: the top row disappearing would shift everything by ~40 px
    const el = k && th.querySelector(`[data-k="${CSS.escape(k)}"]`);
    if (el) sc.scrollTop += el.getBoundingClientRect().top - top - off;
  };
  /** Inside monta: top row and scroll listener. */
  VEL.monta = (V) => {
    const sc = qv(V, "scroll"); if (!sc) return;
    sc.addEventListener("scroll", () => { if (sc.scrollTop < 500 && V.det?.piu_vecchi && V._velVecchi !== "errore") VEL.vecchi(V); }, { passive: true });
    qv(V, "vecchi")?.addEventListener("click", (e) => { if (e.target.closest("[data-vel-vecchi]")) { V._velVecchi = ""; VEL.vecchi(V); } });
  };

  // ---------- startup (Nov 17) ----------
  // in-avvio is set by the script at the top of index.html. First successful "tavolo": remove the class, juggl-pronto event,
  // smooth transition and one log line with the time (only the number). If it does not arrive: retry by itself; after 10 s say so,
  // with the reason and Retry (and keep retrying).
  let startupFirst = 0, startupRetry = 0;
  const startT = () => Math.round(performance.now() - (window.__avvio0 || 0));
  const startupText = (t, subText, btnEl) => {
    const el = document.getElementById("vel-avvio"); if (!el) return;
    el.querySelector(".vel-avvio-t").textContent = t;
    el.querySelector("small")?.remove(); el.querySelector("button")?.remove();
    if (subText) { const s = document.createElement("small"); s.textContent = subText; el.append(s); }
    if (btnEl) { const b = document.createElement("button"); b.className = "btn-nero"; b.textContent = "Riprova"; b.onclick = () => { startupText("Riapro le tue chat…"); caricaElenco(); }; el.append(b); }
  };
  const startT1 = setTimeout(() => { if (!window.JUGGL_PRONTO) startupText("Ancora un attimo…"); }, 4000);
  // opening (Nov 17): at 10 s the error with Retry only if the program answered with an error; if it is just slow
  // (12 s measured on 0.1.85) it stays "Ancora un attimo" and the warning comes at 30 s
  const startupErr = () => startupText("Non riesco ancora a leggere le tue chat", startupFirst ? "Ultimo errore: " + startupFirst : "Il programma non ha ancora risposto.", true);
  const startT2 = setTimeout(() => { if (!window.JUGGL_PRONTO && startupFirst) startupErr(); }, 10000);
  const startT3 = setTimeout(() => { if (!window.JUGGL_PRONTO) startupErr(); }, 30000);
  VEL.avvioPronto = () => {
    if (window.JUGGL_PRONTO) return;
    window.JUGGL_PRONTO = true; clearTimeout(startT1); clearTimeout(startT2); clearTimeout(startT3); clearTimeout(startupRetry);
    const ms = startT(), h = document.documentElement;
    h.classList.remove("in-avvio");
    if (!RIDOTTO) { h.classList.add("avvio-arriva"); setTimeout(() => h.classList.remove("avvio-arriva"), 400); }
    dispatchEvent(new Event("juggl-pronto"));
    invoke("segna_avvio", { ms }).catch(() => {});
  };
  VEL.avvioFallito = (e) => {
    if (window.JUGGL_PRONTO) return;
    startupFirst = String(e?.message || e || "").slice(0, 160);
    clearTimeout(startupRetry);
    startupRetry = setTimeout(() => caricaElenco(), startT() < 10000 ? 700 : 3000);
  };

  // ---------- strip uploads (Nov 16) ----------
  // The shimmer runs only on visible empty boxes, at most 5 together: the others have .fuori-vista
  // (same rule as the sidebar indicators). The loaded thumbnail enters with a 200 ms fade.
  const visibleSet = new Set();
  const updateVisible = () => { let k = 0; for (const el of visibleSet) el.classList.toggle("fuori-vista", ++k > 5 || !el.isConnected); };
  const io = typeof IntersectionObserver === "function" ? new IntersectionObserver((voci) => {
    for (const v of voci) { if (v.isIntersecting) visibleSet.add(v.target); else { visibleSet.delete(v.target); v.target.classList.add("fuori-vista"); } }
    updateVisible();
  }) : null;
  VEL.osservaVuota = (el) => { if (!el || !io) return; el.classList.add("fuori-vista"); io.observe(el); };
  VEL.dimenticaVuota = (el) => { if (!el || !io) return; io.unobserve(el); visibleSet.delete(el); updateVisible(); };
  VEL.entra = (img) => { if (!img) return; const done = () => img.classList.add("caricata"); if (img.complete && img.naturalWidth) done(); else img.addEventListener("load", done, { once: true }); };

  // ---------- chat prepared on mouse hover ----------
  let prepTimer = 0, prepId = null, prepRunning = false;
  VEL.prepara = async (id) => {
    if (isSpecial(id) || prepRunning || viste.some((v) => v.id === id) || VEL.prendi(id)) return;
    if (viste.some((v) => v.det?.vista === "lavora" && mostrate().includes(v))) return;   // never while you read a working chat
    prepRunning = true;
    try { const d = await VEL.chiedi({ id }); if (d && d.sessione === id && !viste.some((v) => v.id === id)) VEL.metti(id, d); } catch {}
    prepRunning = false;
  };
  // neighbors (Nov 16): nearby chats to keep ready (list and history, ⌃Tab and ⌘[ ⌘]). The sidebar calls
  // window.preparaChat(EL.vicine()) after every chat change: they are prepared in a queue, one at a time, after 400 ms and
  // with 250 ms between each; a new list replaces the old one. A single id = like mouse hover.
  let prepQueue = [], queueTimer = 0, lastList = "", lastT = 0;
  const advance = async () => {
    queueTimer = 0;
    const id = prepQueue.shift(); if (!id) return;
    if (prepRunning) { prepQueue.unshift(id); queueTimer = setTimeout(advance, 250); return; }
    await VEL.prepara(id);
    if (prepQueue.length) queueTimer = setTimeout(advance, 250);
  };
  window.preparaChat = (x) => {
    if (!Array.isArray(x)) return VEL.prepara(x);
    const incoming = [...new Set(x.filter((id) => id && !isSpecial(id)))].slice(0, 4);
    // same list as before (the sidebar sends it on every redraw): nothing to redo, the wait does not restart
    if (incoming.join("|") === lastList && Date.now() - lastT < 30000) return;   // refreshed after 30 s
    lastList = incoming.join("|"); lastT = Date.now();
    prepQueue = incoming;
    clearTimeout(queueTimer); queueTimer = prepQueue.length ? setTimeout(advance, 400) : 0;
  };
  // pointerover bubbles (mouseenter does not); I do not stop propagation: the list's fixed order handler is on the same event
  document.addEventListener("pointerover", (e) => {
    const r = e.target.closest?.(".elenco .riga[data-id]");
    const id = r?.dataset.id || null;
    if (id === prepId) return;
    clearTimeout(prepTimer); prepId = id;
    if (id) prepTimer = setTimeout(() => VEL.prepara(id), 150);
  }, { passive: true });
})();
