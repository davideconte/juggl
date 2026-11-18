// Automatic coordinator for each chat group (Nov 12, src-tauri/src/coordinator.rs).
// Under the group header: the summary row ("2 aspettano te: ...", each entry opens its chat) and, with the group
// open, the "Scrivi alla regia" box: the text goes to the coordinator, which passes it to the right chat. The coordinator sits at the top
// of the group's chats, with the "regia" label. Right click on the group: open the coordinator, write to it, turn the automatic
// coordinator on or off. Settings: one switch per group (impostazioni.json "regia_gruppi").
// Data: dati.regie (per group: nome, regia, accesa, n, aspettano, sintesi), the same as the iPhone in /api/tavolo.
// Hooks in index.html (marked "regia automatica (12/11)"): gruppiDi puts the coordinator on top, disegnaElenco calls
// window.regiaVoci, and the <script src="coordinator.js"> tag at the bottom.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const D = () => { try { return dati || {}; } catch { return {}; } };
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const coordinators = () => D().regie || {};
  const isOn = (pid) => (D().imp?.regia_gruppi || {})[pid] !== false;
  const rerender = () => { if (typeof caricaElenco === "function") caricaElenco(); else if (typeof disegnaElenco === "function") disegnaElenco(); };

  const css = `
.gr-sintesi{display:flex;align-items:baseline;gap:7px;margin:-1px 0 4px;padding:3px 10px 5px 57px;font-size:12px;line-height:1.4;color:var(--ink2)}
.gr-sintesi .gs-punto{flex:none;width:6px;height:6px;border-radius:50%;background:#ff5b1f;transform:translateY(-1px)}
.gr-sintesi b{font-weight:600;color:var(--ink)}
.gr-sintesi .gs-v{color:var(--ink2);text-decoration:none;border-bottom:1px solid var(--line);cursor:pointer}
.gr-sintesi .gs-v:hover{color:var(--ink);border-color:var(--ink3)}
.gr-scrivi{display:flex;gap:6px;align-items:center;margin:2px 0 6px;padding:0 8px 0 33px}
.gr-scrivi input{flex:1;min-width:0;height:30px;border-radius:9px;border:1px solid var(--line);background:var(--fill);color:inherit;font:inherit;font-size:12.5px;padding:0 10px;outline:none;-webkit-user-select:text;user-select:text}
.gr-scrivi input:focus{background:var(--bg);border-color:var(--ink3)}
.gr-scrivi button{flex:none;width:28px;height:28px;border-radius:8px;border:0;background:var(--ink);color:var(--bg);display:flex;align-items:center;justify-content:center;opacity:.9}
.gr-scrivi button:hover{opacity:1}
.gr-scrivi button svg{width:12px;height:12px;stroke:currentColor}
.riga-regia .nome::after{content:"regia";margin-left:6px;font-size:10px;font-weight:600;letter-spacing:.03em;text-transform:uppercase;color:var(--ink3);border:1px solid var(--line);border-radius:5px;padding:0 4px;vertical-align:1px}
#imp-regia .imp-riga small{display:block}
`;
  const st = document.createElement("style"); st.id = "regia-css"; st.textContent = css; document.head.appendChild(st);
  // "regia" label on coordinator rows: the class is added after every list redraw
  const ids = () => new Set(Object.values(coordinators()).map((r) => r.regia).filter(Boolean));
  function mark() {
    const all = ids();
    // inside the group the short name is already "regia": the label is only needed when the name does not say it
    document.querySelectorAll(".elenco .riga[data-id]").forEach((el) => el.classList.toggle("riga-regia", all.has(el.dataset.id) && !/(^|\s)regia$/i.test(el.querySelector(".nome")?.textContent.trim() || "")));
  }
  new MutationObserver(() => { clearTimeout(mark._t); mark._t = setTimeout(mark, 0); })
    .observe(document.getElementById("elenco") || document.body, { childList: true, subtree: true });

  const ARROW = '<svg viewBox="0 0 12 12" fill="none" stroke-width="1.8" stroke-linecap="round"><path d="M6 10V2M2.5 5.5 6 2l3.5 3.5"/></svg>';
  /** Extra entries under the group header (called by disegnaElenco): summary and box for the coordinator. */
  window.regiaVoci = (gr) => {
    const R = coordinators()[gr.pid];
    if (!R) return [];
    const out = [];
    const l = R.aspettano || [];
    if (l.length && !window.EL) {   // list (Nov 14): with the new sidebar whoever waits for you is already on top
      const voci = l.slice(0, 3).map((x) => `<a class="gs-v" data-jvai="${E(x.id)}" title="Apri ${E(x.nome)}">${E(x.cosa)}</a>`).join(", ");
      const others = l.length > 3 ? ` e altre ${l.length - 3}` : "";
      out.push({ k: "gs:" + gr.pid, h: `<div class="gr-sintesi" data-gr-sintesi="${E(gr.pid)}" title="Chi del gruppo aspetta te"><span class="gs-punto"></span><span><b>${l.length} ${l.length === 1 ? "aspetta" : "aspettano"} te:</b> ${voci}${others}</span></div>` });
    }
    if (R.regia && gr.aperto && isOn(gr.pid) && (!window.EL || window.EL.scrivi.has(gr.pid))) {   // list (Nov 14): box only from the icon
      out.push({ k: "gw:" + gr.pid, h: `<form class="gr-scrivi" data-gr-scrivi="${E(gr.pid)}" autocomplete="off"><input name="t" placeholder="Scrivi alla regia di ${E(R.nome || "questo gruppo")}…" spellcheck="false"><button type="submit" title="Manda alla regia (Invio): la passa alla chat giusta">${ARROW}</button></form>` });
    }
    return out;
  };

  // "Scrivi alla regia" box: it is your own message (like in the chat), so it also counts as a request
  document.addEventListener("submit", async (e) => {
    const f = e.target.closest?.("[data-gr-scrivi]"); if (!f) return;
    e.preventDefault();
    const inp = f.querySelector("input"), t = inp.value.trim(), R = coordinators()[f.dataset.grScrivi];
    if (!t || !R?.regia) return;
    inp.disabled = true;
    const r = await inv("invia", { id: R.regia, testo: t }).catch((x) => ({ ok: false, msg: String(x) }));
    inp.disabled = false;
    if (r?.ok) { inp.value = ""; notice(r.esito === "in_coda" ? "In coda per la regia: parte appena è libera" : "Mandato alla regia di " + (R.nome || "questo gruppo"), 2200); }
    else notice("Non mandato: " + (r?.msg || "errore"), 2600);
  });
  // Esc in the box: leaves the box
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && e.target.closest?.(".gr-scrivi")) e.target.blur(); });

  function openGroup(pid) {
    try { gruppiScelti.set(pid, true); localStorage.setItem("gruppiscelti", JSON.stringify(Object.fromEntries(gruppiScelti))); } catch {}
    if (typeof disegnaElenco === "function") disegnaElenco();
  }
  async function apply(pid, on) {
    const r = await inv("regia_imposta", { gruppo: pid, accesa: on }).catch((x) => ({ ok: false, msg: String(x) }));
    if (!r?.ok) return notice("Non riuscito: " + (r?.msg || ""), 2400);
    const d = D(); d.imp = d.imp || {}; d.imp.regia_gruppi = { ...(d.imp.regia_gruppi || {}) };
    if (on) delete d.imp.regia_gruppi[pid]; else d.imp.regia_gruppi[pid] = false;
    notice(on ? "Regia automatica accesa: nasce con 3 chat aperte" : "Regia automatica spenta per questo gruppo", 2400);
    renderSettings(); rerender();
  }

  // right click on the group header (or on its summary row)
  document.addEventListener("contextmenu", (e) => {
    const g = e.target.closest?.(".riga-gruppo,[data-gr-sintesi]"); if (!g) return;
    const pid = g.dataset.gruppo || g.dataset.grSintesi, R = coordinators()[pid], on = isOn(pid);
    e.preventDefault();
    const voci = [];
    if (R?.regia) voci.push(["apri", "Apri la regia"], ["scrivi", "Scrivi alla regia…"], ["-"]);
    voci.push(["interr", on ? "Spegni la regia automatica" : "Accendi la regia automatica"]);
    const m = document.getElementById("menu"); if (!m) return;
    m.innerHTML = voci.map(([a, t]) => a === "-" ? "<hr>" : `<button data-regia-az="${a}" data-pid="${E(pid)}">${E(t)}</button>`).join("");
    m.classList.add("on");
    const r = m.getBoundingClientRect();
    m.style.left = Math.min(e.clientX, innerWidth - r.width - 8) + "px";
    m.style.top = Math.min(e.clientY, innerHeight - r.height - 8) + "px";
  });
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-regia-az]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    document.getElementById("menu")?.classList.remove("on");
    const pid = b.dataset.pid, R = coordinators()[pid], a = b.dataset.regiaAz;
    if (a === "apri" && R?.regia) return typeof scegli === "function" && scegli(R.regia);
    if (a === "scrivi") { window.EL?.scrivi.add(pid); openGroup(pid);   // list (Nov 14)
      return setTimeout(() => document.querySelector(`[data-gr-scrivi="${CSS.escape(pid)}"] input`)?.focus(), 60); }
    if (a === "interr") return apply(pid, !isOn(pid));
  }, true);

  // ---------- Settings: "Regia dei gruppi", one row per group ----------
  function groupSettings() {
    const d = D(), m = new Map();
    for (const [pid, R] of Object.entries(coordinators())) m.set(pid, { pid, nome: R.nome || pid, R });
    for (const pid of Object.keys(d.imp?.regia_gruppi || {})) if (!m.has(pid)) {
      const s = (d.sessioni || []).find((x) => x.progetto?.id === pid);
      m.set(pid, { pid, nome: s?.progetto?.nome || pid, R: null });
    }
    return [...m.values()].sort((a, b) => a.nome.localeCompare(b.nome, "it"));
  }
  function settingsHtml() {
    const gg = groupSettings();
    const rowsHtml = gg.map(({ pid, nome, R }) => {
      const on = isOn(pid);
      const sub = !on ? "Spenta: nessuna regia, nessun controllo sulle cartelle" : R?.regia ? `Regia attiva · ${R.n} ${R.n === 1 ? "chat" : "chat"} nel gruppo` : `Nasce da sola con 3 chat aperte (adesso ${R?.n ?? 0})`;
      return `<div class="imp-riga"><div class="tx"><b>${E(nome)}</b><small>${E(sub)}</small></div><button class="interr ${on ? "on" : ""}" data-regia-imp="${E(pid)}" role="switch" aria-checked="${on}" title="${on ? "Spegni" : "Accendi"}"></button></div>`;
    }).join("");
    return `<h4>Regia dei gruppi</h4>${rowsHtml || `<div class="imp-riga"><div class="tx"><b>Nessun gruppo adesso</b><small>Compare qui un progetto con almeno 3 chat aperte</small></div></div>`}
      <p class="imp-nota">Con 3 o più chat aperte nello stesso progetto, Juggl apre una chat di regia: ti dice chi aspetta te, passa i tuoi messaggi alla chat giusta e ferma due chat che vogliono gli stessi file. Si sveglia solo quando succede qualcosa, con il modello piccolo. Si spegne anche dal tasto destro sul gruppo.</p>`;
  }
  function renderSettings() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-regia");
    const h = settingsHtml();
    if (!sec) {
      const footer = f.querySelector(".piedef"); if (!footer) return;
      sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-regia";
      footer.before(sec);
    }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (!document.getElementById("imp-regia")) renderSettings(); }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-regia-imp]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    apply(b.dataset.regiaImp, !b.classList.contains("on"));
  }, true);
  // the first list redraw may come before this file loads: redo it once with the coordinator entries
  try { if ((D().sessioni || []).length && typeof disegnaElenco === "function") disegnaElenco(); } catch {}
  mark();
})();
