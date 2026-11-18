// Live edits (Nov 12, src-tauri/src/live_edits.rs).
// 1) Under every answer that changed files: row "3 file +42 −7"; a click opens the panel with the diff file by file,
//    Copy diff and Undo (the whole answer or one file, with confirmation). While the chat works the row says "finora".
// 2) In the Now panel: the last lines the agent is writing (Edit/Write), with the cursor, updated live.
// Hooks in index.html (marked "modifiche live (12/11)"): in blocchi() the event of type "modifiche" calls
// window.modificheRiga; in disegnaAdesso the panel calls window.modificheLive; the <script src="live-edits.js"> tag.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const fileName = (p) => String(p || "").split("/").pop();
  const fileDir = (p) => { const x = String(p || "").split("/"); x.pop(); return x.slice(-2).join("/"); };

  const css = `
.mod{margin:2px 0 4px;max-width:760px}
.mod-riga{display:inline-flex;align-items:center;gap:9px;border:1px solid var(--line);border-radius:11px;padding:5px 10px 5px 6px;font-size:12.5px;background:var(--bg);color:var(--ink);cursor:pointer;font-family:inherit;transition:border-color .15s}
.mod-riga:hover{border-color:var(--ink3)}
.mod-ic{width:18px;height:18px;border-radius:5px;background:var(--ink);color:var(--bg);display:inline-grid;place-items:center;font-size:11px;font-weight:700;line-height:1}
.mod-riga b{font-weight:600}
.mod-conti{font-family:var(--mono);font-size:11.5px;display:inline-flex;gap:5px;font-variant-numeric:tabular-nums}
.mod-conti .p{color:var(--ink);font-weight:600}
.mod-conti .m{color:var(--ink3)}
.mod-barra{display:inline-flex;gap:2px}
.mod-barra i{width:6px;height:6px;border-radius:1.5px;background:var(--ink)}
.mod-barra i.m{background:var(--ink3);opacity:.55}
.mod-barra i.v{background:var(--fill2)}
.mod-finora{color:var(--ink3);font-size:11.5px}
.mod-chev{width:10px;height:10px;color:var(--ink3);transition:transform .18s}
.mod.aperta .mod-riga .mod-chev{transform:rotate(180deg)}
.mod-scheda{display:none;margin-top:8px;border:1px solid var(--line);border-radius:13px;background:var(--bg);overflow:hidden;animation:modApri .18s ease-out}
.mod.aperta .mod-scheda{display:block}
@keyframes modApri{from{opacity:0;transform:translateY(-3px)}}
.mod-testa{display:flex;align-items:center;gap:8px;padding:8px 10px 8px 12px;border-bottom:1px solid var(--line);font-size:12.5px}
.mod-testa .t{font-weight:600}
.mod-testa .az{margin-left:auto;display:flex;gap:6px}
.mod-btn{font:inherit;font-size:11.5px;border:1px solid var(--line);border-radius:7px;padding:3px 9px;background:var(--bg);color:var(--ink);cursor:pointer}
.mod-btn:hover{border-color:var(--ink3)}
.mod-btn.nero{background:var(--ink);color:var(--bg);border-color:var(--ink)}
.mod-btn[disabled]{opacity:.45;cursor:default}
.mod-f{border-bottom:1px solid var(--line)}
.mod-f:last-child{border-bottom:0}
.mod-fr{display:flex;align-items:center;gap:8px;padding:6px 10px 6px 8px;font-size:12.5px;cursor:pointer}
.mod-fr:hover{background:var(--fill)}
.mod-fr .chv{width:9px;height:9px;color:var(--ink3);transition:transform .15s;flex:none}
.mod-f.aperto .mod-fr .chv{transform:rotate(90deg)}
.mod-fr .n{font-weight:600;white-space:nowrap}
.mod-fr .d{color:var(--ink3);font-size:11.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0}
.mod-fr .tag{font-size:10px;font-weight:600;border-radius:5px;padding:1px 6px;background:var(--fill2);color:var(--ink2);white-space:nowrap}
.mod-fr .tag.nuovo{background:var(--ink);color:var(--bg)}
.mod-fr .dx{margin-left:auto;display:flex;align-items:center;gap:8px;flex:none}
.mod-fr .x{opacity:0;font:inherit;font-size:11px;border:0;background:none;color:var(--ink3);cursor:pointer;padding:2px 4px;border-radius:5px}
.mod-fr:hover .x{opacity:1}
.mod-fr .x:hover{color:var(--ink);background:var(--fill2)}
.mod-diff{display:none;font-family:var(--mono);font-size:11.5px;line-height:1.6;overflow-x:auto;max-height:420px;overflow-y:auto;border-top:1px solid var(--line)}
.mod-f.aperto .mod-diff{display:block}
.mod-r{display:grid;grid-template-columns:36px 36px 14px auto;white-space:pre;min-height:18px;width:max-content;min-width:100%}
.mod-r>span{padding:0 4px}
.mod-r .nn{color:var(--ink3);opacity:.7;text-align:right;user-select:none}
.mod-r .sg{color:var(--ink3);text-align:center;user-select:none}
.mod-r.piu{background:var(--fill);box-shadow:inset 3px 0 0 var(--ink)}
.mod-r.piu .sg{color:var(--ink);font-weight:700}
.mod-r.meno{color:var(--ink3);background:repeating-linear-gradient(135deg,var(--fill) 0 5px,transparent 5px 10px);box-shadow:inset 3px 0 0 var(--ink3)}
.mod-r.meno .tx{text-decoration:line-through;text-decoration-color:var(--fill2)}
.mod-sep{font-family:var(--sans);font-size:11px;color:var(--ink3);padding:3px 12px;background:var(--fill)}
.mod-nota{font-family:var(--sans);font-size:11.5px;color:var(--ink3);padding:8px 12px}
.mod-nota button{font:inherit;border:0;background:none;color:var(--ink2);text-decoration:underline;cursor:pointer;padding:0}
.mod-esito{font-size:11.5px;color:var(--ink2);padding:7px 12px;border-top:1px solid var(--line)}
.ml-live{margin-top:8px;border:1px solid var(--line);border-radius:9px;overflow:hidden;font-family:var(--mono);font-size:11px;line-height:1.6}
.adesso.compatto .ml-live{margin:6px 0 2px}
.ml-live .mod-r{grid-template-columns:34px 14px auto}
.ml-live .mod-r .tx{overflow:hidden;text-overflow:ellipsis}
.ml-cur{display:inline-block;width:6px;height:12px;background:var(--ink);vertical-align:-2px;margin-left:1px;animation:mlLamp 1s steps(1) infinite}
@keyframes mlLamp{50%{opacity:0}}
@media (prefers-reduced-motion:reduce){.ml-cur{animation:none}.mod-scheda{animation:none}}
`;
  const st = document.createElement("style"); st.id = "modifiche-css"; st.textContent = css; document.head.appendChild(st);

  const CHEV = '<svg class="mod-chev" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="m3 4.5 3 3 3-3"/></svg>';
  const CHV = '<svg class="chv" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="m4.5 3 3 3-3 3"/></svg>';

  // state that survives chat redraws
  const openKeys = new Set();      // "sid|da"
  const openFiles = new Map();      // "sid|da" -> Set(path) (opened by hand); the first time the first file is open
  const outcomes = new Map();       // "sid|da" -> text of the last Undo
  const armed = new Map();        // "sid|da|path" -> time of the first click on Undo
  const events = new Map();      // "sid|da" -> event (for Copy and Undo)

  function bar(plus, minus) {
    const total = plus + minus; if (!total) return "";
    const full = Math.min(5, Math.max(1, Math.round(Math.log10(total + 1) * 2)));
    const p = Math.round((plus / total) * full);
    return `<span class="mod-barra">${Array.from({ length: 5 }, (_, i) => `<i class="${i < p ? "" : i < full ? "m" : "v"}"></i>`).join("")}</span>`;
  }
  const counts = (plus, minus) => `<span class="mod-conti"><span class="p">+${plus}</span><span class="m">−${minus}</span></span>`;

  // fast chats (Nov 14): the chat arrives with only the summary (files, +/-) and diff lines are fetched when the panel opens
  const full = new Map();       // "sid|da" -> { firma, elenco } with the lines
  const eventSig = (e) => [e.a || "", e.n, e.piu, e.meno].join("|");
  function withHunks(k, e) {
    const p = full.get(k);
    if (e.leggero && p && p.firma === eventSig(e)) { e.elenco = p.elenco; e.leggero = false; }
    return e;
  }
  function fetchHunks(k, sid, e) {
    if (!e.leggero) return Promise.resolve(true);
    if (e._chiesti) return e._chiesti;
    e._chiesti = TT.core.invoke("modifiche_pezzi", { sessione: sid, da: e.da || "", a: e.a || "" }).catch((x) => ({ ok: false, msg: String(x) })).then((r) => {
      if (r?.ok) { full.set(k, { firma: eventSig(e), elenco: r.elenco || [] }); while (full.size > 30) full.delete(full.keys().next().value); e.elenco = r.elenco || []; e.leggero = false; }
      else e._errore = r?.msg || "diff non disponibile";
      rerender(sid);
      return !!r?.ok;
    });
    return e._chiesti;
  }
  function diffHtml(f, e) {
    if (f.leggero && !(f.pezzi || []).length) return e?._errore ? `<div class="mod-nota">Diff non caricato: ${E(e._errore)}. <button data-mod-path="${E(f.path)}">Apri il file</button></div>` : `<div class="mod-nota">Carico il diff…</div>`;   // fast chats (Nov 14)
    if (f.nascosto) return `<div class="mod-nota">File sensibile modificato (chiavi o password): contenuto e diff non si mostrano, né qui né sull'iPhone.</div>`;
    if (f.completo === false && !(f.pezzi || []).length) return `<div class="mod-nota">Cambiato con un comando: non si sa com'era prima, quindi niente diff. <button data-mod-path="${E(f.path)}">Apri il file</button></div>`;
    const pz = f.pezzi || [];
    if (!pz.length) return `<div class="mod-nota">${f.grande ? "File grande: diff non mostrato." : "Nessuna riga da mostrare."} <button data-mod-path="${E(f.path)}">Apri il file</button></div>`;
    let h = "";
    for (const p of pz) {
      let a = p.a || 1, b = p.b || 1;
      h += `<div class="mod-sep">riga ${b}</div>`;
      for (const r of p.righe || []) {
        const s = r[0], t = r.slice(1);
        if (s === "+") { h += `<div class="mod-r piu"><span class="nn"></span><span class="nn">${b}</span><span class="sg">+</span><span class="tx">${E(t)}</span></div>`; b++; }
        else if (s === "-") { h += `<div class="mod-r meno"><span class="nn">${a}</span><span class="nn"></span><span class="sg">−</span><span class="tx">${E(t)}</span></div>`; a++; }
        else { h += `<div class="mod-r"><span class="nn">${a}</span><span class="nn">${b}</span><span class="sg"></span><span class="tx">${E(t)}</span></div>`; a++; b++; }
      }
    }
    if (f.tagliato) h += `<div class="mod-nota">Diff lungo, mostrato in parte. <button data-mod-path="${E(f.path)}">Apri il file</button></div>`;
    return h;
  }

  /** Row under the answer + panel (closed until you open it). */
  window.modificheRiga = (e, d) => {
    const sid = d.sessione, k = sid + "|" + (e.da || e.t || "");
    withHunks(k, e);   // fast chats (Nov 14)
    events.set(k, e);
    const el = e.elenco || [];
    const latest = (d.eventi || []).filter((x) => x.tipo === "modifiche").pop() === e;
    const running = latest && d.vista === "lavora";
    const isOpen = openKeys.has(k);
    if (isOpen && e.leggero && !e._errore) setTimeout(() => fetchHunks(k, sid, e), 0);   // fast chats (Nov 14)
    if (!openFiles.has(k)) openFiles.set(k, new Set(el.length <= 3 && el[0] ? [el[0].path] : []));
    const fa = openFiles.get(k);
    const confirmed = (p) => { const t = armed.get(k + "|" + p); return t && Date.now() - t < 5000; };
    const undoable = (f) => !f.nascosto && f.completo !== false;
    const files = el.map((f) => {
      const created = f.stato === "creato", removed = f.stato === "tolto";
      return `<div class="mod-f${fa.has(f.path) ? " aperto" : ""}" data-mod-f="${E(f.path)}">
        <div class="mod-fr">${CHV}<span class="n">${E(fileName(f.path))}</span>${created ? '<span class="tag nuovo">nuovo</span>' : removed ? '<span class="tag">tolto</span>' : ""}${f.nascosto ? '<span class="tag">sensibile</span>' : ""}<span class="d">${E(fileDir(f.path))}</span>
        <span class="dx">${/* r1629: the Undo button stays in the row and is only hidden while the turn runs: when the turn ends one attribute changes instead of every row being rebuilt */ !undoable(f) ? "" : `<button class="x" data-mod-annulla="${E(f.path)}"${running ? " hidden" : ""}>${confirmed(f.path) ? "Conferma" : "Annulla"}</button>`}${f.nascosto ? "" : counts(f.piu || 0, f.meno || 0)}</span></div>
        <div class="mod-diff">${fa.has(f.path) ? diffHtml(f, e) : ""}</div></div>`;
    }).join("");
    const confirmAll = confirmed("");
    const az = `<button class="mod-btn" data-mod-copia>Copia diff</button>${running || !el.some(undoable) ? "" : `<button class="mod-btn${confirmAll ? " nero" : ""}" data-mod-annulla="">${confirmAll ? "Conferma: rimetti com'era" : "Annulla tutto"}</button>`}`;
    const es = outcomes.get(k);
    return `<div class="mod${isOpen ? " aperta" : ""}" data-mod="${E(k)}" data-mod-sid="${E(sid)}">
      <button class="mod-riga" data-mod-apri><span class="mod-ic">±</span><b>${e.n === 1 ? "1 file" : e.n + " file"}</b>${counts(e.piu || 0, e.meno || 0)}${bar(e.piu || 0, e.meno || 0)}${running ? '<span class="mod-finora">finora</span>' : ""}${CHEV}</button>
      <div class="mod-scheda"><div class="mod-testa"><span class="t">${running ? "Modifiche finora" : "Modifiche di questa risposta"}</span><span class="az">${az}</span></div>${el.some((f) => f.altre) ? '<div class="mod-nota" style="border-bottom:1px solid var(--line)">In questa cartella lavoravano anche altre chat: qualche modifica potrebbe essere loro.</div>' : ""}${files}${es ? `<div class="mod-esito">${E(es)}</div>` : ""}</div></div>`;
  };

  // ---------- clicks ----------
  const blk = (t) => t.closest("[data-mod]");
  function rerender(sid) { if (typeof ridisegnaFermo === "function") ridisegnaFermo(sid); }
  document.addEventListener("click", async (ev) => {
    const t = ev.target; if (!t.closest) return;
    const b = blk(t); if (!b) return;
    const k = b.dataset.mod, sid = b.dataset.modSid, e = events.get(k);
    const pa = t.closest("[data-mod-path]");
    if (pa) { ev.stopPropagation(); return TT.core.invoke("apri_file", { path: pa.dataset.modPath, finder: false }).catch(() => {}); }
    if (t.closest("[data-mod-apri]")) {
      openKeys.has(k) ? openKeys.delete(k) : openKeys.add(k);
      b.classList.toggle("aperta", openKeys.has(k));
      if (openKeys.has(k) && e?.leggero) { fetchHunks(k, sid, e); return; }   // fast chats (Nov 14): then redraws by itself
      const sc = b.querySelector(".mod-f.aperto .mod-diff"); if (sc && !sc.innerHTML.trim()) rerender(sid);
      return;
    }
    if (t.closest("[data-mod-copia]")) {
      if (!e) return;
      if (e.leggero && !(await fetchHunks(k, sid, e))) return notice("Diff non disponibile");   // fast chats (Nov 14)
      const txt = (e.elenco || []).filter((f) => !f.nascosto).map((f) => `--- a${f.path}\n+++ b${f.path}\n` + (f.pezzi || []).map((p) => {
        const r = p.righe || [], va = r.filter((x) => x[0] !== "+").length, vb = r.filter((x) => x[0] !== "-").length;
        return `@@ -${p.a},${va} +${p.b},${vb} @@\n` + r.join("\n");
      }).join("\n")).join("\n");
      if (typeof copia === "function") copia(txt, "Diff copiato"); else navigator.clipboard?.writeText(txt);
      return;
    }
    const an = t.closest("[data-mod-annulla]");
    if (an) {
      ev.stopPropagation();
      if (!e) return;
      const path = an.dataset.modAnnulla, ka = k + "|" + path, t0 = armed.get(ka);
      if (!t0 || Date.now() - t0 > 5000) { armed.set(ka, Date.now()); rerender(sid); setTimeout(() => { if (armed.get(ka) && Date.now() - armed.get(ka) >= 5000) { armed.delete(ka); rerender(sid); } }, 5100); return; }
      armed.delete(ka);
      let r; try { r = await TT.core.invoke("modifiche_annulla", { sessione: sid, da: e.da || "", a: e.a || "", path: path || null }); } catch (x) { r = { ok: false, msg: String(x) }; }
      outcomes.set(k, r?.msg || "Non riuscito");
      notice(r?.msg || "Non riuscito", 3200);
      rerender(sid);
      return;
    }
    const fr = t.closest(".mod-fr");
    if (fr) {
      const f = fr.closest("[data-mod-f]"), p = f.dataset.modF, s = openFiles.get(k) || new Set();
      s.has(p) ? s.delete(p) : s.add(p); openFiles.set(k, s);
      f.classList.toggle("aperto", s.has(p));
      const box = f.querySelector(".mod-diff");
      if (s.has(p) && box && !box.innerHTML.trim()) { const fe = (e?.elenco || []).find((x) => x.path === p); if (fe) box.innerHTML = diffHtml(fe, e); if (e?.leggero) fetchHunks(k, sid, e); }   // fast chats (Nov 14)
    }
  }, true);

  // ---------- live preview in the Now panel ----------
  const live = new Map();   // sid -> { coda, gen, nome, file }
  function liveHtml(sid, compact) {
    const l = live.get(sid); if (!l || !l.coda || !(l.coda.righe || []).length) return "";
    const rr = l.coda.righe.slice(compact ? -2 : -4);
    const all = l.coda.righe.length, da = l.coda.da != null ? l.coda.da + (all - rr.length) : null;
    return rr.map((r, i) => {
      const lastLines = i === rr.length - 1;
      return `<div class="mod-r piu"><span class="nn">${da != null ? da + i : ""}</span><span class="sg">+</span><span class="tx">${E(r)}${lastLines && l.gen ? '<span class="ml-cur"></span>' : ""}</span></div>`;
    }).join("");
  }
  /** Slot in the Now panel: fills and updates itself on every partial. */
  window.modificheLive = (sid, compact) => {
    const h = liveHtml(sid, compact);
    return h ? `<div class="ml-live" data-mlive="${E(sid)}" data-mlc="${compact ? 1 : 0}">${h}</div>` : "";
  };
  let ref = 0; const fromRef = new Set();
  function refresh() {
    ref = 0;
    for (const sid of fromRef) {
      const h = liveHtml(sid, false);
      document.querySelectorAll(`[data-mlive="${CSS.escape(sid)}"]`).forEach((el) => {
        const x = liveHtml(sid, el.dataset.mlc === "1"); if (el.innerHTML !== x) el.innerHTML = x;
        el.hidden = !x;
      });
      // the panel did not have the slot yet (first line arrived): redraw the panel once
      if (h && !document.querySelector(`[data-mlive="${CSS.escape(sid)}"]`) && typeof visteDi === "function" && typeof disegnaAdesso === "function") {
        for (const V of visteDi(sid)) if (V.det && V.det.vista === "lavora") disegnaAdesso(V, V.det);
      }
    }
    fromRef.clear();
  }
  TT.event.listen("parziale", (ev) => {
    const p = ev.payload || {}; if (!p.id || String(p.id).startsWith("locale:")) return;
    if ("strumento" in p) {
      if (p.coda) live.set(p.id, { coda: p.coda, gen: !!p.gen, nome: p.strumento, file: p.file });
      else if (p.strumento && !["Edit", "Write", "MultiEdit"].includes(p.strumento)) live.delete(p.id);
      else if (live.has(p.id)) live.get(p.id).gen = !!p.gen;
    } else if (p.testo) live.delete(p.id);   // the agent is writing the answer: the preview is done
    fromRef.add(p.id);
    if (!ref) ref = requestAnimationFrame(refresh);
  });
})();
