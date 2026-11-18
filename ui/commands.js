// commands (Nov 13): "/" menu in the chat input box (src-tauri/src/commands.rs).
// Type "/" and the list appears: Juggl commands, Claude Code commands that work here, skills and commands (yours, the project's,
// plugins'). Filter by typing, arrows to choose, Enter to use, Tab to complete, Esc to close.
// When sending a text that starts with "/" it asks Juggl first (comando_esegui): Juggl commands are run by Juggl, those
// that do not exist here say what to use, unknown ones never start. Only "passa" goes to the chat, the normal way.
// Hooks in index.html (marked "comandi (13/11)"): only the <script src="commands.js"> tag after claude-login.js.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const RX_CMD = /^\/([A-Za-z0-9:_?.-]*)$/;              // "/" + name, no spaces: menu with filter
  const RX_ARG = /^\/([A-Za-z0-9:_?.-]+)\s+([^\n]*)$/;    // "/name arguments": row with the argument help
  const RX_INVIO = /^\s*\/([A-Za-z0-9:_-]+|\?)(\s|$)/;    // text to check before sending (not /Users/... paths)

  const css = `
.cm-menu{position:fixed;z-index:70;background:var(--bg);border:1px solid var(--line);border-radius:14px;box-shadow:0 14px 40px rgba(0,0,0,.18);overflow:hidden;display:flex;flex-direction:column;animation:cmSale .14s ease-out}
@keyframes cmSale{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}
.cm-lista{overflow-y:auto;max-height:min(340px,45vh);padding:4px;overscroll-behavior:contain}
.cm-gr{font-size:10.5px;font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:var(--ink3);padding:8px 10px 3px}
.cm-v{display:flex;align-items:baseline;gap:10px;padding:6px 10px;border-radius:9px;cursor:default;min-width:0}
.cm-v.on{background:var(--fill)}
.cm-v b{font:600 13px var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);color:var(--ink);flex:none;white-space:nowrap}
.cm-v b mark{background:none;color:inherit;text-decoration:underline;text-decoration-color:var(--ink3);text-underline-offset:2px}
.cm-v .cm-arg{font:12px var(--mono, ui-monospace, Menlo, monospace);color:var(--ink3);flex:none;white-space:nowrap;max-width:40%;overflow:hidden;text-overflow:ellipsis}
.cm-v .cm-d{flex:1;min-width:0;color:var(--ink2);font-size:12.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.cm-v .cm-al{flex:none;color:var(--ink3);font-size:11px;white-space:nowrap}
.cm-v .cm-fonte{flex:none;font-size:10px;font-weight:600;letter-spacing:.03em;text-transform:uppercase;color:var(--ink3);border:1px solid var(--line);border-radius:5px;padding:0 4px}
.cm-piede{display:flex;gap:12px;align-items:center;justify-content:space-between;border-top:1px solid var(--line);padding:6px 12px;font-size:11px;color:var(--ink3)}
.cm-piede kbd{font:10.5px var(--sans);border:1px solid var(--line);border-radius:4px;padding:0 4px;margin:0 2px;color:var(--ink2)}
.cm-vuoto{padding:12px;color:var(--ink3);font-size:12.5px}
.cm-arg-riga{padding:8px 12px;display:flex;gap:10px;align-items:baseline;font-size:12.5px;color:var(--ink2)}
.cm-arg-riga b{font:600 13px var(--mono, ui-monospace, Menlo, monospace);color:var(--ink)}
.cm-arg-riga code{font:12px var(--mono, ui-monospace, Menlo, monospace);color:var(--ink3)}
.cm-nota{position:fixed;z-index:70;background:var(--bg);border:1px solid var(--line);border-radius:14px;box-shadow:0 14px 40px rgba(0,0,0,.18);padding:11px 14px 10px;animation:cmSale .16s ease-out}
.cm-nota b{font:600 12.5px var(--mono, ui-monospace, Menlo, monospace)}
.cm-nota p{margin:4px 0 0;font-size:12.5px;line-height:1.45;color:var(--ink2);white-space:pre-line}
.cm-nota .cm-az{display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap;margin-top:9px}
.cm-nota .cm-az .sx{margin-right:auto;display:flex;gap:6px;flex-wrap:wrap}
.cm-nota .cm-sim{font:12px var(--mono, ui-monospace, Menlo, monospace);border:1px solid var(--line);border-radius:7px;padding:2px 8px;background:none;color:var(--ink);cursor:pointer}
.cm-nota .cm-sim:hover{background:var(--fill)}
`;
  const st = document.createElement("style"); st.id = "comandi-css"; st.textContent = css; document.head.appendChild(st);

  // ---------- data ----------
  const CACHE = {};   // chat id -> {voci, caricamento, nota, t}
  async function loadList(id, force) {
    const c = CACHE[id];
    if (c && !force && !c.caricamento && Date.now() - c.t < 60000) return c;
    try { const r = await inv("comandi_elenco", { id }); CACHE[id] = { ...r, t: Date.now() }; } catch (e) { console.error(e); }
    return CACHE[id] || { voci: [], caricamento: true };
  }

  // ---------- menu ----------
  let M = null;   // {ta, V, id, voci, filtro, i, el, tRiprova, prove}
  const viewOf = (ta) => (typeof vistaDi === "function" ? vistaDi(ta) : null) || (typeof VA === "function" ? VA() : null);
  const idOf = (V) => V?.det?.sessione || V?.id || "";

  function score(v, q) {
    if (!q) return 1;
    const names = [v.nome, ...(v.alias || [])].map((x) => x.toLowerCase());
    let best = 0;
    for (const [i, n] of names.entries()) {
      const p = i === 0 ? 0 : 1;   // the real name before the alias
      if (n === q) best = Math.max(best, 100 - p);
      else if (n.startsWith(q)) best = Math.max(best, 80 - p - Math.min(n.length - q.length, 20) / 10);
      else if (n.split(/[:\-_]/).some((x) => x.startsWith(q))) best = Math.max(best, 60 - p);
      else if (n.includes(q)) best = Math.max(best, 40 - p);
    }
    if (!best && q.length >= 4 && (v.desc || "").toLowerCase().includes(q)) best = 10;
    return best;
  }
  function filterItems(voci, q) {
    q = (q || "").toLowerCase();
    if (!q) return voci;
    return voci.map((v) => [v, score(v, q)]).filter((x) => x[1] > 0).sort((a, b) => b[1] - a[1]).map((x) => x[0]);
  }
  /** Name to show: if you typed an alias (/login), that one. */
  function shownName(v, q) {
    q = (q || "").toLowerCase();
    if (!q || v.nome.toLowerCase().startsWith(q)) return v.nome;
    return (v.alias || []).find((a) => a.toLowerCase().startsWith(q)) || v.nome;
  }
  function highlight(n, q) {
    if (!q) return E(n);
    const i = n.toLowerCase().indexOf(q.toLowerCase());
    return i < 0 ? E(n) : E(n.slice(0, i)) + "<mark>" + E(n.slice(i, i + q.length)) + "</mark>" + E(n.slice(i + q.length));
  }
  const SOURCES = { progetto: "progetto", plugin: "plugin", "claude.ai": "claude.ai" };

  function placeMenu(el, ta) {
    const f = ta.closest("form") || ta;
    const r = f.getBoundingClientRect();
    el.style.left = Math.max(8, r.left) + "px";
    el.style.width = Math.min(r.width, innerWidth - 16) + "px";
    el.style.bottom = Math.max(8, innerHeight - r.top + 6) + "px";
  }

  function closeMenu() {
    if (!M) return;
    clearTimeout(M.tRiprova); M.el?.remove(); M = null;
  }

  function render() {
    if (!M) return;
    if (!M.el) { M.el = document.createElement("div"); M.el.className = "cm-menu"; M.el.setAttribute("role", "listbox"); document.body.appendChild(M.el); }
    const q = M.filtro;
    let html = "";
    if (M.arg) {
      const v = M.arg;
      html = `<div class="cm-arg-riga"><b>/${E(v.visto)}</b>${v.arg ? `<code>${E(v.arg)}</code>` : ""}<span>${E(v.desc || "")}</span></div>`;
    } else {
      const voci = (M.lista = filterItems(M.voci, q));
      M.i = Math.max(0, Math.min(M.i, voci.length - 1));
      if (!voci.length) html = `<div class="cm-vuoto">${M.caricamento ? "Carico i comandi di questa cartella…" : `Nessun comando «/${E(q)}». Invio lo controlla e ti dice cosa usare.`}</div>`;
      else {
        let group = "";
        html = voci.slice(0, 120).map((v, i) => {
          const g = q ? "" : v.gruppo;
          const groupHead = g && g !== group ? `<div class="cm-gr">${E((group = g))}</div>` : "";
          const seen = shownName(v, q);
          const al = seen === v.nome ? (v.alias || []).filter((a) => /^[a-z][a-z-]{0,13}$/.test(a)).slice(0, 2).map((a) => "/" + a).join(" ") : "Juggl: /" + v.nome;
          return `${groupHead}<div class="cm-v ${i === M.i ? "on" : ""}" data-cm="${i}" role="option"><b>/${highlight(seen, q)}</b>${v.arg ? `<span class="cm-arg">${E(v.arg)}</span>` : ""}<span class="cm-d">${E(v.desc || "")}</span>${al ? `<span class="cm-al">${E(al)}</span>` : ""}${SOURCES[v.fonte] ? `<span class="cm-fonte">${E(SOURCES[v.fonte])}</span>` : ""}</div>`;
        }).join("");
        if (M.caricamento) html += `<div class="cm-vuoto">Carico skill e comandi di questa cartella…</div>`;
      }
      if (M.nota) html = `<div class="cm-vuoto">${E(M.nota)}</div>` + html;
    }
    M.el.innerHTML = `<div class="cm-lista">${html}</div>${M.arg ? "" : `<div class="cm-piede"><span><kbd>↑↓</kbd> scegli <kbd>↩</kbd> usa <kbd>⇥</kbd> completa <kbd>Esc</kbd> chiudi</span><span>${M.terminale ? "Chat nel terminale: i comandi di Claude li fa lui" : ""}</span></div>`}`;
    placeMenu(M.el, M.ta);
    M.el.querySelector(".cm-v.on")?.scrollIntoView({ block: "nearest" });
  }

  async function open(ta) {
    const V = viewOf(ta), id = idOf(V);
    if (!id) return;
    if (!M || M.ta !== ta) { closeMenu(); M = { ta, V, id, voci: [], filtro: "", i: 0, prove: 0 }; }
    updateFilter();
    const r = await loadList(id);
    if (!M || M.ta !== ta) return;
    M.voci = r.voci || []; M.caricamento = !!r.caricamento; M.nota = r.nota || ""; M.terminale = !!r.terminale;
    updateFilter();
    if (M.caricamento && M.prove < 12) { clearTimeout(M.tRiprova); M.tRiprova = setTimeout(async () => { if (!M) return; M.prove++; await loadList(id, true); if (M) open(M.ta); }, 2500); }
  }

  function updateFilter() {
    if (!M) return;
    const t = M.ta.value;
    let m = RX_CMD.exec(t);
    if (m) { M.arg = null; if (M.filtro !== m[1]) { M.filtro = m[1]; M.i = 0; } return render(); }
    m = RX_ARG.exec(t);
    if (m) {
      const n = m[1].toLowerCase();
      const v = M.voci.find((x) => x.nome.toLowerCase() === n || (x.alias || []).some((a) => a.toLowerCase() === n));
      if (v && (v.arg || v.desc)) { M.arg = { ...v, visto: m[1] }; return render(); }
    }
    closeMenu();
  }

  function pick(i, completeIt) {
    if (!M) return;
    const v = (M.lista || [])[i]; if (!v) return;
    const ta = M.ta, label = shownName(v, M.filtro);
    // with an argument (or Tab) it completes and you keep typing; without, it starts right away
    if (completeIt || /^</.test(v.arg || "")) {   // required argument (<...>): complete it; optional ([...]): start
      ta.value = "/" + label + " "; ta.dispatchEvent(new Event("input", { bubbles: true }));
      ta.focus(); ta.setSelectionRange(ta.value.length, ta.value.length);
      return;
    }
    ta.value = "/" + label; ta.dispatchEvent(new Event("input", { bubbles: true }));
    closeMenu();
    ta.form?.requestSubmit();
  }

  // ---------- input box events ----------
  const isInputBox = (t) => t?.classList?.contains("r-risposta");
  document.addEventListener("input", (e) => {
    if (!isInputBox(e.target)) return;
    closeNote();
    const t = e.target.value;
    if (t.startsWith("/") && !t.includes("\n")) { if (!M || M.ta !== e.target) open(e.target); else updateFilter(); }
    else closeMenu();
  });
  document.addEventListener("keydown", (e) => {
    if (!M || e.target !== M.ta || e.isComposing) return;
    if (M.arg) { if (e.key === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); closeMenu(); } return; }
    const n = (M.lista || []).length;
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || (e.ctrlKey && (e.key === "n" || e.key === "p"))) {
      if (!n) return;
      e.preventDefault(); e.stopImmediatePropagation();
      const down = e.key === "ArrowDown" || e.key === "n";
      M.i = (M.i + (down ? 1 : -1) + n) % n; render();
    } else if (e.key === "Tab" && n) {
      e.preventDefault(); e.stopImmediatePropagation(); pick(M.i, true);
    } else if (e.key === "Enter" && !e.shiftKey && !e.altKey && n && RX_CMD.test(M.ta.value)) {
      e.preventDefault(); e.stopImmediatePropagation(); pick(M.i, false);
    } else if (e.key === "Escape") {
      e.preventDefault(); e.stopImmediatePropagation(); closeMenu();
    }
  }, true);
  document.addEventListener("pointerdown", (e) => {
    const r = e.target.closest?.("[data-cm]");
    if (r && M) { e.preventDefault(); M.i = +r.dataset.cm; return pick(M.i, false); }
    if (M && !e.target.closest?.(".cm-menu") && e.target !== M.ta) closeMenu();
  });
  document.addEventListener("pointermove", (e) => {
    const r = e.target.closest?.("[data-cm]");
    if (r && M && +r.dataset.cm !== M.i) { M.i = +r.dataset.cm; M.el.querySelectorAll(".cm-v").forEach((x) => x.classList.toggle("on", +x.dataset.cm === M.i)); }
  });
  document.addEventListener("focusout", (e) => { if (M && e.target === M.ta) setTimeout(() => { if (M && document.activeElement !== M.ta) closeMenu(); }, 120); });
  addEventListener("resize", () => { if (M?.el) placeMenu(M.el, M.ta); });
  // the chat just opened: list ready before the first "/"
  document.addEventListener("focusin", (e) => { if (isInputBox(e.target)) { const id = idOf(viewOf(e.target)); if (id && !CACHE[id]) loadList(id); } });

  // ---------- send: Juggl first, then (only if "passa") the chat ----------
  const letThrough = new WeakSet();
  document.addEventListener("submit", async (e) => {
    const f = e.target, ta = f?.querySelector?.(".r-risposta");
    if (!ta || letThrough.has(f)) { letThrough.delete(f); return; }
    const textVal = ta.value;
    if (!RX_INVIO.test(textVal)) return;
    const V = viewOf(ta), id = idOf(V);
    if (!id) return;
    e.preventDefault(); e.stopImmediatePropagation();
    closeMenu();
    let r;
    try { r = await inv("comando_esegui", { id, testo: textVal.trim() }); } catch (err) { r = { esito: "passa" }; }
    if (r.esito === "passa") {
      // goes out as a normal message (skills, Claude Code commands that work here)
      letThrough.add(f); f.requestSubmit(); return;
    }
    // unknown command: the text stays in the box, to be fixed; the others are done and the box is cleared
    if (r.azione !== "sconosciuto") {
      ta.value = ""; ta.dispatchEvent(new Event("input", { bubbles: true }));
      if (typeof alza === "function") alza(ta);
      if (typeof aggiornaInvia === "function") aggiornaInvia(V);
    }
    execute(r, V, ta);
  }, true);

  // ---------- actions ----------
  let N = null;
  function closeNote() { if (N) { clearTimeout(N._t); N.remove(); N = null; } }
  function note(r, ta, buttons = "") {
    closeNote();
    if (!r.msg && !buttons) return;
    N = document.createElement("div"); N.className = "cm-nota";
    const sim = (r.simili || []).map((x) => `<button class="cm-sim" data-cm-sim="${E(x)}">${E(x)}</button>`).join("");
    N.innerHTML = `<b>${E(r.comando || "")}</b><p>${E(r.msg || "")}</p><div class="cm-az"><span class="sx">${sim}</span>${buttons}<button class="tasto" data-cm-nota="chiudi">Ho capito</button></div>`;
    document.body.appendChild(N);
    N._ta = ta;
    if (ta) placeMenu(N, ta); else { N.style.left = "50%"; N.style.transform = "translateX(-50%)"; N.style.bottom = "92px"; N.style.width = "min(460px, calc(100vw - 32px))"; }
    N._t = setTimeout(closeNote, buttons || sim ? 30000 : 12000);
  }
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-cm-nota],[data-cm-sim]"); if (!b || !N) return;
    const ta = N._ta, a = b.dataset.cmNota, sim = b.dataset.cmSim;
    if (sim && ta) { closeNote(); ta.value = sim + " "; ta.focus(); ta.dispatchEvent(new Event("input", { bubbles: true })); return; }
    const fn = N._azioni?.[a]; closeNote();
    if (fn) await fn();
  });
  document.addEventListener("keydown", (e) => { if (N && e.key === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); closeNote(); } }, true);
  function noteWithButtons(r, ta, buttons) {
    note(r, ta, buttons.map(([k, t, dark]) => `<button class="${dark ? "btn-nero" : "tasto"}" data-cm-nota="${k}">${E(t)}</button>`).join(""));
    if (N) N._azioni = Object.fromEntries(buttons.map(([k, , , f]) => [k, f]));
  }
  const reload = (V) => { try { caricaElenco(); } catch {} try { if (V && V === VA()) caricaDettaglio(false); } catch {} };
  const signOutAccount = () => { if (window.accessoEsci) return window.accessoEsci(); const b = document.createElement("button"); b.dataset.ac = "esci"; b.hidden = true; document.body.appendChild(b); b.click(); b.remove(); };

  async function execute(r, V, ta) {
    const id = idOf(V);
    switch (r.azione) {
      case "fatto": notice(r.msg || "Fatto", 3000); if (r.aggiorna) reload(V); return;
      case "nuova-chat":
        if (r.msg) notice(r.msg, 3600);
        await (window.apriNuova || apriNuova)();
        if (r.arg) { const t = document.getElementById("nuova-testo"); if (t) { t.value = r.arg; t.focus(); } }
        return;
      case "scegli-modello": {
        const b = V?.el?.querySelector(".r-regola") || document.querySelector(".st-mod");
        if (b && typeof apriRegolazioni === "function") { note(r, ta); return apriRegolazioni(b); }
        return note(r, ta);
      }
      case "consumi": {
        const b = document.querySelector(".piede-quota");
        if (b && typeof apriQuota === "function") return apriQuota(b);
        if (typeof apriConsumi === "function") return apriConsumi();
        return note(r, ta);
      }
      case "login": if (window.accessoAvvia) return window.accessoAvvia(); return note(r, ta);
      case "logout": return signOutAccount();
      case "rinomina": if (typeof rinominaInizio === "function") return rinominaInizio(); return note(r, ta);
      case "sposta":
        if (typeof apriSpostaBozza === "function") { if (ta && r.arg) ta.value = r.arg; return apriSpostaBozza(V); }
        return note(r, ta);
      case "riprendi": if (typeof apriSalto === "function") return apriSalto({ q: r.arg || "" }); return note(r, ta);
      case "archivia":
        return noteWithButtons({ ...r, msg: r.msg }, ta, [["archivia", "Archivia", true, () => archiviaSess(id)]]);
      case "cerca":
        if (typeof apriTrova === "function" && apriTrova(V) !== false) {
          if (r.arg) setTimeout(() => { const i = V.el?.querySelector(":scope > .trova input"); if (i) { i.value = r.arg; i.dispatchEvent(new Event("input", { bubbles: true })); } }, 30);
          return;
        }
        return note(r, ta);
      case "richieste": if (typeof apriRichieste === "function") return apriRichieste(); return note(r, ta);
      // settings as chat (Nov 16): "/impostazioni spegni gli avvisi di notte" opens Settings with the sentence already sent
      // width (Nov 16): /larghezza [stretta|normale|larga], with no value the next one
      case "larghezza": { const v = window.tipografia?.larghezza?.(r.arg || ""); if (!v) return note(r, ta); if (ta) ta.value = ""; return; }
      case "permessi": case "impostazioni": if (r.azione === "impostazioni" && r.arg && window.impostazioniChat) return window.impostazioniChat(r.arg); TT.event.emit("impostazioni"); return;
      case "memoria":
        return noteWithButtons(r, ta, (r.file || []).map((p, i) => [`m${i}`, p.includes("/.claude/CLAUDE.md") ? "Apri quella generale" : "Apri quella del progetto", i === 0, () => inv("apri_file", { path: p, finder: false })]));
      case "aiuto":
        if (ta) { ta.value = "/"; ta.focus(); ta.dispatchEvent(new Event("input", { bubbles: true })); }
        return;
      case "cambia-account": return noteWithButtons(r, ta, [["avvia", "Cambia account Claude", true, () => window.accessoAvvia?.()]]);
      default: return note(r, ta);
    }
  }

  // the old path (manda -> invia -> "gestito", "comando-non-disponibile" event) goes through here
  const legacy = window.accessoComando;
  window.accessoComando = (r) => {
    const V = typeof VA === "function" ? VA() : null;
    const ta = V && typeof qv === "function" ? qv(V, "risposta") : null;
    // manda() path: the bubble is already removed, the text of an unknown command goes back to the box (if empty)
    if (r?.azione === "sconosciuto" && r.testo && ta && !ta.value.trim()) { ta.value = r.testo; ta.dispatchEvent(new Event("input", { bubbles: true })); if (typeof alza === "function") alza(ta); }
    if (r && r.azione && r.azione !== "") return execute(r, V, ta);
    if (legacy) return legacy(r);
    note(r, null);
  };
  window.comandiApri = (ta) => open(ta);
})();
